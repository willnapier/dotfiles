//! `## Backlinks` maintenance — port of `scripts/wiki-backlinks`, corrected.
//!
//! SPEC (0.2.0):
//! * bug 3 — links inside a section are not outgoing links (no cascade).
//! * bug 4 — when a note is written (or deleted) every note whose section
//!   lists it is rebuilt, so an entry disappears when the link does.
//! * bug 5 — only the section is rewritten (`wiki::with_section`); no
//!   half-size heuristic. Sections are written only when they change.
//! * bug 6 — `[[x|alias]]`/`[[x#h]]` are links to x; rename preserves the
//!   alias/heading part; an alias-only target gets a real entry.
//! * bug 7 — case-insensitive, regex-escaped resolution via `wiki::Index`.
//! * bug 9 — own writes are recognised by (path, content) not a global marker.
//!
//! Kept from the oracle: the >500 KB and >100-link "garbage web clip" skips
//! and the "< 10 bytes" skip when writing a section.

use crate::wiki::{self, basename, note_name, Ctx, Index, Outcome};
use regex::{Captures, Regex};
use std::path::Path;

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
                handle_rename(ctx, file_path, np, &mut out);
            }
        }
        "Remove" => handle_remove(ctx, file_path, &mut out),
        _ => ctx.log(&format!("❓ Unknown operation: {operation} on {}", file_path.display())),
    }
    out
}

/// Indices of the notes whose `## Backlinks` section lists `name`.
fn notes_listing(index: &Index, name: &str, except: &Path) -> Vec<usize> {
    let key = wiki::name_key(name);
    (0..index.files().len())
        .filter(|&i| index.files()[i] != except)
        .filter(|&i| index.content(i).is_some_and(|c| wiki::section_entries(&c).iter().any(|n| wiki::name_key(n) == key)))
        .collect()
}

fn handle_write(ctx: &Ctx, file_path: &Path, created: bool, out: &mut Outcome) {
    if created {
        ctx.log(&format!("✨ Created: {}", note_name(file_path)));
    } else {
        ctx.log(&format!("📝 Modified: {}", basename(file_path)));
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
        ctx.log("   No wiki links in file");
    } else {
        ctx.log(&format!("   Found {} links", names.len()));
    }

    // Targets of the current links, then (bug 4) every note still listing this one.
    let mut targets: Vec<usize> = Vec::new();
    for n in &names {
        if let Some(t) = index.resolve_idx(n) {
            if !targets.contains(&t) {
                targets.push(t);
            }
        }
    }
    for t in notes_listing(&index, &note_name(file_path), file_path) {
        if !targets.contains(&t) {
            targets.push(t);
        }
    }
    for t in targets {
        update_backlinks(ctx, &index, t, out);
    }
}

/// A deleted note links nothing any more: drop it from every section that lists it.
fn handle_remove(ctx: &Ctx, file_path: &Path, out: &mut Outcome) {
    let name = note_name(file_path);
    ctx.log(&format!("🗑️  Deleted: {name}"));
    let index = Index::build(&ctx.existing_roots());
    for t in notes_listing(&index, &name, file_path) {
        update_backlinks(ctx, &index, t, out);
    }
}

fn handle_rename(ctx: &Ctx, old_path: &Path, new_path: &Path, out: &mut Outcome) {
    let old_name = note_name(old_path);
    let new_name = note_name(new_path);
    ctx.log(&format!("📛 Renamed: {old_name} → {new_name}"));

    let index = Index::build(&ctx.existing_roots());
    let mut updated = 0;
    for i in 0..index.files().len() {
        let Some(content) = index.content(i) else { continue };
        let Some(new) = rewrite_links(&content, &old_name, &new_name) else { continue };
        let path = &index.files()[i];
        match index.write(ctx, path, &new) {
            Ok(()) => {
                updated += 1;
                out.wrote();
                ctx.log(&format!("   ✅ Updated: {}", basename(path)));
            }
            Err(e) => {
                out.fail(format!("rename: failed to update {}: {e}", path.display()));
                ctx.log(&format!("   ❌ Failed to update: {}", basename(path)));
            }
        }
    }
    if updated == 0 {
        ctx.log("   No files link to this note");
    } else {
        ctx.log(&format!("   Updated {updated} files with new link name"));
    }
    if let Some(t) = index.position(new_path) {
        update_backlinks(ctx, &index, t, out);
    }
}

/// Every `[[old]]`, `[[old|alias]]`, `[[old#h]]` (any `!`/`>`/`?` prefix,
/// case-insensitive) → the same with `new`. `None` if nothing matched.
pub fn rewrite_links(content: &str, old: &str, new: &str) -> Option<String> {
    let re = Regex::new(&format!(r"(?i)([!>]?\?*\[\[)\s*{}\s*([|#][^\]\n]*)?(\]\])", regex::escape(old))).ok()?;
    if !re.is_match(content) {
        return None;
    }
    Some(re.replace_all(content, |c: &Captures| format!("{}{}{}{}", &c[1], new, c.get(2).map_or("", |m| m.as_str()), &c[3])).into_owned())
}

/// Rebuild the target's section from the notes that link to it; write only if it changes.
pub fn update_backlinks(ctx: &Ctx, index: &Index, target: usize, out: &mut Outcome) {
    let path = &index.files()[target];
    ctx.log(&format!("   🔗 Updating backlinks for: {}", note_name(path)));
    let names = index.backlink_names(target);
    if names.is_empty() {
        ctx.log("      No backlinks found");
    } else {
        ctx.log(&format!("      Found {} backlinks", names.len()));
    }
    let Some(content) = index.content(target) else { return };
    if content.len() < 10 {
        ctx.log(&format!("      ⚠️  Skipping empty/minimal file: {}", basename(path)));
        return;
    }
    let new = wiki::with_section(&content, &names);
    if new == *content {
        return;
    }
    match index.write(ctx, path, &new) {
        Ok(()) => out.wrote(),
        Err(e) => out.fail(format!("save {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_rewrites_all_forms_case_insensitively() {
        let c = "[[Old]] [[old|alias]] ?[[OLD#h]] ![[Old]] >[[Old]] [[Older]] [[x|Old]] - [[Old]]\n";
        assert_eq!(rewrite_links(c, "Old", "New").unwrap(), "[[New]] [[New|alias]] ?[[New#h]] ![[New]] >[[New]] [[Older]] [[x|Old]] - [[New]]\n");
        assert_eq!(rewrite_links("[[Foo bar]]", "Foo (bar)", "X"), None);
        assert_eq!(rewrite_links("[[Foo (bar)|a]]", "Foo (bar)", "C++").unwrap(), "[[C++|a]]");
        assert_eq!(rewrite_links("[[C++]]", "C++", "Cpp").unwrap(), "[[Cpp]]");
    }
}
