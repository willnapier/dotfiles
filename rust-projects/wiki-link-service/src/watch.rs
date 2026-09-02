//! The event loop — a port of what nu's `watch $forge --glob "**/*.md"
//! --debounce-ms N {|op, path, new_path| handle_change …}` does around the
//! handlers. Same debouncer crate and family as nu 0.106 (`notify-debouncer-full`
//! 0.3), same EventKind → operation mapping, same glob check (on the event's
//! path; for a rename, on the OLD path), same last-path (`paths.pop()`) choice.
//!
//! Only the FIRST root (`~/Forge`) is watched, as in the oracle; every root
//! is scanned by the handlers.

use crate::heartbeat::Heartbeat;
use crate::wiki::{Ctx, Outcome};
use anyhow::{bail, Context, Result};
use notify::event::{DataChange, ModifyKind, RenameMode};
use notify::{EventKind, RecursiveMode, Watcher};
use notify_debouncer_full::new_debouncer;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    Backlinks,
    ResolveMark,
}

impl Which {
    pub fn sub(self) -> &'static str {
        match self {
            Which::Backlinks => "backlinks",
            Which::ResolveMark => "resolve-mark",
        }
    }
    /// The log file name `link-service` used for each watcher.
    pub fn log_file(self) -> &'static str {
        match self {
            Which::Backlinks => "backlinks.out.log",
            Which::ResolveMark => "resolve.out.log",
        }
    }
    /// The oracle's `handle_change` for this watcher.
    pub fn handle(self, ctx: &Ctx, op: &str, path: &Path, new_path: Option<&Path>) -> Outcome {
        match self {
            Which::Backlinks => crate::backlinks::handle_change(ctx, op, path, new_path),
            Which::ResolveMark => crate::resolve::handle_change(ctx, op, path, new_path),
        }
    }
}

/// nu: `Pattern::new("<root>/**/*.md").matches_path(path)` with default
/// MatchOptions — `*` may match `/` and leading dots, so this is simply
/// "ends with .md" (case-sensitive) for anything under the watched root.
pub fn matches_glob(path: &Path) -> bool {
    path.to_string_lossy().ends_with(".md")
}

/// One debounced notify event → (operation, path, new_path), or None when nu
/// would ignore it (metadata-only modifies, unstitched renames, access, …).
pub fn map_event(kind: EventKind, mut paths: Vec<PathBuf>) -> Option<(&'static str, PathBuf, Option<PathBuf>)> {
    match kind {
        EventKind::Create(_) => paths.pop().map(|p| ("Create", p, None)),
        EventKind::Remove(_) => paths.pop().map(|p| ("Remove", p, None)),
        EventKind::Modify(ModifyKind::Data(DataChange::Content)) | EventKind::Modify(ModifyKind::Data(DataChange::Any)) | EventKind::Modify(ModifyKind::Any) => {
            paths.pop().map(|p| ("Write", p, None))
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
            let to = paths.pop()?;
            let from = paths.pop()?;
            Some(("Rename", from, Some(to)))
        }
        _ => None,
    }
}

fn banner(which: Which, ctx: &Ctx, debounce_ms: u64) {
    match which {
        Which::Backlinks => ctx.log("🔗 Starting wiki backlinks manager..."),
        Which::ResolveMark => ctx.log("🔗 Starting wiki resolve marker..."),
    }
    for (i, root) in ctx.roots.iter().enumerate() {
        let suffix = if i == 0 { "" } else { " - optional" };
        ctx.log(&format!("   {}: {}{suffix}", root.file_name().unwrap_or_default().to_string_lossy(), root.display()));
    }
    match which {
        Which::Backlinks => {
            ctx.log("   Feature: Automatic ## Backlinks maintenance");
            ctx.log("   Mode: Event-driven - zero CPU when idle");
        }
        Which::ResolveMark => {
            ctx.log("   Feature: Clean ?[[ markers when files created/renamed/removed");
            ctx.log("   Mode: Event-driven (Create/Rename/Remove only)");
            ctx.log("   Note: Marking unresolved links happens via batch scan");
        }
    }
    ctx.log(&format!("📂 Watching {} directories for markdown files", ctx.existing_roots().len()));
    ctx.log(&format!("⏱️  Debounce: {debounce_ms}"));
}

/// Watch `ctx.roots[0]` forever, dispatching each event to `which`'s handler
/// and stamping the heartbeat after every one. Returns only on failure.
pub fn run(which: Which, ctx: &Ctx, debounce_ms: u64, heartbeat: &mut Heartbeat) -> Result<()> {
    let primary = ctx.roots.first().context("no root directory to watch")?;
    if !primary.exists() {
        ctx.log("❌ Forge directory not found");
        bail!("{} does not exist", primary.display());
    }
    banner(which, ctx, debounce_ms);

    let (tx, rx) = std::sync::mpsc::channel();
    let mut debouncer = new_debouncer(Duration::from_millis(debounce_ms), None, tx).context("creating debouncer")?;
    debouncer.watcher().watch(primary, RecursiveMode::Recursive).with_context(|| format!("watching {}", primary.display()))?;
    debouncer.cache().add_root(primary, RecursiveMode::Recursive);
    ctx.log("🔍 Monitoring Forge for file events...");

    for res in rx {
        match res {
            Ok(events) => {
                for ev in events {
                    let Some((op, path, new_path)) = map_event(ev.event.kind, ev.event.paths) else { continue };
                    if !matches_glob(&path) {
                        continue;
                    }
                    let out = which.handle(ctx, op, &path, new_path.as_deref());
                    heartbeat.cycle(out.actions, out.error);
                }
            }
            Err(errs) => {
                let msg = format!("❌ watch error: {errs:?}");
                ctx.log(&msg);
                heartbeat.cycle(0, Some(msg));
            }
        }
    }
    bail!("file watcher channel closed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, MetadataKind, RemoveKind};

    #[test]
    fn event_mapping_matches_nu_watch() {
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(map_event(EventKind::Create(CreateKind::File), vec![p("/a.md")]), Some(("Create", p("/a.md"), None)));
        assert_eq!(map_event(EventKind::Remove(RemoveKind::File), vec![p("/a.md")]), Some(("Remove", p("/a.md"), None)));
        assert_eq!(map_event(EventKind::Modify(ModifyKind::Data(DataChange::Content)), vec![p("/a.md")]), Some(("Write", p("/a.md"), None)));
        assert_eq!(map_event(EventKind::Modify(ModifyKind::Any), vec![p("/a.md")]), Some(("Write", p("/a.md"), None)));
        assert_eq!(map_event(EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)), vec![p("/a.md")]), None);
        assert_eq!(map_event(EventKind::Modify(ModifyKind::Name(RenameMode::From)), vec![p("/a.md")]), None);
        assert_eq!(map_event(EventKind::Modify(ModifyKind::Name(RenameMode::Both)), vec![p("/old.md"), p("/new.md")]), Some(("Rename", p("/old.md"), Some(p("/new.md")))));
        assert_eq!(map_event(EventKind::Access(notify::event::AccessKind::Any), vec![p("/a.md")]), None);
        assert!(matches_glob(Path::new("/x/.hidden/a.md")));
        assert!(!matches_glob(Path::new("/x/a.MD")));
        assert!(!matches_glob(Path::new("/x/a.md.bak")));
    }
}
