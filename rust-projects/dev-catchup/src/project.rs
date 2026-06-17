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

/// Whether a project bucket is substantive dev work, as opposed to incidental
/// churn — memory/config edits ("claude"), notes ("forge"), or unclassifiable
/// files ("misc"). Used to suppress incidental buckets on days that also have
/// real project work.
pub fn is_substantive(project: &str) -> bool {
    !matches!(project, "claude" | "misc" | "forge")
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
    fn test_is_substantive() {
        assert!(is_substantive("practiceforge"));
        assert!(is_substantive("dotfiles"));
        assert!(is_substantive("skills"));
        assert!(!is_substantive("claude"));
        assert!(!is_substantive("misc"));
        assert!(!is_substantive("forge"));
    }
}
