//! `?[[…]]` marking — port of `scripts/wiki-resolve-mark`, corrected.
//!
//! SPEC (0.2.0): one rule, [`transform`], applied to a note's text outside its
//! `## Backlinks` section, occurrence by occurrence:
//! * target exists (case-insensitive, escaped — bug 7) → unmarked `[[…]]`;
//! * embed `![[…]]` or inbox `>[[…]]` → never marked, stale marks removed (bug 8);
//! * target missing and the name passes the smart filter → `?[[…]]` (one `?`);
//! * otherwise left as it is.
//!
//! Write applies it to the written note. Create / Remove re-apply it to
//! every note mentioning the name (bug 1); Rename = Remove(old) + Create(new).
//! Nothing here ever strips a newline (bug 2). Own writes are recognised by
//! (path, content), not a global marker (bug 9).
//!
//! Kept from the oracle: the smart filter and the >500 KB / >100-link skips.

use crate::wiki::{self, basename, note_name, Ctx, Index, Outcome};
use regex::{Captures, Regex};
use std::path::Path;
use std::sync::OnceLock;

// ── smart filter (unchanged from the oracle's create_filter_config) ─
const ACTION_PREFIXES: &[&str] = &["tel:", "mailto:", "http:", "https:", "ftp:", "file:", "obsidian:"];
const SYSTEM_PATHS: &[&str] = &["C:", "/usr/", "/var/", "/etc/", "~/", "\\\\"];
const AUTO_GENERATED: &[&str] = &["unknown_filename_.*", "temp_.*", "IMG_.*", "Screenshot.*", "Pasted image .*", "image-.*"];
const RESERVED_NAMES: &[&str] = &["^(con|prn|aux|nul|com[1-9]|lpt[1-9])$"];
const UUID_PATTERNS: &[&str] = &[
    "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
    "^[0-9a-f]{32}$",
    "^[0-9a-f]{40}$",
    "^[0-9a-f]{64}$",
];
const SYMLINK_DIRS: &[&str] = &["linked_media/", "attachments/", "assets/"];
const INVALID_CHARS: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
const MIN_LENGTH: usize = 2;
const MAX_LENGTH: usize = 100;

fn filter_regexes() -> &'static Vec<Regex> {
    static RES: OnceLock<Vec<Regex>> = OnceLock::new();
    RES.get_or_init(|| AUTO_GENERATED.iter().chain(RESERVED_NAMES).chain(UUID_PATTERNS).map(|p| Regex::new(p).expect("static regex")).collect())
}

/// Link names that are never marked: lengths in bytes, unanchored
/// case-sensitive regexes, literal prefixes/substrings.
pub fn should_exclude_link(link_name: &str) -> bool {
    let len = link_name.len();
    if !(MIN_LENGTH..=MAX_LENGTH).contains(&len) {
        return true;
    }
    if ACTION_PREFIXES.iter().chain(SYSTEM_PATHS).any(|p| link_name.starts_with(p)) {
        return true;
    }
    if filter_regexes().iter().any(|re| re.is_match(link_name)) {
        return true;
    }
    if SYMLINK_DIRS.iter().any(|d| link_name.contains(d)) {
        return true;
    }
    link_name.contains(INVALID_CHARS)
}

// ── the rule ────────────────────────────────────────────────────────
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transformed {
    pub content: String,
    /// `?` markers added.
    pub added: usize,
    /// `?` markers removed.
    pub removed: usize,
}

fn fix_text(text: &str, index: &Index, added: &mut usize, removed: &mut usize) -> String {
    wiki::link_re()
        .replace_all(text, |c: &Captures| {
            let (flag, marks, inner) = (&c[1], &c[2], &c[3]);
            let (name, _) = wiki::split_inner(inner);
            if !flag.is_empty() {
                if !marks.is_empty() {
                    *removed += 1;
                }
                return format!("{flag}[[{inner}]]");
            }
            if index.resolve(name).is_some() {
                if !marks.is_empty() {
                    *removed += 1;
                }
                return format!("[[{inner}]]");
            }
            if should_exclude_link(name.trim()) {
                return c[0].to_string();
            }
            if marks.is_empty() {
                *added += 1;
            }
            format!("?[[{inner}]]")
        })
        .into_owned()
}

/// Apply the marking rule to everything outside the note's `## Backlinks` section.
pub fn transform(content: &str, index: &Index) -> Transformed {
    let (mut added, mut removed) = (0, 0);
    let (before, after) = wiki::outside_section(content);
    let b = fix_text(before, index, &mut added, &mut removed);
    let content = match wiki::find_section(content) {
        Some(s) => format!("{b}{}{}", &content[s.start..s.end], fix_text(after, index, &mut added, &mut removed)),
        None => b,
    };
    Transformed { content, added, removed }
}

// ── handlers ────────────────────────────────────────────────────────
pub fn handle_change(ctx: &Ctx, operation: &str, file_path: &Path, new_path: Option<&Path>) -> Outcome {
    let mut out = Outcome::default();
    let file_path = &wiki::canon(file_path);
    let new_path = new_path.map(wiki::canon);
    let new_path = new_path.as_deref();
    match operation {
        "Write" | "Create" => {
            if ctx.is_own_write(file_path) {
                return out;
            }
            handle_write(ctx, file_path, operation == "Create", &mut out);
        }
        "Rename" => {
            if let Some(np) = new_path.filter(|p| !p.as_os_str().is_empty()) {
                let (old_name, new_name) = (note_name(file_path), note_name(np));
                ctx.log(&format!("📛 Renamed: {old_name} → {new_name}"));
                let index = Index::build(&ctx.existing_roots());
                reevaluate_mentions(ctx, &index, &old_name, &mut out);
                reevaluate_mentions(ctx, &index, &new_name, &mut out);
            }
        }
        "Remove" => {
            let name = note_name(file_path);
            ctx.log(&format!("🗑️  Deleted: {name}"));
            let index = Index::build(&ctx.existing_roots());
            reevaluate_mentions(ctx, &index, &name, &mut out);
        }
        _ => ctx.log(&format!("❓ Unknown operation: {operation} on {}", file_path.display())),
    }
    out
}

fn handle_write(ctx: &Ctx, file_path: &Path, created: bool, out: &mut Outcome) {
    if created {
        ctx.log(&format!("✨ Created: {}", note_name(file_path)));
    } else {
        ctx.log(&format!("📝 Modified: {}", basename(file_path)));
    }

    if wiki::is_conflict_copy(file_path) {
        ctx.log("   ⚠️  Skipping Syncthing conflict copy - not a note");
        return;
    }
    let size = wiki::file_size(file_path);
    if size > wiki::LARGE_FILE_BYTES {
        ctx.log(&format!("   ⚠️  Skipping large file ({size} B) - likely garbage web clip"));
        return;
    }

    let index = Index::build(&ctx.existing_roots());
    let Some(content) = index.content_of(file_path) else { return };
    let names = wiki::outgoing_names(&content);
    if names.len() > wiki::MAX_LINKS {
        ctx.log(&format!("   ⚠️  Skipping file with {} links - likely garbage", names.len()));
        return;
    }
    if names.is_empty() {
        ctx.log("   No wiki links in file, skipping");
    } else {
        ctx.log(&format!("   Found {} links", names.len()));
        apply(ctx, &index, file_path, &content, out);
    }
    if created {
        reevaluate_mentions(ctx, &index, &note_name(file_path), out);
    }
}

/// Transform one note and save it if anything changed.
fn apply(ctx: &Ctx, index: &Index, path: &Path, content: &str, out: &mut Outcome) -> bool {
    let t = transform(content, index);
    if t.content == content {
        return false;
    }
    match index.write(ctx, path, &t.content) {
        Ok(()) => {
            out.wrote();
            if t.added > 0 {
                ctx.log(&format!("   ⚠️ Marked {} new unresolved links in {}", t.added, basename(path)));
            }
            if t.removed > 0 {
                ctx.log(&format!("   ✅ Unmarked {} resolved links in {}", t.removed, basename(path)));
            }
            true
        }
        Err(e) => {
            out.fail(format!("save {}: {e}", path.display()));
            ctx.log(&format!("   ❌ Failed to update: {}", basename(path)));
            false
        }
    }
}

/// Re-apply the rule to every note that links `name` (any form, case-insensitive, NFC or NFD spelling).
fn reevaluate_mentions(ctx: &Ctx, index: &Index, name: &str, out: &mut Outcome) {
    let Ok(re) = Regex::new(&format!(r"(?i)\[\[\s*{}\s*(?:[|#\]])", wiki::name_pattern(name))) else { return };
    let mut changed = 0;
    for i in 0..index.files().len() {
        let Some(content) = index.content(i) else { continue };
        if !re.is_match(&content) {
            continue;
        }
        if apply(ctx, index, &index.files()[i].clone(), &content, out) {
            changed += 1;
        }
    }
    if changed > 0 {
        ctx.log(&format!("   🧹 Re-evaluated ?[[ markers in {changed} files"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_filter() {
        for s in ["a", "https://x", "mailto:me", "C:\\x", "~/y", "IMG_0001", "my_temp_file", "Screenshot 2026", "con", "lpt1", "0123456789abcdef0123456789abcdef", "attachments/x", "a:b", "what?", "a|b", &"x".repeat(101)] {
            assert!(should_exclude_link(s), "{s} should be excluded");
        }
        for s in ["ab", "Real Note", "Foo (bar)", "2026-09-02", "CON", "é", &"x".repeat(100)] {
            assert!(!should_exclude_link(s), "{s} should NOT be excluded");
        }
    }

    #[test]
    fn transform_rules() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("Forge");
        std::fs::create_dir_all(&f).unwrap();
        for n in ["Alpha.md", "Foo (bar).md", "C++.md"] {
            std::fs::write(f.join(n), "").unwrap();
        }
        let ix = Index::build(&[f]);
        let t = transform(
            "[[Gone]] [[Gone|al]] [[Gone#h]] ?[[Gone]] ??[[Gone]] [[alpha]] ?[[Alpha|x]] ![[img.png]] !?[[img.png]] >[[In]] >?[[In]] [[https://x]] [[a]] ?[[a]] [[foo (bar)]] [[c++]] [[Note: x]]\n\n## Backlinks\n\n- [[Gone]]\n",
            &ix,
        );
        assert_eq!(t.content, "?[[Gone]] ?[[Gone|al]] ?[[Gone#h]] ?[[Gone]] ?[[Gone]] [[alpha]] [[Alpha|x]] ![[img.png]] ![[img.png]] >[[In]] >[[In]] [[https://x]] [[a]] ?[[a]] [[foo (bar)]] [[c++]] [[Note: x]]\n\n## Backlinks\n\n- [[Gone]]\n");
        assert_eq!((t.added, t.removed), (3, 3));
        let same = transform(&t.content, &ix);
        assert_eq!(same.content, t.content);
        assert_eq!((same.added, same.removed), (0, 0));
        assert_eq!(transform("x [[Gone]]", &ix).content, "x ?[[Gone]]");
    }

    /// Create/Remove of an NFD-named note (as macOS reports it) must find the
    /// NFC `[[…]]` mentions in other notes and re-evaluate their markers; before
    /// the boundary fix the pattern was built from the raw stem and matched
    /// nothing, leaving `?[[` in place until the next full reconcile.
    #[test]
    fn create_and_remove_of_nfd_named_note_reevaluate_nfc_mentions() {
        let d = tempfile::tempdir().unwrap();
        let forge = d.path().join("Forge");
        std::fs::create_dir_all(&forge).unwrap();
        let mention = forge.join("Mention.md");
        std::fs::write(&mention, "see ?[[Zoë Harcombe]] and ?[[Zoe\u{0308} Harcombe|z]]\n").unwrap();
        let ctx = Ctx::new(vec![forge.clone()], crate::logger::Logger::silent());
        let target = forge.join("Zoe\u{0308} Harcombe.md");
        std::fs::write(&target, "").unwrap();

        let out = handle_change(&ctx, "Create", &target, None);
        assert_eq!(out.error, None);
        assert_eq!(std::fs::read_to_string(&mention).unwrap(), "see [[Zoë Harcombe]] and [[Zoe\u{0308} Harcombe|z]]\n", "both spellings unmarked, bytes otherwise untouched");

        std::fs::remove_file(&target).unwrap();
        let out = handle_change(&ctx, "Remove", &target, None);
        assert_eq!(out.error, None);
        assert_eq!(std::fs::read_to_string(&mention).unwrap(), "see ?[[Zoë Harcombe]] and ?[[Zoe\u{0308} Harcombe|z]]\n", "both spellings re-marked");
    }
}
