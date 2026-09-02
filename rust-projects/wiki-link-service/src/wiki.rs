//! Shared primitives: the note index (case-insensitive resolution), wiki-link
//! parsing, `## Backlinks` section handling, and per-watcher self-write
//! suppression.
//!
//! SPEC (0.2.0):
//! * bug 7 — names are never interpolated into regexes unescaped; `[[foo]]`,
//!   `[[Foo]]` and `[[C++]]` all resolve by case-insensitive file stem.
//! * bug 3 — `outgoing_names` never reads links out of the `## Backlinks` section.
//! * bug 5 — `with_section` replaces exactly the section (heading → next
//!   heading of any level or EOF) and preserves everything else byte-for-byte;
//!   an empty list removes the section.
//! * bug 2 — the file's trailing-newline state is preserved.
//! * bug 9 — `Ctx::is_own_write`: an event is skipped only when this watcher
//!   wrote that path and the bytes on disk still match what it wrote.

use crate::logger::Logger;
use ignore::WalkBuilder;
use regex::Regex;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

/// nu `500kb` = 500,000 bytes.
pub const LARGE_FILE_BYTES: u64 = 500_000;
pub const MAX_LINKS: usize = 100;

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// `~/Forge`, `~/Admin`, `~/Archives` — those that exist. `~/Assistants` is
/// deliberately NOT included (as in the oracles).
pub fn default_roots() -> Vec<PathBuf> {
    let h = home();
    ["Forge", "Admin", "Archives"].iter().map(|d| h.join(d)).filter(|p| p.exists()).collect()
}

// ── context ─────────────────────────────────────────────────────────
/// Per-watcher context: scanned roots (first = watched), logger, and the
/// record of this watcher's own writes.
#[derive(Debug)]
pub struct Ctx {
    pub roots: Vec<PathBuf>,
    pub logger: Logger,
    self_writes: Mutex<HashMap<PathBuf, u64>>,
}

/// The path as the OS reports it in events (macOS FSEvents gives
/// `/private/var/…` for a `/var/…` tempdir): canonicalised as far as it
/// exists, so a deleted file keeps its (canonical) parent plus name.
pub fn canon(path: &Path) -> PathBuf {
    if let Ok(p) = path.canonicalize() {
        return p;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => canon(parent).join(name),
        _ => path.to_path_buf(),
    }
}

impl Ctx {
    /// Roots are canonicalised so index paths compare equal to event paths.
    pub fn new(roots: Vec<PathBuf>, logger: Logger) -> Self {
        let roots = roots.iter().map(|r| if r.exists() { canon(r) } else { r.clone() }).collect();
        Ctx { roots, logger, self_writes: Mutex::new(HashMap::new()) }
    }
    pub fn log(&self, msg: &str) {
        self.logger.log(msg);
    }
    pub fn existing_roots(&self) -> Vec<PathBuf> {
        self.roots.iter().filter(|p| p.exists()).cloned().collect()
    }
    /// Remember that this watcher wrote `content` to `path`.
    pub fn record_write(&self, path: &Path, content: &str) {
        self.self_writes.lock().unwrap().insert(path.to_path_buf(), hash_bytes(content.as_bytes()));
    }
    /// True iff this watcher last wrote `path` and the bytes on disk still
    /// match. Anything else (never written by us, changed since, deleted)
    /// is a real event and forgets the record.
    pub fn is_own_write(&self, path: &Path) -> bool {
        let mut map = self.self_writes.lock().unwrap();
        let Some(&recorded) = map.get(path) else { return false };
        match std::fs::read(path) {
            Ok(bytes) if hash_bytes(&bytes) == recorded => true,
            _ => {
                map.remove(path);
                false
            }
        }
    }
}

fn hash_bytes(b: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}

/// Result of handling one event.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// Files rewritten.
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

// ── names, files ────────────────────────────────────────────────────
pub fn basename(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

/// The note's name: its file stem.
pub fn note_name(path: &Path) -> String {
    path.file_stem().unwrap_or_default().to_string_lossy().into_owned()
}

/// Case-insensitive, whitespace-trimmed key a link name resolves by. A trailing
/// `.md` is dropped: 787 Forge notes (Readwise/Obsidian era) link as
/// `[[Note.md|Note]]`, and the index is keyed by file stem.
pub fn name_key(name: &str) -> String {
    let n = name.trim();
    // ".md" is ASCII, so when the ASCII-lowercased copy ends with it the last three BYTES of `n`
    // are exactly that suffix and the slice is on a char boundary.
    let n = if n.len() > 3 && n.to_ascii_lowercase().ends_with(".md") { &n[..n.len() - 3] } else { n };
    n.trim_end().to_lowercase()
}

pub fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

pub fn read_text(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

pub fn save(path: &Path, content: &str) -> std::io::Result<()> {
    std::fs::write(path, content)
}

/// Sort backlink entry names case-insensitively (then bytewise) and dedup.
pub fn sort_names(names: &mut Vec<String>) {
    names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b)));
    names.dedup();
}

// ── links ───────────────────────────────────────────────────────────
/// One wiki-link occurrence: `flag` is `!` (embed), `>` (inbox) or empty;
/// `marks` is the run of `?` before `[[`; `inner` is everything between the
/// brackets; `name`/`suffix` split `inner` at the first `|` or `#`.
pub const LINK_RE: &str = r"([!>]?)(\?*)\[\[([^\]\n]+)\]\]";

pub fn link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(LINK_RE).expect("static regex"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link<'a> {
    pub flag: &'a str,
    pub marks: &'a str,
    pub inner: &'a str,
    pub name: &'a str,
    pub suffix: &'a str,
}

/// `Note|alias` → (`Note`, `|alias`); `Note#h` → (`Note`, `#h`).
pub fn split_inner(inner: &str) -> (&str, &str) {
    match inner.find(['|', '#']) {
        Some(i) => (&inner[..i], &inner[i..]),
        None => (inner, ""),
    }
}

pub fn links_in(text: &str) -> Vec<Link<'_>> {
    link_re()
        .captures_iter(text)
        .map(|c| {
            let inner = c.get(3).map_or("", |m| m.as_str());
            let (name, suffix) = split_inner(inner);
            Link { flag: c.get(1).map_or("", |m| m.as_str()), marks: c.get(2).map_or("", |m| m.as_str()), inner, name, suffix }
        })
        .collect()
}

/// Distinct non-empty link names in `text`, in order of first appearance.
pub fn link_names(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in links_in(text) {
        let n = l.name.trim();
        if !n.is_empty() && !out.iter().any(|o| o == n) {
            out.push(n.to_string());
        }
    }
    out
}

/// Outgoing link names of a note — everything outside its `## Backlinks` section.
pub fn outgoing_names(content: &str) -> Vec<String> {
    let (before, after) = outside_section(content);
    let mut names = link_names(before);
    for n in link_names(after) {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    names
}

// ── ## Backlinks section ────────────────────────────────────────────
/// Byte range of the section: from the start of the `## Backlinks` line to
/// the start of the next heading line (any level) or the end of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    pub start: usize,
    pub end: usize,
}

/// ATX heading: 1–6 `#` followed by whitespace or end of line.
pub fn is_heading_line(line: &str) -> bool {
    let rest = line.trim_start_matches('#');
    let hashes = line.len() - rest.len();
    (1..=6).contains(&hashes) && (rest.trim_end_matches('\r').is_empty() || rest.starts_with(' ') || rest.starts_with('\t'))
}

pub fn find_section(content: &str) -> Option<Section> {
    let mut offset = 0;
    let mut start = None;
    for line in content.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        match start {
            None => {
                if text.trim_end() == "## Backlinks" {
                    start = Some(offset);
                }
            }
            Some(s) => {
                if is_heading_line(text) {
                    return Some(Section { start: s, end: offset });
                }
            }
        }
        offset += line.len();
    }
    start.map(|s| Section { start: s, end: content.len() })
}

/// The note's text outside its section: (before, after). No section → (content, "").
pub fn outside_section(content: &str) -> (&str, &str) {
    match find_section(content) {
        Some(s) => (&content[..s.start], &content[s.end..]),
        None => (content, ""),
    }
}

/// Names currently listed in the section.
pub fn section_entries(content: &str) -> Vec<String> {
    find_section(content).map(|s| link_names(&content[s.start..s.end])).unwrap_or_default()
}

/// The note with its `## Backlinks` section set to exactly `names`
/// (`- [[name]]` lines). Only the section changes; an empty list removes it.
/// The file's final newline is present afterwards iff it was before.
pub fn with_section(content: &str, names: &[String]) -> String {
    let final_nl = content.ends_with('\n');
    let entries = names.iter().map(|n| format!("- [[{n}]]")).collect::<Vec<_>>().join("\n");
    match find_section(content) {
        Some(s) => {
            let before = &content[..s.start];
            let after = &content[s.end..];
            if names.is_empty() {
                if !after.is_empty() {
                    return format!("{before}{after}");
                }
                let trimmed = before.trim_end_matches('\n');
                if trimmed.is_empty() {
                    return content.to_string(); // the note is nothing but a section: leave it
                }
                format!("{trimmed}{}", if final_nl { "\n" } else { "" })
            } else {
                let tail = if after.is_empty() {
                    if final_nl {
                        "\n"
                    } else {
                        ""
                    }
                } else {
                    "\n\n"
                };
                format!("{before}## Backlinks\n\n{entries}{tail}{after}")
            }
        }
        None => {
            if names.is_empty() {
                return content.to_string();
            }
            if content.is_empty() {
                return format!("## Backlinks\n\n{entries}\n");
            }
            let sep = if content.ends_with("\n\n") {
                ""
            } else if final_nl {
                "\n"
            } else {
                "\n\n"
            };
            format!("{content}{sep}## Backlinks\n\n{entries}{}", if final_nl { "\n" } else { "" })
        }
    }
}

// ── index ───────────────────────────────────────────────────────────
/// All `.md` notes under the roots (hidden and gitignored files skipped, as
/// rg/fd did), in root order then sorted path order, with case-insensitive
/// name resolution and per-event content/link caches. Built once per event.
pub struct Index {
    files: Vec<PathBuf>,
    by_key: HashMap<String, usize>,
    pos: HashMap<PathBuf, usize>,
    contents: RefCell<HashMap<usize, Option<Rc<str>>>>,
    outgoing: RefCell<HashMap<usize, Rc<Vec<String>>>>,
    reverse: RefCell<Option<Rc<ReverseMap>>>,
}

/// target index → indices of the notes linking to it.
type ReverseMap = HashMap<usize, Vec<usize>>;

impl Index {
    pub fn build(roots: &[PathBuf]) -> Index {
        let mut files = Vec::new();
        for root in roots.iter().filter(|r| r.exists()) {
            let mut b = WalkBuilder::new(root);
            b.sort_by_file_path(|a, b| a.cmp(b));
            for e in b.build().filter_map(Result::ok) {
                if e.file_type().is_some_and(|t| t.is_file()) && e.path().extension().is_some_and(|x| x == "md") {
                    files.push(e.into_path());
                }
            }
        }
        let mut by_key = HashMap::new();
        let mut pos = HashMap::new();
        for (i, f) in files.iter().enumerate() {
            by_key.entry(name_key(&note_name(f))).or_insert(i);
            pos.insert(f.clone(), i);
        }
        Index { files, by_key, pos, contents: RefCell::default(), outgoing: RefCell::default(), reverse: RefCell::default() }
    }

    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }
    pub fn position(&self, path: &Path) -> Option<usize> {
        self.pos.get(path).copied()
    }
    /// The note a link name refers to: first note (root order, sorted) whose stem matches case-insensitively.
    pub fn resolve_idx(&self, name: &str) -> Option<usize> {
        self.by_key.get(&name_key(name)).copied()
    }
    pub fn resolve(&self, name: &str) -> Option<&Path> {
        self.resolve_idx(name).map(|i| self.files[i].as_path())
    }

    pub fn content(&self, i: usize) -> Option<Rc<str>> {
        if let Some(c) = self.contents.borrow().get(&i) {
            return c.clone();
        }
        let c = read_text(&self.files[i]).map(Rc::from);
        self.contents.borrow_mut().insert(i, c.clone());
        c
    }
    pub fn content_of(&self, path: &Path) -> Option<Rc<str>> {
        match self.position(path) {
            Some(i) => self.content(i),
            None => read_text(path).map(Rc::from),
        }
    }
    /// Outgoing link names of note `i` (outside its section).
    pub fn outgoing(&self, i: usize) -> Rc<Vec<String>> {
        if let Some(o) = self.outgoing.borrow().get(&i) {
            return o.clone();
        }
        let o = Rc::new(self.content(i).map(|c| outgoing_names(&c)).unwrap_or_default());
        self.outgoing.borrow_mut().insert(i, o.clone());
        o
    }

    fn reverse_map(&self) -> Rc<ReverseMap> {
        if let Some(r) = self.reverse.borrow().as_ref() {
            return r.clone();
        }
        let mut map: ReverseMap = HashMap::new();
        for i in 0..self.files.len() {
            for name in self.outgoing(i).iter() {
                if let Some(t) = self.resolve_idx(name) {
                    let v = map.entry(t).or_default();
                    if !v.contains(&i) {
                        v.push(i);
                    }
                }
            }
        }
        let r = Rc::new(map);
        *self.reverse.borrow_mut() = Some(r.clone());
        r
    }

    /// Sorted, distinct names of the notes whose outgoing links resolve to `target` (never the target itself).
    pub fn backlink_names(&self, target: usize) -> Vec<String> {
        let map = self.reverse_map();
        let mut names: Vec<String> = map.get(&target).map(|v| v.iter().filter(|&&i| i != target).map(|&i| note_name(&self.files[i])).collect()).unwrap_or_default();
        sort_names(&mut names);
        names
    }

    /// Save `new` to `path`, record it as this watcher's own write, refresh the caches.
    pub fn write(&self, ctx: &Ctx, path: &Path, new: &str) -> std::io::Result<()> {
        save(path, new)?;
        ctx.record_write(path, new);
        if let Some(i) = self.position(path) {
            let old_out = self.outgoing(i);
            self.contents.borrow_mut().insert(i, Some(Rc::from(new)));
            self.outgoing.borrow_mut().remove(&i);
            if *self.outgoing(i) != *old_out {
                *self.reverse.borrow_mut() = None;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_key_strips_md_suffix_and_case() {
        assert_eq!(name_key("ASD and Initiative.md"), "asd and initiative");
        assert_eq!(name_key("Note.MD"), "note");
        assert_eq!(name_key(" Foo (bar) "), "foo (bar)");
        assert_eq!(name_key(".md"), ".md");
        assert_eq!(name_key("Is ‘relating’ in RFT swappable with ‘comparing’_.md"), "is ‘relating’ in rft swappable with ‘comparing’_");
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn link_parsing() {
        let ls = links_in("[[a]] ?[[b|x]] ![[img.png]] >??[[in#h]] [[[c]]] [[d\ne]]");
        let v: Vec<(&str, &str, &str, &str)> = ls.iter().map(|l| (l.flag, l.marks, l.name, l.suffix)).collect();
        assert_eq!(v, vec![("", "", "a", ""), ("", "?", "b", "|x"), ("!", "", "img.png", ""), (">", "??", "in", "#h"), ("", "", "[c", "")]);
        assert_eq!(link_names("[[a]] [[ a ]] [[b|x]] [[b#y]] [[|z]]"), names(&["a", "b"]));
        assert_eq!(note_name(Path::new("/x/Note.md")), "Note");
        assert_eq!(note_name(Path::new("/x/a.md.md")), "a.md");
        assert_eq!(name_key(" Foo Bar "), "foo bar");
    }

    #[test]
    fn section_detection_and_outgoing() {
        let c = "# T\n\n[[A]]\n\n## Backlinks\n\n- [[B]]\n- [[C|x]]\n\n### Sub\n\n[[D]]\n";
        let s = find_section(c).unwrap();
        assert_eq!(&c[s.start..s.end], "## Backlinks\n\n- [[B]]\n- [[C|x]]\n\n");
        assert_eq!(section_entries(c), names(&["B", "C"]));
        assert_eq!(outgoing_names(c), names(&["A", "D"]));
        assert_eq!(find_section("## Backlinks\n"), Some(Section { start: 0, end: 13 }));
        assert_eq!(find_section("### Backlinks\n- [[B]]\n"), None);
        assert_eq!(find_section("x ## Backlinks\n"), None);
        assert!(is_heading_line("#"));
        assert!(is_heading_line("###### x"));
        assert!(!is_heading_line("####### x"));
        assert!(!is_heading_line("#hashtag"));
    }

    #[test]
    fn with_section_shapes() {
        let b = names(&["B"]);
        // append: final newline preserved, one blank line before the heading
        assert_eq!(with_section("# T\n\nbody\n", &b), "# T\n\nbody\n\n## Backlinks\n\n- [[B]]\n");
        assert_eq!(with_section("# T\n\nbody\n\n", &b), "# T\n\nbody\n\n## Backlinks\n\n- [[B]]\n");
        assert_eq!(with_section("# T\n\nbody", &b), "# T\n\nbody\n\n## Backlinks\n\n- [[B]]");
        assert_eq!(with_section("", &b), "## Backlinks\n\n- [[B]]\n");
        assert_eq!(with_section("# T\n", &[]), "# T\n");
        // replace only the section; everything else byte-for-byte
        let mid = "\n  # T\n\nbody\n\n## Backlinks\n\n- [[A]]\n## Next\n\nkept\n";
        assert_eq!(with_section(mid, &b), "\n  # T\n\nbody\n\n## Backlinks\n\n- [[B]]\n\n## Next\n\nkept\n");
        assert_eq!(with_section(mid, &[]), "\n  # T\n\nbody\n\n## Next\n\nkept\n");
        let end = "# T\n\nbody\n\n## Backlinks\n\n- [[A]]\n\n";
        assert_eq!(with_section(end, &b), "# T\n\nbody\n\n## Backlinks\n\n- [[B]]\n");
        assert_eq!(with_section(end, &[]), "# T\n\nbody\n");
        assert_eq!(with_section("# T\n\nbody\n\n## Backlinks\n\n- [[A]]", &b), "# T\n\nbody\n\n## Backlinks\n\n- [[B]]");
        assert_eq!(with_section("# T\n\nbody\n\n## Backlinks\n\n- [[A]]", &[]), "# T\n\nbody");
        assert_eq!(with_section("## Backlinks\n\n- [[A]]\n", &[]), "## Backlinks\n\n- [[A]]\n");
        // idempotent
        let once = with_section(mid, &b);
        assert_eq!(with_section(&once, &b), once);
    }

    #[test]
    fn index_resolution_and_backlinks() {
        let d = tempfile::tempdir().unwrap();
        let (forge, admin) = (d.path().join("Forge"), d.path().join("Admin"));
        std::fs::create_dir_all(forge.join("sub")).unwrap();
        std::fs::create_dir_all(forge.join(".hidden")).unwrap();
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(forge.join("Note.md"), "[[deep]] [[C++]] [[Foo (bar)|x]]\n\n## Backlinks\n\n- [[Deep]]\n").unwrap();
        std::fs::write(forge.join("sub/Deep.md"), "[[note]] [[Note#h]]").unwrap();
        std::fs::write(forge.join("C++.md"), "").unwrap();
        std::fs::write(forge.join("Foo (bar).md"), "").unwrap();
        std::fs::write(forge.join(".hidden/H.md"), "[[Note]]").unwrap();
        std::fs::write(forge.join("Notes.txt"), "[[Note]]").unwrap();
        std::fs::write(admin.join("Note.md"), "[[note]]").unwrap();
        let ix = Index::build(&[forge.clone(), admin.clone(), d.path().join("Missing")]);
        let rel: Vec<String> = ix.files().iter().map(|p| p.strip_prefix(d.path()).unwrap().display().to_string()).collect();
        assert_eq!(rel, vec!["Forge/C++.md", "Forge/Foo (bar).md", "Forge/Note.md", "Forge/sub/Deep.md", "Admin/Note.md"]);
        assert_eq!(ix.resolve("note"), Some(forge.join("Note.md").as_path()));
        assert_eq!(ix.resolve("DEEP"), Some(forge.join("sub/Deep.md").as_path()));
        assert_eq!(ix.resolve("c++"), Some(forge.join("C++.md").as_path()));
        assert_eq!(ix.resolve("foo (bar)"), Some(forge.join("Foo (bar).md").as_path()));
        assert_eq!(ix.resolve("nope"), None);
        let note = ix.position(&forge.join("Note.md")).unwrap();
        assert_eq!(*ix.outgoing(note), names(&["deep", "C++", "Foo (bar)"]));
        assert_eq!(ix.backlink_names(note), names(&["Deep", "Note"])); // Admin/Note links [[note]] → Forge/Note; self excluded
        assert_eq!(ix.backlink_names(ix.position(&forge.join("sub/Deep.md")).unwrap()), names(&["Note"]));
        assert_eq!(ix.backlink_names(ix.position(&admin.join("Note.md")).unwrap()), Vec::<String>::new());
    }

    #[test]
    fn self_write_suppression() {
        let d = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(vec![d.path().to_path_buf()], Logger::silent());
        let p = d.path().join("a.md");
        std::fs::write(&p, "x").unwrap();
        assert!(!ctx.is_own_write(&p));
        ctx.record_write(&p, "x");
        assert!(ctx.is_own_write(&p));
        assert!(ctx.is_own_write(&p), "record survives while bytes match");
        std::fs::write(&p, "y").unwrap();
        assert!(!ctx.is_own_write(&p));
        assert!(!ctx.is_own_write(&p), "record forgotten after a real change");
        ctx.record_write(&p, "y");
        std::fs::remove_file(&p).unwrap();
        assert!(!ctx.is_own_write(&p));
    }
}
