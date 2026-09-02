//! Shared primitives: the oracle's `rg`/`fd`/`sd` calls emulated exactly,
//! link extraction, the feedback-loop marker file, and the handler context.
//!
//! Emulation notes (each verified against the installed rg 14.1.1 / fd 10.2.0):
//! * `rg -l <pattern> <dir> --glob "*.md"` → [`rg_files`]: regex-crate bytes
//!   regex (rg's own engine), `ignore`-crate walk (rg's own walker: hidden
//!   files and gitignored files skipped, `.ignore`/`.rgignore` honoured),
//!   files only, basename ends with `.md`. rg's parallel walk yields files in
//!   a nondeterministic order; we yield `--sort=path` order (the parity tests
//!   pin the oracle to the same order via `RIPGREP_CONFIG_PATH`).
//! * `fd -t f "^<link>.md$" <dir> | first` → [`find_target_file`]: the link
//!   name is interpolated into the regex UNESCAPED (so `.` in `.md` is a
//!   wildcard and `(`, `+`, `?` in note names are regex operators — ORACLE
//!   BUG, replicated) with fd's smart case (case-insensitive unless the
//!   pattern has an uppercase char), matched against the basename.

use crate::logger::Logger;
use ignore::WalkBuilder;
use regex::bytes::{Regex, RegexBuilder};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Written by whichever watcher is about to save a file; the other watcher
/// (and this one) skip events for [`MARKER_MAX_AGE_SECS`] afterwards.
pub const DEFAULT_MARKER_FILE: &str = "/tmp/wiki-watcher-writing";
pub const MARKER_MAX_AGE_SECS: f64 = 5.0;
/// nu `500kb` = 500,000 bytes.
pub const LARGE_FILE_BYTES: u64 = 500_000;
pub const MAX_LINKS: usize = 100;

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// The oracle's `watch_paths`: `~/Forge`, `~/Admin`, `~/Archives` — those that
/// exist. `~/Assistants` is deliberately NOT included (the watchers exclude it
/// even though `link-service` lists it in its banner).
pub fn default_roots() -> Vec<PathBuf> {
    let h = home();
    ["Forge", "Admin", "Archives"].iter().map(|d| h.join(d)).filter(|p| p.exists()).collect()
}

/// Handler context: scanned roots (first = watched), marker file, logger.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub roots: Vec<PathBuf>,
    pub marker: PathBuf,
    pub logger: Logger,
}

impl Ctx {
    pub fn log(&self, msg: &str) {
        self.logger.log(msg);
    }
    /// `$watch_paths | where {|p| $p | path exists}` — re-evaluated per call, as the oracle does.
    pub fn existing_roots(&self) -> Vec<PathBuf> {
        self.roots.iter().filter(|p| p.exists()).cloned().collect()
    }
}

/// Result of handling one event.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// Files rewritten (saves performed, whether or not the bytes changed).
    pub actions: usize,
    /// Last failure, for the heartbeat's `last_error`.
    pub error: Option<String>,
}

impl Outcome {
    pub fn wrote(&mut self) {
        self.actions += 1;
    }
    pub fn fail(&mut self, msg: String) {
        self.error = Some(msg);
    }
}

// ── marker file ─────────────────────────────────────────────────────
/// `should_skip_event`: marker exists and is younger than 5 s.
pub fn should_skip_event(marker: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(marker) else { return false };
    let Ok(modified) = meta.modified() else { return false };
    // nu: (now - modified) as seconds; a future mtime gives a negative age → skip.
    let age = match SystemTime::now().duration_since(modified) {
        Ok(d) => d.as_secs_f64(),
        Err(_) => 0.0,
    };
    age < MARKER_MAX_AGE_SECS
}

/// `mark_writing`: `"<who>" | save -f $MARKER_FILE`.
pub fn mark_writing(marker: &Path, who: &str) {
    let _ = std::fs::write(marker, who);
}

// ── names, sizes, content ───────────────────────────────────────────
/// `$path | path basename | str replace '.md' ''` — first occurrence only.
pub fn note_name(path: &Path) -> String {
    basename(path).replacen(".md", "", 1)
}

pub fn basename(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

/// `ls $path | first | get size`, 0 on error.
pub fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// nu `open` of a markdown file as text; `None` where the oracle's `try` would catch.
pub fn read_text(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// nu `save -f`: the string's bytes, nothing appended.
pub fn save(path: &Path, content: &str) -> std::io::Result<()> {
    std::fs::write(path, content)
}

/// `open $f | rg -o '<re>' --replace '$1' | lines | uniq` — group 1 of every
/// match in order, distinct. rg matches line-by-line; the callers' patterns
/// use `[^\]\n]+` so whole-content matching is equivalent.
pub fn extract_links(content: &[u8], re: &Regex) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for cap in re.captures_iter(content) {
        if let Some(m) = cap.get(1) {
            let s = String::from_utf8_lossy(m.as_bytes()).into_owned();
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out
}

/// `$link | str replace -r '[#|].*' ''` — strip from the first `#` or `|`.
pub fn clean_link(link: &str) -> &str {
    match link.find(['#', '|']) {
        Some(i) => &link[..i],
        None => link,
    }
}

// ── rg / fd emulation ───────────────────────────────────────────────
fn walk_files(dir: &Path, custom_ignore: &str) -> impl Iterator<Item = ignore::DirEntry> {
    let mut b = WalkBuilder::new(dir);
    b.sort_by_file_path(|a, b| a.cmp(b)).add_custom_ignore_filename(custom_ignore);
    b.build().filter_map(Result::ok).filter(|e| e.file_type().is_some_and(|t| t.is_file()))
}

/// `^rg -l <pattern> <dir> --glob "*.md" | lines` for one directory.
/// An invalid pattern is what the oracle's `try` catches → empty list.
pub fn rg_files(pattern: &str, dir: &Path) -> Vec<PathBuf> {
    let Ok(re) = RegexBuilder::new(pattern).multi_line(true).build() else { return Vec::new() };
    walk_files(dir, ".rgignore")
        .filter(|e| e.file_name().to_string_lossy().ends_with(".md"))
        .filter(|e| std::fs::read(e.path()).map(|b| re.is_match(&b)).unwrap_or(false))
        .map(|e| e.into_path())
        .collect()
}

/// `find_target_file`: for each existing root, `^fd -t f "^<link>.md$" <dir> | lines | first`.
pub fn find_target_file(link: &str, roots: &[PathBuf]) -> Option<PathBuf> {
    let pattern = format!("^{link}.md$");
    // fd smart case: sensitive iff the pattern contains an uppercase char.
    let case_sensitive = pattern.chars().any(char::is_uppercase);
    for dir in roots.iter().filter(|p| p.exists()) {
        let Ok(re) = RegexBuilder::new(&pattern).case_insensitive(!case_sensitive).dot_matches_new_line(true).build() else {
            continue; // fd rejects the regex → `try` catches → next dir
        };
        if let Some(e) = walk_files(dir, ".fdignore").find(|e| re.is_match(e.file_name().as_encoded_bytes())) {
            return Some(e.into_path());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn re(p: &str) -> Regex {
        Regex::new(p).unwrap()
    }

    #[test]
    fn names_and_links() {
        assert_eq!(note_name(Path::new("/x/Note.md")), "Note");
        assert_eq!(note_name(Path::new("/x/a.md.md")), "a.md");
        assert_eq!(note_name(Path::new("/x/a.mdx.md")), "ax.md");
        assert_eq!(clean_link("Note#Heading"), "Note");
        assert_eq!(clean_link("Note|alias"), "Note");
        assert_eq!(clean_link("Note"), "Note");
        let links = extract_links(b"[[a]] ?[[b|x]] ![[img.png]] [[a]] [[[c]]] [[d\ne]] >[[in]]", &re(r"[!?>]?\[\[([^\]\n]+)\]\]"));
        assert_eq!(links, vec!["a", "b|x", "img.png", "[c", "in"]);
    }

    #[test]
    fn rg_emulation_sorted_hidden_and_invalid_pattern() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("sub")).unwrap();
        std::fs::create_dir_all(d.path().join(".hidden")).unwrap();
        std::fs::write(d.path().join("z.md"), "[[T]]").unwrap();
        std::fs::write(d.path().join("a.md"), "x [[T]] y").unwrap();
        std::fs::write(d.path().join("sub/m.md"), "[[T]]").unwrap();
        std::fs::write(d.path().join("n.txt"), "[[T]]").unwrap();
        std::fs::write(d.path().join(".hidden/h.md"), "[[T]]").unwrap();
        std::fs::write(d.path().join("alias.md"), "[[T|x]] ?[[U]]").unwrap();
        let got: Vec<String> = rg_files(r"\[\[T\]\]", d.path()).iter().map(|p| p.strip_prefix(d.path()).unwrap().display().to_string()).collect();
        assert_eq!(got, vec!["a.md", "sub/m.md", "z.md"]);
        assert!(rg_files(r"\[\[T", d.path()).len() == 4); // unanchored: alias too
        assert!(rg_files(r"\\[\\[T\\]\\]", d.path()).is_empty()); // the resolve-mark double-escape
        assert!(rg_files(r"[[T", d.path()).is_empty()); // invalid regex → catch → []
    }

    #[test]
    fn fd_emulation_smart_case_and_regex_metachars() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("Forge");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("Note.md"), "").unwrap();
        std::fs::write(root.join("sub/Deep.md"), "").unwrap();
        std::fs::write(root.join("Foo (bar).md"), "").unwrap();
        std::fs::write(root.join("Foo bar.md"), "").unwrap();
        std::fs::write(root.join("Notexmd"), "").unwrap();
        let roots = vec![root.clone(), d.path().join("Missing")];
        assert_eq!(find_target_file("Note", &roots), Some(root.join("Note.md")));
        assert_eq!(find_target_file("note", &roots), Some(root.join("Note.md"))); // smart case
        assert_eq!(find_target_file("deep", &roots), Some(root.join("sub/Deep.md")));
        assert_eq!(find_target_file("Foo (bar)", &roots), Some(root.join("Foo bar.md"))); // ORACLE BUG: parens are a regex group
        assert_eq!(find_target_file("Nope", &roots), None);
        assert_eq!(find_target_file("C++", &roots), None); // invalid regex → not found
    }

    #[test]
    fn marker_age() {
        let d = tempfile::tempdir().unwrap();
        let m = d.path().join("marker");
        assert!(!should_skip_event(&m));
        mark_writing(&m, "backlinks");
        assert!(should_skip_event(&m));
        assert_eq!(std::fs::read_to_string(&m).unwrap(), "backlinks");
    }
}
