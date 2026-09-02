use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Scenario;
use crate::log_parser::{self, LogEntry};

/// Preamble cue prepended to scenario prompts to trigger full session behaviour
const SESSION_CUE: &str = "You are starting a new interactive session. \
Follow all session preamble instructions in your skill file before responding.\n\n";

/// Run a scenario against an AI CLI and return parsed log entries.
/// After capturing the transcript, any mutations to ~/dotfiles are auto-reverted
/// so that scenario side effects never persist.
pub fn run_scenario(cli_name: &str, skill: &str, scenario: &Scenario) -> Result<Vec<LogEntry>> {
    let home = dirs::home_dir().context("No home directory")?;
    let dotfiles_dir = home.join("dotfiles");
    let has_dotfiles = dotfiles_dir.join(".git").exists();

    // Stash any pre-existing uncommitted work before the scenario runs.
    // This prevents the post-scenario revert from destroying legitimate work.
    // If the tree cannot be secured, nothing runs: the revert below ends in
    // `git clean -fd`, which must never execute over unconfirmed state (D2-11).
    let had_stash = if has_dotfiles {
        match stash_dotfiles(&dotfiles_dir) {
            Stash::Clean => false,
            Stash::Stashed => true,
            Stash::Failed(why) => anyhow::bail!(
                "refusing to run scenario {}: could not secure ~/dotfiles ({why}); nothing was run and nothing will be reverted",
                scenario.id
            ),
        }
    } else {
        false
    };

    let result = if scenario.sandbox {
        run_sandboxed(cli_name, skill, scenario)
    } else {
        run_direct(cli_name, skill, scenario)
    };

    // Auto-revert scenario mutations, then restore stashed work.
    if has_dotfiles {
        revert_dotfiles(&dotfiles_dir, &scenario.id, had_stash);
    }

    result
}

fn run_direct(cli_name: &str, skill: &str, scenario: &Scenario) -> Result<Vec<LogEntry>> {
    match cli_name {
        "claude" => run_claude(skill, scenario, None),
        "gemini" => run_gemini(skill, scenario),
        other => anyhow::bail!("CLI '{}' not yet supported for live invocation", other),
    }
}

fn run_sandboxed(cli_name: &str, skill: &str, scenario: &Scenario) -> Result<Vec<LogEntry>> {
    let worktree = Worktree::create(&scenario.id)?;
    eprintln!("  Sandbox: {}", worktree.path.display());

    let result = match cli_name {
        "claude" => run_claude(skill, scenario, Some(&worktree.path)),
        "gemini" => run_gemini(skill, scenario),
        other => anyhow::bail!("CLI '{}' not yet supported for live invocation", other),
    };

    // Always clean up, even on error
    if let Err(e) = worktree.cleanup() {
        eprintln!("  Warning: worktree cleanup failed: {}", e);
    }

    result
}

fn run_claude(skill: &str, scenario: &Scenario, sandbox_dir: Option<&Path>) -> Result<Vec<LogEntry>> {
    let skill_flag = format!("/{}", skill);
    let prompt = format!("{}\n{}{}", skill_flag, SESSION_CUE, scenario.prompt);

    eprintln!("  Invoking: claude -p \"{}\" ...", scenario.prompt);

    let mut cmd = Command::new("claude");
    // Clear ANTHROPIC_API_KEY so claude uses OAuth/Max, not the billed API.
    cmd.env_remove("ANTHROPIC_API_KEY");
    cmd.arg("-p")
        .arg(&prompt)
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--dangerously-skip-permissions")
        .arg("--no-session-persistence");

    if let Some(dir) = sandbox_dir {
        // Run claude in the worktree directory so file edits land there
        cmd.current_dir(dir);
        // Also grant access to the worktree
        cmd.arg("--add-dir").arg(dir);
    }

    let output = cmd.output().context("Failed to invoke claude CLI")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("claude -p failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_stream_json(&stdout)
}

fn run_gemini(skill: &str, scenario: &Scenario) -> Result<Vec<LogEntry>> {
    let prompt = format!(
        // Canonical vendor-neutral path (2026-07-29): this prompt goes to a
        // NON-Claude harness, so it must not point at a Claude-owned directory.
        "Please read and follow the skill instructions in ~/Assistants/skills/{}/SKILL.md\n\n{}{}",
        skill, SESSION_CUE, scenario.prompt
    );

    eprintln!("  Invoking: gemini -p \"{}\" ...", scenario.prompt);

    let output = Command::new("gemini")
        .arg("-p")
        .arg(&prompt)
        .output()
        .context("Failed to invoke gemini CLI")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("gemini -p failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(vec![LogEntry {
        role: "assistant".to_string(),
        content_type: log_parser::EntryType::Text,
        content: stdout.to_string(),
        timestamp: None,
    }])
}

/// Parse Claude's --output-format stream-json --verbose output into LogEntries
fn parse_stream_json(output: &str) -> Result<Vec<LogEntry>> {
    let mut entries = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let msg_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match msg_type {
            "assistant" | "user" => {
                let msg = match v.get("message") {
                    Some(m) => m,
                    None => continue,
                };

                let role = msg
                    .get("role")
                    .and_then(|r| r.as_str())
                    .unwrap_or(msg_type);

                if let Some(blocks) = msg.get("content").and_then(|c| c.as_array()) {
                    for block in blocks {
                        if let Some(entry) = log_parser::parse_content_block(block, role, None) {
                            entries.push(entry);
                        }
                    }
                }
            }
            // Skip system, rate_limit_event, result types
            _ => {}
        }
    }

    Ok(entries)
}

/// Outcome of securing `~/dotfiles` before a scenario. Only `Clean` (status
/// ran and reported nothing) and `Stashed` (stash push exited 0) permit the
/// post-scenario `git clean -fd`; anything else is `Failed` and the scenario
/// must not run at all.
#[derive(Debug, PartialEq, Eq)]
pub enum Stash {
    Clean,
    Stashed,
    Failed(String),
}

/// Stash any pre-existing uncommitted work in ~/dotfiles before a scenario runs.
pub fn stash_dotfiles(dotfiles_dir: &Path) -> Stash {
    // Check if there's anything to stash (staged, unstaged or untracked)
    let status = match Command::new("git")
        .arg("-C")
        .arg(dotfiles_dir)
        .arg("status")
        .arg("--porcelain")
        .output()
    {
        Ok(o) if o.status.success() => o,
        Ok(o) => return Stash::Failed(format!("git status exited {}: {}", o.status, String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => return Stash::Failed(format!("could not run git status: {e}")),
    };
    if status.stdout.is_empty() {
        return Stash::Clean;
    }

    match Command::new("git")
        .arg("-C")
        .arg(dotfiles_dir)
        .arg("stash")
        .arg("push")
        .arg("-m")
        .arg("skill-eval: pre-scenario stash")
        .arg("--include-untracked")
        .output()
    {
        Ok(o) if o.status.success() => {
            eprintln!("  Stashed pre-existing uncommitted work in ~/dotfiles");
            Stash::Stashed
        }
        Ok(o) => Stash::Failed(format!("git stash push exited {}: {}", o.status, String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Stash::Failed(format!("could not run git stash: {e}")),
    }
}

/// Revert ~/dotfiles to its pre-scenario state, then restore any stashed work.
/// Uses stash-based approach to avoid destroying uncommitted work that existed
/// before the eval run started.
fn revert_dotfiles(dotfiles_dir: &Path, scenario_id: &str, had_stash: bool) {
    // Unstage everything
    let _ = Command::new("git")
        .arg("-C")
        .arg(dotfiles_dir)
        .arg("reset")
        .arg("HEAD")
        .arg(".")
        .output();

    // Discard working tree changes from the scenario
    let _ = Command::new("git")
        .arg("-C")
        .arg(dotfiles_dir)
        .arg("checkout")
        .arg("--")
        .arg(".")
        .output();

    // Clean untracked files the scenario may have created
    let _ = Command::new("git")
        .arg("-C")
        .arg(dotfiles_dir)
        .arg("clean")
        .arg("-fd")
        .output();

    // Restore pre-existing work if we stashed it
    if had_stash {
        let pop = Command::new("git")
            .arg("-C")
            .arg(dotfiles_dir)
            .arg("stash")
            .arg("pop")
            .output();

        match &pop {
            Ok(output) if output.status.success() => {
                eprintln!("  Restored pre-existing uncommitted work from stash");
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("  Warning: git stash pop failed: {}", stderr);
            }
            Err(e) => {
                eprintln!("  Warning: git stash pop failed: {}", e);
            }
        }
    }

    eprintln!("  Auto-reverted ~/dotfiles after scenario '{}'", scenario_id);
}

/// Disposable git worktree for sandboxing scenario runs
struct Worktree {
    path: PathBuf,
    repo_dir: PathBuf,
    branch_name: String,
    bare_path: Option<PathBuf>,
}

impl Worktree {
    /// Create a new worktree from the dotfiles repo at HEAD
    fn create(scenario_id: &str) -> Result<Self> {
        let home = dirs::home_dir().context("No home directory")?;
        let repo_dir = home.join("dotfiles");

        if !repo_dir.join(".git").exists() {
            anyhow::bail!("~/dotfiles is not a git repository");
        }

        let branch_name = format!(
            "skill-eval-sandbox-{}-{}",
            scenario_id,
            std::process::id()
        );
        let worktree_path = std::env::temp_dir().join(&branch_name);

        // Create worktree on a temporary branch (not detached) so git commit works
        let output = Command::new("git")
            .arg("-C")
            .arg(&repo_dir)
            .arg("worktree")
            .arg("add")
            .arg("-b")
            .arg(&branch_name)
            .arg(&worktree_path)
            .arg("HEAD")
            .output()
            .context("Failed to create git worktree")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git worktree add failed: {}", stderr);
        }

        // Set up a local bare remote so git push works inside the sandbox.
        // This is a throwaway — the bare repo is cleaned up with the worktree.
        let bare_path = std::env::temp_dir().join(format!("{}-bare", branch_name));
        let _ = Command::new("git")
            .arg("init")
            .arg("--bare")
            .arg(&bare_path)
            .output();
        let _ = Command::new("git")
            .arg("-C")
            .arg(&worktree_path)
            .arg("remote")
            .arg("add")
            .arg("origin")
            .arg(&bare_path)
            .output();
        // Push current state so the remote has the branch
        let _ = Command::new("git")
            .arg("-C")
            .arg(&worktree_path)
            .arg("push")
            .arg("-u")
            .arg("origin")
            .arg(&branch_name)
            .output();

        Ok(Worktree {
            path: worktree_path,
            repo_dir,
            branch_name,
            bare_path: Some(bare_path),
        })
    }

    /// Remove the worktree, its branch, and the bare remote
    fn cleanup(&self) -> Result<()> {
        // Remove worktree
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.repo_dir)
            .arg("worktree")
            .arg("remove")
            .arg("--force")
            .arg(&self.path)
            .output()
            .context("Failed to remove git worktree")?;

        if !output.status.success() {
            let _ = std::fs::remove_dir_all(&self.path);
        }

        // Delete the temporary branch from the main repo
        let _ = Command::new("git")
            .arg("-C")
            .arg(&self.repo_dir)
            .arg("branch")
            .arg("-D")
            .arg(&self.branch_name)
            .output();

        // Remove the bare remote repo
        if let Some(ref bare) = self.bare_path {
            let _ = std::fs::remove_dir_all(bare);
        }

        // Prune stale worktree entries
        let _ = Command::new("git")
            .arg("-C")
            .arg(&self.repo_dir)
            .arg("worktree")
            .arg("prune")
            .output();

        Ok(())
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        // Best-effort cleanup on panic/early return
        let _ = self.cleanup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn git(dir: &Path, args: &[&str]) -> std::process::Output {
        Command::new("git").arg("-C").arg(dir).args(args).output().unwrap()
    }

    fn repo_with_commit() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        assert!(git(d, &["init", "-q"]).status.success());
        git(d, &["config", "user.email", "t@example.invalid"]);
        git(d, &["config", "user.name", "t"]);
        fs::write(d.join("tracked.txt"), "v1\n").unwrap();
        git(d, &["add", "."]);
        assert!(git(d, &["commit", "-q", "-m", "base"]).status.success());
        tmp
    }

    /// D2-11: a directory git cannot report on is never "clean" — the scenario must not run.
    #[test]
    fn unreadable_status_is_failed_not_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let not_a_repo = tmp.path().join("plain");
        fs::create_dir(&not_a_repo).unwrap();
        match stash_dotfiles(&not_a_repo) {
            Stash::Failed(why) => assert!(why.contains("git status"), "{why}"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn clean_repo_is_clean_and_dirty_repo_is_stashed_only_when_push_succeeds() {
        let tmp = repo_with_commit();
        let d = tmp.path();
        assert_eq!(stash_dotfiles(d), Stash::Clean);

        fs::write(d.join("untracked.txt"), "wip\n").unwrap();
        fs::write(d.join("tracked.txt"), "v2\n").unwrap();
        assert_eq!(stash_dotfiles(d), Stash::Stashed);
        assert!(!d.join("untracked.txt").exists(), "untracked work moved into the stash");
        assert_eq!(fs::read_to_string(d.join("tracked.txt")).unwrap(), "v1\n");
        let list = String::from_utf8(git(d, &["stash", "list"]).stdout).unwrap();
        assert!(list.contains("skill-eval: pre-scenario stash"), "{list}");

        // Restore and confirm nothing was lost.
        assert!(git(d, &["stash", "pop"]).status.success());
        assert_eq!(fs::read_to_string(d.join("untracked.txt")).unwrap(), "wip\n");
        assert_eq!(fs::read_to_string(d.join("tracked.txt")).unwrap(), "v2\n");
    }

    /// A stash push that exits non-zero must not be reported as Stashed.
    #[test]
    fn failed_stash_push_is_failed() {
        let tmp = repo_with_commit();
        let d = tmp.path();
        fs::write(d.join("untracked.txt"), "wip\n").unwrap();
        // Make the stash ref unwritable so `git stash push` fails after status succeeded.
        let refs = d.join(".git/refs");
        let mut perms = fs::metadata(&refs).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&refs, perms.clone()).unwrap();
        let result = stash_dotfiles(d);
        perms.set_readonly(false);
        fs::set_permissions(&refs, perms).unwrap();
        match result {
            Stash::Failed(why) => assert!(why.contains("stash"), "{why}"),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(d.join("untracked.txt").exists(), "work untouched on failure");
    }
}
