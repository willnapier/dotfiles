use chrono::{DateTime, SecondsFormat, Utc};
use std::path::Path;
use std::process::Command;

/// A single git commit within a time window.
#[derive(Debug, Clone)]
pub struct Commit {
    pub short_sha: String,
    /// Commit subject — read to filter merge commits out of the SHA list.
    pub subject: String,
    pub pr: Option<u32>,
}

/// Return the commits in `repo_root` between `start` and `end` (inclusive of
/// merges — squash/merge subjects carry the PR number).
///
/// On any error (not a repo, git missing, bad output) returns an empty vec —
/// never panics.
pub fn commits_in_window(
    repo_root: &Path,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    rel_files: &[String],
) -> Vec<Commit> {
    let since = start.to_rfc3339_opts(SecondsFormat::Secs, true);
    let until = end.to_rfc3339_opts(SecondsFormat::Secs, true);

    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo_root).args([
        "log",
        // --full-history keeps merge commits that the path filter would
        // otherwise simplify away — that's where "Merge pull request #N" (the PR
        // number) lives.
        "--full-history",
        "--since",
        &since,
        "--until",
        &until,
        "--pretty=%h%x09%s",
    ]);
    // Scope to the exact files touched so a busy day's unrelated commits don't
    // get swept in. Pathspecs are interpreted relative to the repo root.
    if !rel_files.is_empty() {
        cmd.arg("--");
        cmd.args(rel_files);
    }
    let output = cmd.output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return vec![],
    };

    let stdout = match String::from_utf8(output.stdout) {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let mut commits = Vec::new();
    for line in stdout.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        // Split on the first tab into (short_sha, subject).
        let (short_sha, subject) = match line.split_once('\t') {
            Some((sha, subj)) => (sha.to_string(), subj.to_string()),
            None => (line.to_string(), String::new()),
        };
        let pr = parse_pr(&subject);
        commits.push(Commit {
            short_sha,
            subject,
            pr,
        });
    }

    commits
}

/// Make an absolute path repo-relative for use as a git pathspec. Returns
/// `None` if `abs_path` isn't under `repo_root`.
pub fn relativize(repo_root: &Path, abs_path: &str) -> Option<String> {
    let root = repo_root.to_str()?;
    let rest = abs_path.strip_prefix(root)?;
    Some(rest.trim_start_matches('/').to_string())
}

/// Find the first PR number in a commit subject: locate `(#`, then read
/// consecutive ASCII digits up to `)`. No regex — a byte/char scan.
fn parse_pr(subject: &str) -> Option<u32> {
    let bytes = subject.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > start {
                // Accept only PR-like contexts: `(#124)` (squash) or
                // `…pull request #124…` (merge). A bare `#99` issue ref is
                // ignored. `i` is at a '#' byte (ASCII) so `..i` is a boundary.
                let before = &subject[..i];
                if before.ends_with('(') || before.ends_with("pull request ") {
                    if let Ok(n) = subject[start..j].parse::<u32>() {
                        return Some(n);
                    }
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pr_merge_commit() {
        assert_eq!(
            parse_pr("Merge pull request #118 from willnapier/fix/foo-2026"),
            Some(118)
        );
    }

    #[test]
    fn test_parse_pr_ignores_bare_issue_ref() {
        assert_eq!(parse_pr("fix: handle the thing, closes #99"), None);
    }

    #[test]
    fn test_parse_pr_conventional() {
        assert_eq!(parse_pr("feat: add devlog mode (#124)"), Some(124));
    }

    #[test]
    fn test_parse_pr_none() {
        assert_eq!(parse_pr("chore: tidy up"), None);
    }

    #[test]
    fn test_parse_pr_multiple_parens_first_wins() {
        assert_eq!(parse_pr("fix: handle (edge) case (#7) and (#9)"), Some(7));
    }

    #[test]
    fn test_parse_pr_empty_parens_skipped() {
        // "(#)" has no digits; should fall through to the real one.
        assert_eq!(parse_pr("weird (#) but real (#42)"), Some(42));
    }

    #[test]
    fn test_commits_in_window_non_repo_returns_empty() {
        let commits = commits_in_window(
            Path::new("/nonexistent/not/a/repo"),
            "2026-01-01T00:00:00Z".parse().unwrap(),
            "2026-01-02T00:00:00Z".parse().unwrap(),
            &[],
        );
        assert!(commits.is_empty());
    }

    #[test]
    fn test_relativize() {
        let root = Path::new("/Users/will/Code/practiceforge");
        assert_eq!(
            relativize(root, "/Users/will/Code/practiceforge/src/main.rs"),
            Some("src/main.rs".to_string())
        );
        assert_eq!(relativize(root, "/elsewhere/x.rs"), None);
    }
}
