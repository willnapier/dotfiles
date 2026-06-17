use std::collections::BTreeMap;
use std::path::PathBuf;

/// Classify a file path into a (project_name, repo_root) pair.
///
/// Returns `None` if the path matches none of the known project shapes.
pub fn classify(path: &str) -> Option<(String, Option<PathBuf>)> {
    // /Code/<seg>/... → project = <seg>, repo_root = up to and including .../Code/<seg>
    if let Some((seg, root)) = segment_after(path, "/Code/") {
        return Some((seg, Some(root)));
    }

    // /rust-projects/<seg>/... → project = <seg>, repo_root = .../rust-projects/<seg>
    if let Some((seg, root)) = segment_after(path, "/rust-projects/") {
        return Some((seg, Some(root)));
    }

    // /.claude/skills/ → project = "skills", repo_root = None
    if path.contains("/.claude/skills/") {
        return Some(("skills".to_string(), None));
    }

    // /.claude/ → project = "claude", repo_root = None
    if path.contains("/.claude/") {
        return Some(("claude".to_string(), None));
    }

    // /dotfiles/ (not caught by rust-projects above) → project = "dotfiles",
    // repo_root = path up to and including .../dotfiles
    if let Some(idx) = path.find("/dotfiles/") {
        let root = PathBuf::from(&path[..idx + "/dotfiles".len()]);
        return Some(("dotfiles".to_string(), Some(root)));
    }

    // /Forge/ → project = "forge", repo_root = None
    if path.contains("/Forge/") {
        return Some(("forge".to_string(), None));
    }

    None
}

/// For a marker like "/Code/", extract the segment immediately following it and
/// build the repo root path up to and including that segment.
fn segment_after(path: &str, marker: &str) -> Option<(String, PathBuf)> {
    let idx = path.find(marker)?;
    let after = &path[idx + marker.len()..];
    let seg = after.split('/').next().unwrap_or("");
    if seg.is_empty() {
        return None;
    }
    let root_end = idx + marker.len() + seg.len();
    let root = PathBuf::from(&path[..root_end]);
    Some((seg.to_string(), root))
}

/// Determine the primary project (highest summed edit-count) across modified
/// files, the sorted-unique list of all projects, and the repo_root of the
/// primary project.
///
/// If no path classifies, primary = "misc", list = ["misc"], repo_root = None.
pub fn primary_project(files: &BTreeMap<String, u32>) -> (String, Vec<String>, Option<PathBuf>) {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    let mut roots: BTreeMap<String, Option<PathBuf>> = BTreeMap::new();

    for (path, edits) in files {
        if let Some((project, root)) = classify(path) {
            *counts.entry(project.clone()).or_insert(0) += *edits;
            // Record a repo_root for this project if we don't already have one.
            roots.entry(project).or_insert(root);
        }
    }

    if counts.is_empty() {
        return ("misc".to_string(), vec!["misc".to_string()], None);
    }

    // Primary = highest summed edit-count; ties broken by name (BTreeMap order).
    let primary = counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(name, _)| name.clone())
        .unwrap();

    let mut all: Vec<String> = counts.keys().cloned().collect();
    all.sort();
    all.dedup();

    let repo_root = roots.get(&primary).cloned().flatten();

    (primary, all, repo_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_code() {
        let (proj, root) = classify("/Users/will/Code/meli/src/main.rs").unwrap();
        assert_eq!(proj, "meli");
        assert_eq!(root, Some(PathBuf::from("/Users/will/Code/meli")));
    }

    #[test]
    fn test_classify_rust_projects() {
        let (proj, root) =
            classify("/Users/will/dotfiles/rust-projects/dev-catchup/src/main.rs").unwrap();
        assert_eq!(proj, "dev-catchup");
        assert_eq!(
            root,
            Some(PathBuf::from(
                "/Users/will/dotfiles/rust-projects/dev-catchup"
            ))
        );
    }

    #[test]
    fn test_classify_skills() {
        let (proj, root) = classify("/Users/will/.claude/skills/verify/SKILL.md").unwrap();
        assert_eq!(proj, "skills");
        assert_eq!(root, None);
    }

    #[test]
    fn test_classify_claude() {
        let (proj, root) = classify("/Users/will/.claude/settings.json").unwrap();
        assert_eq!(proj, "claude");
        assert_eq!(root, None);
    }

    #[test]
    fn test_classify_dotfiles() {
        let (proj, root) = classify("/Users/will/dotfiles/niri/config.kdl").unwrap();
        assert_eq!(proj, "dotfiles");
        assert_eq!(root, Some(PathBuf::from("/Users/will/dotfiles")));
    }

    #[test]
    fn test_classify_forge() {
        let (proj, root) = classify("/Users/will/Forge/NapierianLogs/foo.md").unwrap();
        assert_eq!(proj, "forge");
        assert_eq!(root, None);
    }

    #[test]
    fn test_classify_none() {
        assert!(classify("/Users/will/Documents/notes.txt").is_none());
    }

    #[test]
    fn test_rust_projects_takes_precedence_over_dotfiles() {
        // rust-projects lives under dotfiles, but must classify as the project, not dotfiles.
        let (proj, _root) = classify("/Users/will/dotfiles/rust-projects/foo/src/lib.rs").unwrap();
        assert_eq!(proj, "foo");
    }

    #[test]
    fn test_primary_project_picks_highest_count() {
        let mut files = BTreeMap::new();
        files.insert("/Users/will/Code/meli/a.rs".to_string(), 5);
        files.insert("/Users/will/Code/meli/b.rs".to_string(), 3);
        files.insert("/Users/will/dotfiles/x.kdl".to_string(), 2);
        let (primary, all, root) = primary_project(&files);
        assert_eq!(primary, "meli");
        assert_eq!(all, vec!["dotfiles".to_string(), "meli".to_string()]);
        assert_eq!(root, Some(PathBuf::from("/Users/will/Code/meli")));
    }

    #[test]
    fn test_primary_project_misc_when_unclassified() {
        let mut files = BTreeMap::new();
        files.insert("/Users/will/Documents/notes.txt".to_string(), 4);
        let (primary, all, root) = primary_project(&files);
        assert_eq!(primary, "misc");
        assert_eq!(all, vec!["misc".to_string()]);
        assert_eq!(root, None);
    }
}
