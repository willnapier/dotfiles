//! forge-names — the **path→name boundary** shared by every tool that turns a
//! file under a synced tree (Forge, Admin, Archives, Clinical, a photo vault)
//! into a name, key, ID, regex or link.
//!
//! macOS lists many file names in Unicode NFD (`e` + U+0308) while Linux
//! stores and lists NFC (`ë`), and Syncthing delivers NFC to Linux. Text that
//! people type is NFC. So a name taken off the filesystem and compared with
//! text from anywhere else disagrees across hosts unless it is normalised.
//!
//! The rule (design forum `wls-nfc-boundary-vs-site-patches`, 2026-09-02, and
//! the roll-out map `meta-nfc-boundary-rollout`):
//!
//! 1. **`Path`/`PathBuf` are opaque I/O identities.** Open, rename and delete
//!    through the path the OS gave you. Never build a path from an NFC name.
//! 2. **Every name derived from a path is NFC at the boundary** — one function
//!    per codebase, not `.nfc()` at each comparison. These are those functions.
//! 3. **Text is matched in both spellings** ([`name_pattern`]): older notes,
//!    Finder paste and non-normalising editors leave NFD in note bodies.
//! 4. To go from a typed name back to a file, **look it up** in a listing
//!    ([`find_in_dir`], [`find_note`]) and use the entry's own path.

use regex::escape;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

/// NFC-normalise any text.
pub fn nfc(s: &str) -> String {
    s.nfc().collect()
}

/// A case-insensitive comparison key: Unicode-lowercased, then NFC — in that
/// order, so the *final* key carries the invariant.
pub fn nfc_key(s: &str) -> String {
    s.to_lowercase().nfc().collect()
}

/// The file name as an NFC string (extension included).
pub fn file_name(path: &Path) -> String {
    nfc(&path.file_name().unwrap_or_default().to_string_lossy())
}

/// The note's name: its file stem, NFC. The boundary for stems.
pub fn note_name(path: &Path) -> String {
    nfc(&path.file_stem().unwrap_or_default().to_string_lossy())
}

/// The key a root-relative path (without extension) is indexed and looked up
/// by: `\` → `/`, Unicode-lowercased, then NFC. Use it on BOTH the insertion
/// side (from a walk) and the lookup side (from typed `[[Dir/Name]]` text).
pub fn rel_key(rel: &str) -> String {
    rel.replace('\\', "/").to_lowercase().nfc().collect()
}

/// A regex fragment matching `name` in either its NFC or NFD spelling, for
/// scanning *text* (never file names). Collapses to one escaped literal when
/// the two forms are identical (ASCII).
pub fn name_pattern(name: &str) -> String {
    let c: String = name.nfc().collect();
    let d: String = name.nfd().collect();
    if c == d {
        escape(&c)
    } else {
        format!("(?:{}|{})", escape(&c), escape(&d))
    }
}

/// Resolve a typed file name (with extension) to an existing entry of `dir`,
/// comparing NFC to NFC, non-recursively. Returns the entry's own path.
pub fn find_in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    let want = nfc(name);
    std::fs::read_dir(dir).ok()?.filter_map(Result::ok).map(|e| e.path()).find(|p| file_name(p) == want)
}

/// Resolve a typed name to an existing directory entry of `dir`, comparing
/// case-insensitively (NFC-lowercased both sides). Returns the entry's own path.
pub fn find_in_dir_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    let want = nfc_key(name);
    std::fs::read_dir(dir).ok()?.filter_map(Result::ok).map(|e| e.path()).find(|p| nfc_key(&file_name(p)) == want)
}

/// Every `.md` note under `roots` whose stem (or root-relative path without
/// `.md`, when `name` contains `/`) matches `name`, compared case-insensitively
/// after NFC. A trailing `.md` on `name` is accepted. Hidden directories and
/// Syncthing conflict copies are skipped. Order: root order, then sorted path.
/// Returns the walked paths, never reconstructed ones.
pub fn find_note(roots: &[PathBuf], name: &str) -> Vec<PathBuf> {
    let n = name.trim();
    let n = n.strip_suffix(".md").or_else(|| n.strip_suffix(".MD")).unwrap_or(n);
    let want_rel = n.contains('/').then(|| rel_key(n));
    let want_stem = nfc_key(n.rsplit('/').next().unwrap_or(n));
    let mut out = Vec::new();
    for root in roots.iter().filter(|r| r.is_dir()) {
        let mut files = Vec::new();
        walk_md(root, &mut files);
        files.sort();
        for p in files {
            if let Some(want) = &want_rel {
                let rel = p.strip_prefix(root).unwrap_or(&p).with_extension("");
                if rel_key(&rel.to_string_lossy()) == *want {
                    return vec![p];
                }
            } else if nfc_key(&note_name(&p)) == want_stem {
                out.push(p);
            }
        }
    }
    if want_rel.is_some() {
        // Path-qualified miss: fall back to the stem, as wiki-link-service does.
        let stem = n.rsplit('/').next().unwrap_or(n);
        return find_note(roots, stem);
    }
    out
}

fn walk_md(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            walk_md(&p, out);
        } else if p.extension().is_some_and(|x| x == "md") && !name.contains(".sync-conflict-") {
            out.push(p);
        }
    }
}

/// Pairs of distinct paths in one listing whose NFC file names coincide (an NFC
/// and an NFD spelling side by side — two files on Linux, one on macOS).
pub fn equivalent_duplicates(paths: &[PathBuf]) -> Vec<(PathBuf, PathBuf)> {
    let mut seen: HashMap<String, &PathBuf> = HashMap::new();
    let mut dups = Vec::new();
    for p in paths {
        let key = nfc(&p.to_string_lossy());
        match seen.get(&key) {
            Some(first) => dups.push(((*first).clone(), p.clone())),
            None => {
                seen.insert(key, p);
            }
        }
    }
    dups
}

#[cfg(test)]
mod tests {
    use super::*;
    const NFD: &str = "Zoe\u{0308} Harcombe";
    const NFC: &str = "Zoë Harcombe";

    #[test]
    fn names_are_nfc_at_the_boundary() {
        assert_ne!(NFD, NFC);
        assert_eq!(note_name(Path::new(&format!("/x/{NFD}.md"))), NFC);
        assert_eq!(file_name(Path::new(&format!("/x/{NFD}.md"))), format!("{NFC}.md"));
        assert_eq!(nfc_key(NFD), "zoë harcombe");
        assert_eq!(rel_key(&format!("Zoe\u{0308}\\{NFD}")), "zoë/zoë harcombe");
    }

    #[test]
    fn pattern_matches_both_spellings() {
        for name in [NFD, NFC] {
            let re = regex::Regex::new(&format!(r"\[\[{}\]\]", name_pattern(name))).unwrap();
            assert!(re.is_match(&format!("[[{NFC}]]")), "{name:?} vs NFC text");
            assert!(re.is_match(&format!("[[{NFD}]]")), "{name:?} vs NFD text");
            assert!(!re.is_match("[[Zoe Harcombe]]"));
        }
        assert_eq!(name_pattern("Foo (bar)"), escape("Foo (bar)"));
    }

    #[test]
    fn lookup_returns_the_walked_path_for_either_spelling() {
        let d = tempfile::tempdir().unwrap();
        let forge = d.path().join("Forge");
        std::fs::create_dir_all(forge.join("Zoe\u{0308}")).unwrap();
        let on_disk = forge.join("Zoe\u{0308}").join(format!("{NFD}.md"));
        std::fs::write(&on_disk, "").unwrap();
        std::fs::write(forge.join("Other.md"), "").unwrap();
        std::fs::create_dir_all(forge.join(".hidden")).unwrap();
        std::fs::write(forge.join(".hidden").join(format!("{NFC}.md")), "").unwrap();
        std::fs::write(forge.join(format!("{NFC}.sync-conflict-20260101-000000-ABC.md")), "").unwrap();

        assert_eq!(find_in_dir(&forge.join("Zoe\u{0308}"), &format!("{NFC}.md")), Some(on_disk.clone()));
        assert_eq!(find_in_dir_ci(&forge.join("Zoe\u{0308}"), "zoë harcombe.MD"), Some(on_disk.clone()));
        assert_eq!(find_in_dir(&forge, "nope.md"), None);
        let roots = vec![forge.clone()];
        assert_eq!(find_note(&roots, NFC), vec![on_disk.clone()]);
        assert_eq!(find_note(&roots, &format!("{NFC}.md")), vec![on_disk.clone()]);
        assert_eq!(find_note(&roots, "zoë harcombe"), vec![on_disk.clone()]);
        assert_eq!(find_note(&roots, &format!("Zoë/{NFC}")), vec![on_disk.clone()], "path-qualified, NFC dir vs NFD dir");
        assert_eq!(find_note(&roots, &format!("Nowhere/{NFC}")), vec![on_disk.clone()], "unknown dir falls back to the stem");
        assert_eq!(find_note(&roots, "missing"), Vec::<PathBuf>::new());
        assert!(find_note(&roots, "Other").len() == 1);
    }

    #[test]
    fn duplicates_are_pairs_of_equivalent_names() {
        let a = PathBuf::from(format!("/x/{NFC}.md"));
        let b = PathBuf::from(format!("/x/{NFD}.md"));
        let c = PathBuf::from("/x/Other.md");
        assert_eq!(equivalent_duplicates(&[a.clone(), c, b.clone()]), vec![(a, b)]);
    }
}
