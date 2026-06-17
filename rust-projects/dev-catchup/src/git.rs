use chrono::{DateTime, SecondsFormat, Utc};
use std::path::Path;
use std::process::Command;

/// A single git commit within a time window.
#[derive(Debug, Clone)]
pub struct Commit {
    pub short_sha: String,
    /// Kept as part of the public API; read by `parse_pr` at construction.
    #[allow(dead_code)]
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
) -> Vec<Commit> {
    let since = start.to_rfc3339_opts(SecondsFormat::Secs, true);
    let until = end.to_rfc3339_opts(SecondsFormat::Secs, true);

    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args([
            "log",
            "--since",
            &since,
            "--until",
            &until,
            "--pretty=%h%x09%s",
        ])
        .output();

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

/// Find the first PR number in a commit subject: locate `(#`, then read
/// consecutive ASCII digits up to `)`. No regex — a byte/char scan.
fn parse_pr(subject: &str) -> Option<u32> {
    let bytes = subject.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'(' && bytes[i + 1] == b'#' {
            let mut j = i + 2;
            let start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            // Require at least one digit and a closing ')'.
            if j > start && j < bytes.len() && bytes[j] == b')' {
                if let Ok(s) = std::str::from_utf8(&bytes[start..j]) {
                    if let Ok(n) = s.parse::<u32>() {
                        return Some(n);
                    }
                }
            }
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pr_merge() {
        assert_eq!(
            parse_pr("Merge pull request #5 from foo/bar (#124)"),
            Some(124)
        );
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
        );
        assert!(commits.is_empty());
    }
}
