use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// Result of a git pull operation.
#[derive(Debug)]
pub enum PullResult {
    /// Already up to date.
    UpToDate,
    /// Pulled new changes.
    Updated { summary: String },
    /// No remote configured — local-only mode.
    NoRemote,
}

/// Summary of repository status.
#[derive(Debug)]
pub struct RepoStatus {
    pub is_repo: bool,
    pub has_remote: bool,
    pub clean: bool,
    pub uncommitted_count: usize,
    pub branch: String,
    pub ahead: usize,
    pub behind: usize,
}

/// Initialize a new git repository with the PracticeForge directory structure.
pub fn init_repo(path: &Path) -> Result<()> {
    // Create directory structure
    std::fs::create_dir_all(path.join("clients"))?;
    std::fs::create_dir_all(path.join("calendars"))?;
    std::fs::create_dir_all(path.join("attendance"))?;
    std::fs::create_dir_all(path.join("config"))?;

    // Keep empty dirs in git
    for dir in &["clients", "calendars", "attendance"] {
        let gitkeep = path.join(dir).join(".gitkeep");
        if !gitkeep.exists() {
            std::fs::write(&gitkeep, "")?;
        }
    }

    // git init
    run_git(path, &["init"])?;

    // Create default practice config if missing
    let practice_yaml = path.join("config").join("practice.yaml");
    if !practice_yaml.exists() {
        let default_config = "name: \"\"\naddress: \"\"\nphone: \"\"\nsession_notes_mirror: false\n";
        std::fs::write(&practice_yaml, default_config)?;
    }

    // Create default practitioners file if missing
    let practitioners_yaml = path.join("config").join("practitioners.yaml");
    if !practitioners_yaml.exists() {
        std::fs::write(&practitioners_yaml, "# Practitioners\n# - id: william\n#   name: Dr William Napier\n#   email: will@willnapier.com\n")?;
    }

    // Initial commit
    run_git(path, &["add", "-A"])?;
    run_git(path, &["commit", "-m", "Initial PracticeForge registry"])?;

    Ok(())
}

/// Clone a remote repository.
pub fn clone_repo(url: &str, path: &Path) -> Result<()> {
    let parent = path.parent().context("Invalid registry path")?;
    std::fs::create_dir_all(parent)?;

    let dir_name = path
        .file_name()
        .context("Invalid registry path")?
        .to_string_lossy();

    let output = Command::new("git")
        .args(["clone", url, &dir_name])
        .current_dir(parent)
        .output()
        .context("Failed to run git clone")?;

    if !output.status.success() {
        bail!(
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

/// Pull from remote (rebase to keep history linear).
pub fn pull(repo_path: &Path) -> Result<PullResult> {
    if !has_remote(repo_path)? {
        return Ok(PullResult::NoRemote);
    }

    let output = Command::new("git")
        .args(["pull", "--rebase"])
        .current_dir(repo_path)
        .output()
        .context("Failed to run git pull")?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Check for conflict
        if stderr.contains("CONFLICT") || stderr.contains("could not apply") {
            bail!("Merge conflict during pull. Resolve manually in {}", repo_path.display());
        }
        bail!("git pull failed: {}", stderr);
    }

    if stdout.contains("Already up to date") {
        Ok(PullResult::UpToDate)
    } else {
        Ok(PullResult::Updated {
            summary: stdout.trim().to_string(),
        })
    }
}

/// Stage specific paths and commit.
pub fn add_and_commit(repo_path: &Path, paths: &[&str], message: &str) -> Result<()> {
    for p in paths {
        run_git(repo_path, &["add", p])?;
    }

    // Check if there's anything to commit
    let output = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(repo_path)
        .output()?;

    if output.status.success() {
        // Nothing staged — nothing to commit
        return Ok(());
    }

    run_git(repo_path, &["commit", "-m", message])?;
    Ok(())
}

/// Push to remote. No-op if no remote configured.
pub fn push(repo_path: &Path) -> Result<()> {
    if !has_remote(repo_path)? {
        return Ok(());
    }

    let output = Command::new("git")
        .args(["push"])
        .current_dir(repo_path)
        .output()
        .context("Failed to run git push")?;

    if !output.status.success() {
        bail!(
            "git push failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

/// Check whether the repo has a remote origin configured.
pub fn has_remote(repo_path: &Path) -> Result<bool> {
    let output = Command::new("git")
        .args(["remote"])
        .current_dir(repo_path)
        .output()
        .context("Failed to run git remote")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.trim().contains("origin"))
}

/// Add a remote origin URL.
pub fn add_remote(repo_path: &Path, url: &str) -> Result<()> {
    if has_remote(repo_path)? {
        run_git(repo_path, &["remote", "set-url", "origin", url])?;
    } else {
        run_git(repo_path, &["remote", "add", "origin", url])?;
    }
    Ok(())
}

/// Get repository status summary.
pub fn status(repo_path: &Path) -> Result<RepoStatus> {
    let is_repo = repo_path.join(".git").exists();
    if !is_repo {
        return Ok(RepoStatus {
            is_repo: false,
            has_remote: false,
            clean: true,
            uncommitted_count: 0,
            branch: String::new(),
            ahead: 0,
            behind: 0,
        });
    }

    let remote = has_remote(repo_path)?;

    // Get branch name
    let output = Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(repo_path)
        .output()?;
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Count uncommitted changes
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo_path)
        .output()?;
    let porcelain = String::from_utf8_lossy(&output.stdout);
    let uncommitted_count = porcelain.lines().filter(|l| !l.is_empty()).count();

    // Get ahead/behind if remote exists
    let (ahead, behind) = if remote {
        let output = Command::new("git")
            .args(["rev-list", "--left-right", "--count", "HEAD...@{upstream}"])
            .current_dir(repo_path)
            .output();

        match output {
            Ok(o) if o.status.success() => {
                let s = String::from_utf8_lossy(&o.stdout);
                let parts: Vec<&str> = s.trim().split('\t').collect();
                if parts.len() == 2 {
                    (
                        parts[0].parse().unwrap_or(0),
                        parts[1].parse().unwrap_or(0),
                    )
                } else {
                    (0, 0)
                }
            }
            _ => (0, 0),
        }
    } else {
        (0, 0)
    };

    Ok(RepoStatus {
        is_repo: true,
        has_remote: remote,
        clean: uncommitted_count == 0,
        uncommitted_count,
        branch,
        ahead,
        behind,
    })
}

/// Run a git command, returning an error if it fails.
fn run_git(repo_path: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .output()
        .with_context(|| format!("Failed to run git {}", args.join(" ")))?;

    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
