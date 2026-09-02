//! Port of `scripts/wiki-backlinks` — `handle_change` and everything it calls.
//!
//! Section writing follows the watcher oracle, NOT `backlinks-init`: the
//! watcher discards everything after the `## Backlinks` heading and trims the
//! preceding content, whereas backlinks-init preserves following sections.
//! (backlinks-init's `update_backlinks_section` was therefore not reusable
//! here — parity with the watcher is the requirement.)

use crate::wiki::{self, basename, note_name, Ctx, Outcome};
use regex::bytes::Regex;
use std::path::Path;
use std::sync::OnceLock;

/// rg pattern the oracle extracts links with (`[^\]\n]` ≡ rg's per-line `[^\]]`).
pub const LINK_RE: &str = r"[!?]?\[\[([^\]\n]+)\]\]";

fn link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(LINK_RE).expect("static regex"))
}

/// `handle_change [operation, file_path, new_path, watch_paths]`.
pub fn handle_change(ctx: &Ctx, operation: &str, file_path: &Path, new_path: Option<&Path>) -> Outcome {
    let mut out = Outcome::default();
    if wiki::should_skip_event(&ctx.marker) {
        return out;
    }
    match operation {
        "Write" => handle_write(ctx, file_path, &mut out),
        "Create" => handle_create(ctx, file_path, &mut out),
        "Rename" => {
            if let Some(np) = new_path.filter(|p| !p.as_os_str().is_empty()) {
                handle_rename(ctx, file_path, np, &mut out);
            }
        }
        "Remove" => ctx.log(&format!("🗑️  Deleted: {}", basename(file_path))),
        _ => ctx.log(&format!("❓ Unknown operation: {operation} on {}", file_path.display())),
    }
    out
}

fn links_in(file_path: &Path) -> Vec<String> {
    std::fs::read(file_path).map(|b| wiki::extract_links(&b, link_re())).unwrap_or_default()
}

fn handle_create(ctx: &Ctx, file_path: &Path, out: &mut Outcome) {
    let file_name = note_name(file_path);
    ctx.log(&format!("✨ Created: {file_name}"));

    let size = wiki::file_size(file_path);
    if size > wiki::LARGE_FILE_BYTES {
        ctx.log(&format!("   ⚠️  Skipping large file ({size} B) - likely garbage web clip"));
        return;
    }

    let links = links_in(file_path);
    if !links.is_empty() {
        ctx.log(&format!("   Found {} links in new file", links.len()));
        for link in &links {
            let clean = wiki::clean_link(link);
            if let Some(target) = wiki::find_target_file(clean, &ctx.roots) {
                update_backlinks(ctx, &target, out);
            }
        }
    }
}

fn handle_write(ctx: &Ctx, file_path: &Path, out: &mut Outcome) {
    ctx.log(&format!("📝 Modified: {}", basename(file_path)));

    let size = wiki::file_size(file_path);
    if size > wiki::LARGE_FILE_BYTES {
        ctx.log(&format!("   ⚠️  Skipping large file ({size} B) - likely garbage web clip"));
        return;
    }

    let links = links_in(file_path);
    if links.is_empty() {
        ctx.log("   No wiki links in file, skipping");
        return;
    }
    if links.len() > wiki::MAX_LINKS {
        ctx.log(&format!("   ⚠️  Skipping file with {} links - likely garbage", links.len()));
        return;
    }
    ctx.log(&format!("   Found {} links", links.len()));

    // ORACLE BUG (replicated): only the targets of links CURRENTLY in the file
    // are refreshed, so removing `[[B]]` from A leaves A listed in B's
    // `## Backlinks` until something else triggers `update_backlinks B`.
    for link in &links {
        let clean = wiki::clean_link(link);
        if let Some(target) = wiki::find_target_file(clean, &ctx.roots) {
            update_backlinks(ctx, &target, out);
        }
    }
}

fn handle_rename(ctx: &Ctx, old_path: &Path, new_path: &Path, out: &mut Outcome) {
    let old_name = note_name(old_path);
    let new_name = note_name(new_path);
    ctx.log(&format!("📛 Renamed: {old_name} → {new_name}"));

    let mut affected = Vec::new();
    for dir in ctx.existing_roots() {
        affected.extend(wiki::rg_files(&format!(r"\[\[{old_name}\]\]"), &dir));
    }
    if affected.is_empty() {
        ctx.log("   No files link to this note");
        return; // the renamed file's own section is not refreshed in this case (oracle)
    }
    ctx.log(&format!("   Updating {} files with new link name", affected.len()));

    for file in &affected {
        let saved = wiki::read_text(file).and_then(|content| {
            // The second replace is a no-op after the first (`?[[old]]` contains `[[old]]`); kept as in the oracle.
            let updated = content
                .replace(&format!("[[{old_name}]]"), &format!("[[{new_name}]]"))
                .replace(&format!("?[[{old_name}]]"), &format!("?[[{new_name}]]"));
            // NB: no mark_writing here — the backlinks oracle omits it on rename.
            wiki::save(file, &updated).ok()
        });
        match saved {
            Some(()) => {
                out.wrote();
                ctx.log(&format!("   ✅ Updated: {}", basename(file)));
            }
            None => {
                out.fail(format!("rename: failed to update {}", file.display()));
                ctx.log(&format!("   ❌ Failed to update: {}", basename(file)));
            }
        }
    }

    update_backlinks(ctx, new_path, out);
}

/// `update_backlinks`: rebuild the target's `## Backlinks` from every file
/// containing the regex `\[\[<name>\]\]` (so `?[[name]]`/`![[name]]` count
/// but `[[name|alias]]`/`[[name#h]]` do NOT — oracle behaviour).
pub fn update_backlinks(ctx: &Ctx, file_path: &Path, out: &mut Outcome) {
    let file_name = note_name(file_path);
    ctx.log(&format!("   🔗 Updating backlinks for: {file_name}"));

    let mut backlinks = Vec::new();
    for dir in ctx.existing_roots() {
        backlinks.extend(wiki::rg_files(&format!(r"\[\[{file_name}\]\]"), &dir).into_iter().filter(|p| p.as_os_str() != file_path.as_os_str()));
    }

    if backlinks.is_empty() {
        ctx.log("      No backlinks found");
        ensure_backlinks_section(ctx, file_path, &[], out);
        return;
    }
    ctx.log(&format!("      Found {} backlinks", backlinks.len()));
    let list: Vec<String> = backlinks.iter().map(|p| format!("- [[{}]]", note_name(p))).collect();
    ensure_backlinks_section(ctx, file_path, &list, out);
}

/// `ensure_backlinks_section` — byte-exact port. Returns whether a save happened.
pub fn ensure_backlinks_section(ctx: &Ctx, file_path: &Path, backlinks: &[String], out: &mut Outcome) -> bool {
    let Some(content) = wiki::read_text(file_path) else { return false };
    let name = basename(file_path);

    // nu `str length` counts bytes.
    if content.len() < 10 {
        ctx.log(&format!("      ⚠️  Skipping empty/minimal file: {name}"));
        return false;
    }

    let section = if backlinks.is_empty() {
        "\n\n## Backlinks\n".to_string()
    } else {
        format!("\n\n## Backlinks\n\n{}\n", backlinks.join("\n"))
    };

    let updated = if content.contains("## Backlinks") {
        let lines: Vec<&str> = content.lines().collect();
        let Some(idx) = lines.iter().position(|l| l.starts_with("## Backlinks")) else {
            ctx.log(&format!("      ⚠️  Backlinks section detection failed for: {name}"));
            return false;
        };
        if idx < 2 {
            ctx.log(&format!("      ⚠️  Backlinks section too early in file: {name}"));
            return false;
        }
        // ORACLE BUG (replicated): everything from the heading to EOF is
        // discarded (any later section is lost) and the surviving content is
        // trimmed at BOTH ends (leading blank lines/indentation vanish).
        let before = lines[..idx].join("\n").trim().to_string();
        let updated = format!("{before}{section}");
        if (updated.len() as f64) < (content.len() as f64) / 2.0 {
            ctx.log(&format!("      ⚠️  Refusing to write potentially corrupted content to: {name}"));
            return false;
        }
        updated
    } else {
        // Appended verbatim: a file ending in "\n" gets "\n\n\n## Backlinks".
        format!("{content}{section}")
    };

    wiki::mark_writing(&ctx.marker, "backlinks");
    match wiki::save(file_path, &updated) {
        Ok(()) => {
            out.wrote();
            true
        }
        Err(e) => {
            out.fail(format!("save {}: {e}", file_path.display()));
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logger::Logger;

    fn ctx(d: &Path) -> Ctx {
        Ctx { roots: vec![d.join("Forge")], marker: d.join("marker"), logger: Logger::silent() }
    }

    #[test]
    fn section_shapes() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("Forge")).unwrap();
        let c = ctx(d.path());
        let f = d.path().join("Forge/T.md");
        let mut out = Outcome::default();

        std::fs::write(&f, "# T\n\nbody\n").unwrap();
        assert!(ensure_backlinks_section(&c, &f, &["- [[A]]".into()], &mut out));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "# T\n\nbody\n\n\n## Backlinks\n\n- [[A]]\n");

        // existing section: content after it is discarded, leading whitespace trimmed
        std::fs::write(&f, "\n  # T\n\nbody\n\n## Backlinks\n\n- [[A]]\n\n## Later\n\nkept?\n").unwrap();
        assert!(ensure_backlinks_section(&c, &f, &["- [[B]]".into(), "- [[C]]".into()], &mut out));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "# T\n\nbody\n\n## Backlinks\n\n- [[B]]\n- [[C]]\n");

        // empty list → empty section
        assert!(ensure_backlinks_section(&c, &f, &[], &mut out));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "# T\n\nbody\n\n## Backlinks\n");

        // "### Backlinks" contains the substring but no line starts with it → untouched
        std::fs::write(&f, "# T\n\nbody\n\n### Backlinks\n\n- [[A]]\n").unwrap();
        assert!(!ensure_backlinks_section(&c, &f, &["- [[B]]".into()], &mut out));
        // too early / too short / half-size refusal
        std::fs::write(&f, "## Backlinks\n\n- [[A]]\n").unwrap();
        assert!(!ensure_backlinks_section(&c, &f, &[], &mut out));
        std::fs::write(&f, "tiny\n").unwrap();
        assert!(!ensure_backlinks_section(&c, &f, &[], &mut out));
        std::fs::write(&f, "# T\n\nb\n\n## Backlinks\n\n- [[A]]\n- [[B]]\n- [[C]]\n- [[D]]\n- [[E]]\n- [[F]]\n").unwrap();
        assert!(!ensure_backlinks_section(&c, &f, &[], &mut out));
        assert_eq!(out.actions, 3);
        assert!(c.marker.exists());
    }
}
