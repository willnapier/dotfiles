//! One-shot note operations for the CLI (0.2.8): `backlinks`, `rename`, `new`,
//! `promote`. They replace Nushell commands that resolved a typed note name by
//! byte-matching `fd`/`rg` output — which missed every NFD-named note on macOS
//! (`note-rename` then renamed the file anyway and orphaned its inbound links).
//!
//! Every name→file step goes through `forge_names` (either spelling,
//! case-insensitive) and the returned path is the walked one; a new file is
//! only ever created after the resolver has said no equivalent note exists.
//! `rename` runs the SAME fan-out the live watchers run for a rename event
//! (`backlinks::handle_change`, `resolve::handle_change`), so it is correct
//! whether or not the service is running; when it is, the watchers re-process
//! the resulting events idempotently.

use crate::wiki::{self, Ctx, Index};
use crate::{backlinks, resolve};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Resolve `name` (a link name, `Dir/Name`, `.md` accepted, or an existing
/// path) to exactly one note under `roots`.
pub fn resolve_one(roots: &[PathBuf], name: &str) -> Result<PathBuf> {
    let as_path = Path::new(name);
    if as_path.is_file() {
        return Ok(wiki::canon(as_path));
    }
    let found = forge_names::find_note(roots, name);
    match found.len() {
        0 => bail!("no note named {name:?} under the roots"),
        1 => Ok(found.into_iter().next().unwrap()),
        n => bail!("{n} notes named {name:?}; give a Dir/Name or a path:\n{}", found.iter().map(|p| format!("  {}", p.display())).collect::<Vec<_>>().join("\n")),
    }
}

/// Paths of the notes whose links resolve to `name`.
pub fn backlinks(roots: &[PathBuf], name: &str) -> Result<Vec<PathBuf>> {
    let target = resolve_one(roots, name)?;
    let index = Index::build(roots);
    let Some(i) = index.position(&target) else { bail!("{} is not in the index (hidden, conflict copy, or outside the roots)", target.display()) };
    Ok(index.backlink_sources(i).into_iter().map(|j| index.files()[j].clone()).collect())
}

/// Rename the note `old` to the NFC name `new` (same directory), then rewrite
/// every `[[old]]` link in either spelling, rebuild the sections and
/// re-evaluate `?[[` markers — exactly what the watchers do on a rename event.
/// Refuses when a note with the new name already exists anywhere under the
/// roots, because a bare `[[new]]` would then be ambiguous.
pub fn rename(ctx: &Ctx, old: &str, new: &str) -> Result<(PathBuf, PathBuf)> {
    let old_path = resolve_one(&ctx.roots, old)?;
    let new_name = forge_names::nfc(new.trim().trim_end_matches(".md"));
    if new_name.is_empty() || new_name.contains('/') {
        bail!("new name must be a bare note name, got {new:?}");
    }
    if let Some(existing) = forge_names::find_note(&ctx.roots, &new_name).first() {
        bail!("a note named {new_name:?} already exists: {}", existing.display());
    }
    let dir = old_path.parent().context("note has no parent directory")?;
    let new_path = dir.join(format!("{new_name}.md"));
    std::fs::rename(&old_path, &new_path).with_context(|| format!("renaming {} → {}", old_path.display(), new_path.display()))?;
    let mut failures = Vec::new();
    for out in [backlinks::handle_change(ctx, "Rename", &old_path, Some(&new_path)), resolve::handle_change(ctx, "Rename", &old_path, Some(&new_path))] {
        if let Some(e) = out.error {
            failures.push(e);
        }
    }
    if !failures.is_empty() {
        bail!("renamed, but the link fan-out reported: {}", failures.join("; "));
    }
    Ok((old_path, new_path))
}

pub struct NewNote {
    pub path: PathBuf,
    /// False when an equivalent note already existed and `path` is that note.
    pub created: bool,
}

/// Create `<dir or roots[0]>/<NFC name>.md` with the standard frontmatter —
/// unless a note of that name already exists anywhere under the roots, in
/// which case return it and create nothing (this is what stops the NFC twin
/// beside an NFD-named file).
pub fn new_note(roots: &[PathBuf], name: &str, dir: Option<&Path>) -> Result<NewNote> {
    let name = forge_names::nfc(name.trim().trim_end_matches(".md"));
    if name.is_empty() || name.contains('/') {
        bail!("note name must be a bare name, got {name:?}");
    }
    if let Some(existing) = forge_names::find_note(roots, &name).first() {
        return Ok(NewNote { path: existing.clone(), created: false });
    }
    let dir = match dir {
        Some(d) => d.to_path_buf(),
        None => roots.first().context("no root")?.clone(),
    };
    let path = dir.join(format!("{name}.md"));
    let now = chrono::Local::now();
    let stamp = now.format("%Y-%m-%d %H:%M");
    let content = format!("---\ndate created: {stamp}\ndate modified: {stamp}\n---\n# {name}\n\n");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&path, content).with_context(|| format!("creating {}", path.display()))?;
    Ok(NewNote { path, created: true })
}

/// Move `<roots[0]>/Reception/<name>.md` (either spelling) to `<roots[0]>/<NFC name>.md`.
/// Refuses when the destination already exists in either spelling.
pub fn promote(roots: &[PathBuf], name: &str) -> Result<(PathBuf, PathBuf)> {
    let root = roots.first().context("no root")?;
    let name = forge_names::nfc(name.trim().trim_end_matches(".md"));
    let reception = root.join("Reception");
    let Some(src) = forge_names::find_in_dir(&reception, &format!("{name}.md")) else { bail!("no note named {name:?} in {}", reception.display()) };
    if let Some(existing) = forge_names::find_in_dir(root, &format!("{name}.md")) {
        bail!("a permanent note already exists: {}", existing.display());
    }
    let dst = root.join(format!("{name}.md"));
    std::fs::rename(&src, &dst).with_context(|| format!("moving {} → {}", src.display(), dst.display()))?;
    Ok((src, dst))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logger::Logger;
    const NFD: &str = "Zoe\u{0308} Example";
    const NFC: &str = "Zoë Example";

    fn forge() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("Forge");
        std::fs::create_dir_all(f.join("Reception")).unwrap();
        std::fs::write(f.join(format!("{NFD}.md")), "# Zoë\n\n[[Other]]\n").unwrap();
        std::fs::write(f.join("Other.md"), format!("see [[{NFC}]] and ?[[Nowhere]]\n")).unwrap();
        std::fs::write(f.join("Alias.md"), format!("see [[{NFD}|z]]\n")).unwrap();
        (d, f)
    }

    #[test]
    fn backlinks_resolve_either_spelling_and_list_both_link_forms() {
        let (_d, f) = forge();
        let roots = vec![f.clone()];
        let mut got = backlinks(&roots, NFC).unwrap();
        got.sort();
        assert_eq!(got, vec![f.join("Alias.md"), f.join("Other.md")]);
        assert_eq!(backlinks(&roots, NFD).unwrap().len(), 2);
        assert!(backlinks(&roots, "nope").is_err());
    }

    #[test]
    fn rename_rewrites_links_in_both_spellings_and_refuses_twins() {
        let (_d, f) = forge();
        let ctx = Ctx::new(vec![f.clone()], Logger::silent());
        let (old, new) = rename(&ctx, NFC, "Zoë Renamed").unwrap();
        // Ctx canonicalises roots (macOS: /var → /private/var), so compare canonical paths.
        assert_eq!(new, f.join("Zoë Renamed.md").canonicalize().unwrap());
        assert!(!old.exists() && new.exists());
        assert_eq!(std::fs::read_to_string(f.join("Other.md")).unwrap(), "see [[Zoë Renamed]] and ?[[Nowhere]]\n");
        assert_eq!(std::fs::read_to_string(f.join("Alias.md")).unwrap(), "see [[Zoë Renamed|z]]\n");
        assert!(rename(&ctx, "Other", "Zoë Renamed").is_err(), "existing name refused");
        assert!(rename(&ctx, "Other", "a/b").is_err());
    }

    #[test]
    fn new_note_returns_the_existing_nfd_file_instead_of_a_twin() {
        let (_d, f) = forge();
        let roots = vec![f.clone()];
        let n = new_note(&roots, NFC, None).unwrap();
        assert!(!n.created);
        assert_eq!(n.path, f.join(format!("{NFD}.md")));
        assert_eq!(std::fs::read_dir(&f).unwrap().filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().starts_with("Zo")).count(), 1);
        let n = new_note(&roots, "Brand New", None).unwrap();
        assert!(n.created);
        let c = std::fs::read_to_string(&n.path).unwrap();
        assert!(c.starts_with("---\ndate created: ") && c.ends_with("---\n# Brand New\n\n"), "{c:?}");
    }

    #[test]
    fn promote_finds_the_nfd_reception_note_from_its_nfc_name() {
        let (_d, f) = forge();
        let roots = vec![f.clone()];
        std::fs::write(f.join("Reception").join("Cafe\u{0301} Idea.md"), "x").unwrap();
        let (src, dst) = promote(&roots, "Café Idea").unwrap();
        assert!(src.starts_with(f.join("Reception")));
        assert_eq!(dst, f.join("Café Idea.md"));
        assert!(dst.exists());
        assert!(promote(&roots, "Café Idea").is_err(), "gone from Reception");
        std::fs::write(f.join("Reception").join("Other.md"), "x").unwrap();
        assert!(promote(&roots, "Other").is_err(), "destination exists");
    }
}
