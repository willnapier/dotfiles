//! Audit the independent repositories and linked worktrees under `~/Code`.
//!
//! This is deliberately detection-only. It refreshes remote-tracking refs, but
//! never commits, pushes, switches branches, or edits a working tree.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditReport {
    pub worktrees_scanned: usize,
    pub repositories_scanned: usize,
    pub findings: Vec<String>,
}

#[derive(Debug, Clone)]
struct Worktree {
    path: PathBuf,
    common_dir: PathBuf,
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| format!("could not start git: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.lines().next().unwrap_or("git command failed").trim();
        Err(detail.to_string())
    }
}

fn fetch_origin(repo: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .args([
            "-c",
            "credential.interactive=never",
            "-c",
            "core.sshCommand=ssh -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=10 -o ServerAliveCountMax=2",
            "-C",
        ])
        .arg(repo)
        .args([
            "fetch",
            "--quiet",
            "--prune",
            "origin",
            "+refs/heads/*:refs/remotes/origin/*",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_HTTP_LOW_SPEED_LIMIT", "1")
        .env("GIT_HTTP_LOW_SPEED_TIME", "20")
        .output()
        .map_err(|error| format!("could not start git fetch: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.lines().next().unwrap_or("git fetch failed").trim();
        Err(detail.to_string())
    }
}

fn common_dir(worktree: &Path) -> Result<PathBuf, String> {
    let raw = git(
        worktree,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let path = PathBuf::from(raw);
    let absolute = if path.is_absolute() {
        path
    } else {
        worktree.join(path)
    };
    fs::canonicalize(&absolute).map_err(|error| format!("cannot resolve Git common dir: {error}"))
}

fn repo_name(common_dir: &Path) -> String {
    if common_dir.file_name().is_some_and(|name| name == ".git") {
        return common_dir
            .parent()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
    }
    common_dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn status_path(line: &str) -> &str {
    line.get(3..).unwrap_or("").trim_matches('"')
}

fn substantive_status_lines(status: &str) -> Vec<&str> {
    status
        .lines()
        .filter(|line| {
            let code = line.get(..2).unwrap_or("");
            let path = status_path(line);
            !(code == "??" && matches!(path, ".frontier-only" | "target" | "target/"))
        })
        .collect()
}

fn dirty_summary(name: &str, lines: &[&str]) -> String {
    let mut samples: Vec<&str> = lines.iter().take(3).map(|line| status_path(line)).collect();
    samples.retain(|path| !path.is_empty());
    let suffix = if samples.is_empty() {
        String::new()
    } else if lines.len() > samples.len() {
        format!("; e.g. {}, …", samples.join(", "))
    } else {
        format!("; {}", samples.join(", "))
    };
    format!(
        "Code dirty: {name} ({} changed {}{suffix})",
        lines.len(),
        if lines.len() == 1 { "path" } else { "paths" }
    )
}

/// Inspect every top-level directory under `~/Code`.
///
/// Linked worktrees are all checked for dirty files, then deduplicated by Git
/// common-dir for the fetch/branch-reachability phase.
pub fn audit_code_repositories(home: &Path) -> AuditReport {
    let code_dir = home.join("Code");
    let mut findings = Vec::new();
    let mut worktrees = Vec::new();

    let mut entries: Vec<_> = match fs::read_dir(&code_dir) {
        Ok(entries) => entries.filter_map(Result::ok).collect(),
        Err(error) => {
            return AuditReport {
                worktrees_scanned: 0,
                repositories_scanned: 0,
                findings: vec![format!(
                    "Code audit unavailable: cannot read {}: {error}",
                    code_dir.display()
                )],
            };
        }
    };
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if git(&path, &["rev-parse", "--is-inside-work-tree"]).as_deref() != Ok("true") {
            findings.push(format!("Code audit: {name} is not a Git worktree"));
            continue;
        }

        match git(
            &path,
            &["status", "--porcelain=v1", "--untracked-files=normal"],
        ) {
            Ok(status) => {
                let changed = substantive_status_lines(&status);
                if !changed.is_empty() {
                    findings.push(dirty_summary(&name, &changed));
                }
            }
            Err(error) => findings.push(format!("Code audit: could not inspect {name}: {error}")),
        }

        match common_dir(&path) {
            Ok(common_dir) => worktrees.push(Worktree { path, common_dir }),
            Err(error) => findings.push(format!("Code audit: could not identify {name}: {error}")),
        }
    }

    if worktrees.is_empty() {
        findings.push("Code audit: zero Git worktrees examined".to_string());
        return AuditReport {
            worktrees_scanned: 0,
            repositories_scanned: 0,
            findings,
        };
    }

    let mut groups: BTreeMap<PathBuf, Vec<Worktree>> = BTreeMap::new();
    for worktree in worktrees {
        groups
            .entry(worktree.common_dir.clone())
            .or_default()
            .push(worktree);
    }
    let worktrees_scanned = groups.values().map(Vec::len).sum();
    let repositories_scanned = groups.len();

    for (common, group) in groups {
        let repo = group
            .iter()
            .find(|worktree| worktree.path.join(".git").is_dir())
            .unwrap_or(&group[0]);
        let name = repo_name(&common);

        if git(&repo.path, &["remote", "get-url", "origin"]).is_err() {
            findings.push(format!("Code audit: {name} has no origin remote"));
            continue;
        }
        if let Err(error) = fetch_origin(&repo.path) {
            findings.push(format!(
                "Code audit: {name} could not refresh origin: {error}"
            ));
            continue;
        }

        let remote_heads = match git(
            &repo.path,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/remotes/origin/",
            ],
        ) {
            Ok(heads) => heads
                .lines()
                .filter(|line| !line.ends_with("/HEAD"))
                .count(),
            Err(error) => {
                findings.push(format!(
                    "Code audit: {name} could not enumerate origin: {error}"
                ));
                continue;
            }
        };
        if remote_heads == 0 {
            findings.push(format!(
                "Code audit: {name} origin advertises no branch heads"
            ));
            continue;
        }

        let branches = match git(
            &repo.path,
            &[
                "for-each-ref",
                "--format=%(refname:short)%09%(objectname)",
                "refs/heads/",
            ],
        ) {
            Ok(branches) => branches,
            Err(error) => {
                findings.push(format!(
                    "Code audit: {name} could not enumerate local branches: {error}"
                ));
                continue;
            }
        };
        if branches.is_empty() {
            findings.push(format!(
                "Code audit: {name} has no local branches to examine"
            ));
            continue;
        }

        for line in branches.lines() {
            let Some((branch, tip)) = line.split_once('\t') else {
                findings.push(format!(
                    "Code audit: {name} returned an unparseable branch record"
                ));
                continue;
            };
            let count = match git(
                &repo.path,
                &["rev-list", "--count", tip, "--not", "--remotes=origin"],
            ) {
                Ok(count) => count.parse::<usize>().ok(),
                Err(_) => None,
            };
            match count {
                Some(0) => {}
                Some(count) => findings.push(format!(
                    "Code unpushed: {name}/{branch} has {count} {} unreachable from origin",
                    if count == 1 { "commit" } else { "commits" }
                )),
                None => findings.push(format!(
                    "Code audit: {name}/{branch} reachability could not be established"
                )),
            }
        }
    }

    findings.sort();
    findings.dedup();
    AuditReport {
        worktrees_scanned,
        repositories_scanned,
        findings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_git(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "git -C {} {}",
            repo.display(),
            args.join(" ")
        );
    }

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let remote = home.join("remote.git");
        fs::create_dir_all(home.join("Code")).unwrap();
        let status = Command::new("git")
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(&remote)
            .status()
            .unwrap();
        assert!(status.success());

        let app = home.join("Code/app");
        fs::create_dir_all(&app).unwrap();
        run_git(&app, &["init", "--initial-branch=main"]);
        run_git(&app, &["config", "user.name", "Health Check"]);
        run_git(&app, &["config", "user.email", "health@example.invalid"]);
        fs::write(app.join("tracked.txt"), "one\n").unwrap();
        run_git(&app, &["add", "tracked.txt"]);
        run_git(&app, &["commit", "-m", "initial"]);
        run_git(&app, &["remote", "add", "origin", remote.to_str().unwrap()]);
        run_git(&app, &["push", "-u", "origin", "main"]);
        (temp, app)
    }

    #[test]
    fn green_fixture_has_nonzero_denominator_and_deduplicates_worktrees() {
        let (temp, app) = fixture();
        fs::write(app.join(".frontier-only"), "policy marker\n").unwrap();
        fs::write(app.join("target"), "build-link placeholder\n").unwrap();
        let linked = temp.path().join("Code/app-feature");
        run_git(
            &app,
            &[
                "worktree",
                "add",
                "-b",
                "feature",
                linked.to_str().unwrap(),
                "main",
            ],
        );

        let report = audit_code_repositories(temp.path());
        assert_eq!(report.worktrees_scanned, 2, "{report:?}");
        assert_eq!(report.repositories_scanned, 1, "{report:?}");
        assert!(report.findings.is_empty(), "{report:?}");
    }

    #[test]
    fn red_fixture_reports_dirty_tree_and_remote_unreachable_commit() {
        let (temp, app) = fixture();
        run_git(&app, &["switch", "-c", "local-only"]);
        fs::write(app.join("local.txt"), "local\n").unwrap();
        run_git(&app, &["add", "local.txt"]);
        run_git(&app, &["commit", "-m", "local only"]);
        fs::write(app.join("tracked.txt"), "dirty\n").unwrap();

        let report = audit_code_repositories(temp.path());
        assert_eq!(report.worktrees_scanned, 1, "{report:?}");
        assert_eq!(report.repositories_scanned, 1, "{report:?}");
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.starts_with("Code dirty: app")
                    && finding.contains("tracked.txt")),
            "{report:?}"
        );
        assert!(
            report.findings.iter().any(|finding| {
                finding == "Code unpushed: app/local-only has 1 commit unreachable from origin"
            }),
            "{report:?}"
        );
    }

    #[test]
    fn empty_code_directory_is_not_a_false_green() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("Code")).unwrap();
        let report = audit_code_repositories(temp.path());
        assert_eq!(report.worktrees_scanned, 0);
        assert!(report
            .findings
            .iter()
            .any(|finding| finding.contains("zero Git worktrees")));
    }
}
