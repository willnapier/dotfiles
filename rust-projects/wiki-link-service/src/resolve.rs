//! Port of `scripts/wiki-resolve-mark` — `handle_change` and everything it calls.
//!
//! ORACLE BUG (replicated, the big one): the Create / Remove / Rename paths
//! build their `rg -l` pattern with `$'\\[\\[…\\]\\]'`. Nushell `$'…'`
//! strings do not process backslash escapes, so rg receives the literal
//! `\\[\\[name\\]\\]` — a regex for "a backslash followed by one of
//! `\ n a m e`" — which never matches a wiki link. Consequently, in the oracle:
//! creating a target does NOT unmark `?[[target]]`, deleting a target does NOT
//! mark `[[target]]`, and renaming does NOT rewrite links — except in files
//! that happen to contain a backslash sequence such as `\n` in a code span.
//! Only the Write path (marking links whose target is missing) works.
//! `wiki-resolve-batch` (Rust) is what actually cleans `?[[` markers.
//!
//! `wiki-resolve-batch`'s marker regex was not reused: it strips `.md` from
//! link names and normalises `??[[`, neither of which the watcher does.

use crate::wiki::{self, basename, note_name, Ctx, Outcome};
use regex::bytes::Regex as BytesRegex;
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

/// The oracle also accepts `>[[inbox]]` here (but see `mark_unresolved_in_file`).
pub const LINK_RE: &str = r"[!?>]?\[\[([^\]\n]+)\]\]";
const PLACEHOLDER: &str = "QMARK_DBRACKET_PLACEHOLDER";

fn link_re() -> &'static BytesRegex {
    static RE: OnceLock<BytesRegex> = OnceLock::new();
    RE.get_or_init(|| BytesRegex::new(LINK_RE).expect("static regex"))
}

// ── smart filter (ported from the oracle's create_filter_config) ────
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

/// `should_exclude_link` — lengths are bytes (nu `str length`), regexes are
/// unanchored searches (`echo $name | rg $pattern`), case-sensitive.
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

// ── handlers ────────────────────────────────────────────────────────
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
        "Remove" => handle_remove(ctx, file_path, &mut out),
        _ => ctx.log(&format!("❓ Unknown operation: {operation} on {}", file_path.display())),
    }
    out
}

fn handle_create(ctx: &Ctx, file_path: &Path, out: &mut Outcome) {
    let file_name = note_name(file_path);
    ctx.log(&format!("✨ Created: {file_name}"));
    clean_resolved_links(ctx, &file_name, out);
}

fn handle_write(ctx: &Ctx, file_path: &Path, out: &mut Outcome) {
    ctx.log(&format!("📝 Modified: {}", basename(file_path)));

    let size = wiki::file_size(file_path);
    if size > wiki::LARGE_FILE_BYTES {
        ctx.log(&format!("   ⚠️  Skipping large file ({size} B) - likely garbage web clip"));
        return;
    }

    let links = std::fs::read(file_path).map(|b| wiki::extract_links(&b, link_re())).unwrap_or_default();
    if links.is_empty() {
        ctx.log("   No wiki links in file, skipping");
        return;
    }
    if links.len() > wiki::MAX_LINKS {
        ctx.log(&format!("   ⚠️  Skipping file with {} links - likely garbage", links.len()));
        return;
    }
    ctx.log(&format!("   Found {} links", links.len()));
    mark_unresolved_in_file(ctx, file_path, &links, out);
}

fn handle_rename(ctx: &Ctx, old_path: &Path, new_path: &Path, out: &mut Outcome) {
    let old_name = note_name(old_path);
    let new_name = note_name(new_path);
    ctx.log(&format!("📛 Renamed: {old_name} → {new_name}"));

    // ORACLE BUG: double-escaped pattern (see module docs).
    let mut affected = Vec::new();
    for dir in ctx.existing_roots() {
        affected.extend(wiki::rg_files(&format!(r"\\[\\[{old_name}\\]\\]"), &dir));
    }
    if affected.is_empty() {
        ctx.log("   No files link to this note");
        return;
    }
    ctx.log(&format!("   Updating {} files with new link name", affected.len()));

    for file in &affected {
        let saved = wiki::read_text(file).and_then(|content| {
            let updated = content
                .replace(&format!("[[{old_name}]]"), &format!("[[{new_name}]]"))
                .replace(&format!("?[[{old_name}]]"), &format!("?[[{new_name}]]"));
            wiki::mark_writing(&ctx.marker, "resolve-mark");
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
}

fn handle_remove(ctx: &Ctx, file_path: &Path, out: &mut Outcome) {
    let file_name = note_name(file_path);
    ctx.log(&format!("🗑️  Deleted: {file_name}"));

    // ORACLE BUG: double-escaped pattern (see module docs).
    let mut affected = Vec::new();
    for dir in ctx.existing_roots() {
        affected.extend(wiki::rg_files(&format!(r"\\[\\[{file_name}\\]\\]"), &dir));
    }
    if affected.is_empty() {
        ctx.log("   No files link to this note");
        return;
    }
    ctx.log(&format!("   Marking {} references as unresolved", affected.len()));

    for file in &affected {
        let saved = wiki::read_text(file).and_then(|content| {
            let protected = content.replace("?[[", PLACEHOLDER);
            let marked = protected.replace(&format!("[[{file_name}]]"), &format!("?[[{file_name}]]"));
            let updated = marked.replace(PLACEHOLDER, "?[[");
            wiki::mark_writing(&ctx.marker, "resolve-mark");
            wiki::save(file, &updated).ok()
        });
        match saved {
            Some(()) => {
                out.wrote();
                ctx.log(&format!("   ⚠️ Marked unresolved: {}", basename(file)));
            }
            None => {
                out.fail(format!("remove: failed to update {}", file.display()));
                ctx.log(&format!("   ❌ Failed to update: {}", basename(file)));
            }
        }
    }
}

/// `mark_unresolved_in_file`: `[[link]]` → `?[[link]]` for every link whose
/// target does not exist, using the placeholder dance so `?[[x]]` is never
/// double-marked. The file is saved whenever any link was judged missing —
/// even if the bytes did not change (e.g. the only occurrence was already
/// marked), as the oracle does.
pub fn mark_unresolved_in_file(ctx: &Ctx, file_path: &Path, links: &[String], out: &mut Outcome) {
    let Some(mut content) = wiki::read_text(file_path) else { return };
    let mut marked_count = 0;

    for link in links {
        let clean = wiki::clean_link(link);
        // Dead code in the oracle too: `link` is capture group 1, which never
        // carries the prefix. Kept for fidelity. ORACLE BUG: as a result
        // `>[[inbox]]` becomes `>?[[inbox]]` and `![[img.png]]` becomes `!?[[img.png]]`.
        if link.starts_with('?') || link.starts_with('>') {
            continue;
        }
        if should_exclude_link(clean) {
            continue;
        }
        if wiki::find_target_file(clean, &ctx.roots).is_none() {
            let protected = content.replace("?[[", PLACEHOLDER);
            let marked = protected.replace(&format!("[[{link}]]"), &format!("?[[{link}]]"));
            content = marked.replace(PLACEHOLDER, "?[[");
            marked_count += 1;
        }
    }

    if marked_count > 0 {
        wiki::mark_writing(&ctx.marker, "resolve-mark");
        match wiki::save(file_path, &content) {
            Ok(()) => {
                out.wrote();
                ctx.log(&format!("   ⚠️ Marked {marked_count} new unresolved links"));
            }
            Err(e) => out.fail(format!("save {}: {e}", file_path.display())),
        }
    }
}

/// `clean_resolved_links`: strip `?` from `?[[name]]` in files found by the
/// (broken, see module docs) rg pattern; the replacement itself is `sd`'s
/// correctly-escaped regex `\?\[\[name\]\]` → `[[name]]`.
///
/// ORACLE BUG (replicated): `let updated = ($content | sd …)` collects an
/// external command's output, and Nushell strips exactly one trailing line
/// ending (`\n` or `\r\n`) when it does so — every file this path saves
/// loses its final newline. (Verified on nu 0.106.1; the other handlers use
/// nu-native `str replace` and are unaffected.)
pub fn clean_resolved_links(ctx: &Ctx, file_name: &str, out: &mut Outcome) {
    let mut affected = Vec::new();
    for dir in ctx.existing_roots() {
        affected.extend(wiki::rg_files(&format!(r"\\?\\[\\[{file_name}\\]\\]"), &dir));
    }
    if affected.is_empty() {
        return;
    }
    ctx.log(&format!("   🧹 Cleaning ?[[ markers in {} files", affected.len()));

    for file in &affected {
        let saved = wiki::read_text(file).and_then(|content| {
            let re = Regex::new(&format!(r"\?\[\[{file_name}\]\]")).ok()?;
            let replaced = re.replace_all(&content, format!("[[{file_name}]]").as_str());
            let updated = strip_one_trailing_newline(&replaced);
            wiki::mark_writing(&ctx.marker, "resolve-mark");
            wiki::save(file, updated).ok()
        });
        match saved {
            Some(()) => {
                out.wrote();
                ctx.log(&format!("   ✅ Cleaned: {}", basename(file)));
            }
            None => {
                out.fail(format!("clean: failed to update {}", file.display()));
                ctx.log(&format!("   ❌ Failed to clean: {}", basename(file)));
            }
        }
    }
}

/// What nu does to captured external output: drop one trailing `\r\n` or `\n`.
fn strip_one_trailing_newline(s: &str) -> &str {
    s.strip_suffix("\r\n").or_else(|| s.strip_suffix('\n')).unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nu_captured_output_loses_one_line_ending() {
        assert_eq!(strip_one_trailing_newline("abc\n"), "abc");
        assert_eq!(strip_one_trailing_newline("abc\n\n"), "abc\n");
        assert_eq!(strip_one_trailing_newline("abc\r\n"), "abc");
        assert_eq!(strip_one_trailing_newline("abc"), "abc");
        assert_eq!(strip_one_trailing_newline("abc\r"), "abc\r");
    }

    #[test]
    fn smart_filter() {
        for s in ["a", "https://x", "mailto:me", "C:\\x", "~/y", "IMG_0001", "my_temp_file", "Screenshot 2026", "con", "lpt1", "0123456789abcdef0123456789abcdef", "attachments/x", "a:b", "what?", "a|b", &"x".repeat(101)] {
            assert!(should_exclude_link(s), "{s} should be excluded");
        }
        for s in ["ab", "Real Note", "Foo (bar)", "2026-09-02", "CON", "é1", &"x".repeat(100)] {
            assert!(!should_exclude_link(s), "{s} should NOT be excluded");
        }
        // nu `str length` counts bytes: "é" is 2 bytes so it passes min_length 2.
        assert!(!should_exclude_link("é"));
    }
}
