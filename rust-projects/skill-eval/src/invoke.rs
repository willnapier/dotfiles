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
    // Snapshot dotfiles state before the scenario runs
    let home = dirs::home_dir().context("No home directory")?;
    let dotfiles_dir = home.join("dotfiles");
    let has_dotfiles = dotfiles_dir.join(".git").exists();

    let result = if scenario.sandbox {
        run_sandboxed(cli_name, skill, scenario)
    } else {
        run_direct(cli_name, skill, scenario)
    };

    // Auto-revert ~/dotfiles after every scenario, regardless of outcome.
    // The transcript is already captured — we only needed the agent's behaviour,
    // not the persistent side effects.
    if has_dotfiles {
        revert_dotfiles(&dotfiles_dir, &scenario.id);
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
        "Please read and follow the skill instructions in ~/.claude/skills/{}/SKILL.md\n\n{}{}",
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

/// Revert ~/dotfiles to its pre-scenario state.
/// Unstages any staged changes and discards working tree modifications.
/// This ensures no scenario mutations persist after transcript capture.
fn revert_dotfiles(dotfiles_dir: &Path, scenario_id: &str) {
    // Unstage everything
    let reset = Command::new("git")
        .arg("-C")
        .arg(dotfiles_dir)
        .arg("reset")
        .arg("HEAD")
        .arg(".")
        .output();

    if let Err(e) = &reset {
        eprintln!("  Warning: git reset failed for {}: {}", scenario_id, e);
    }

    // Discard working tree changes
    let checkout = Command::new("git")
        .arg("-C")
        .arg(dotfiles_dir)
        .arg("checkout")
        .arg("--")
        .arg(".")
        .output();

    if let Err(e) = &checkout {
        eprintln!("  Warning: git checkout failed for {}: {}", scenario_id, e);
    }

    // Clean untracked files the scenario may have created
    let clean = Command::new("git")
        .arg("-C")
        .arg(dotfiles_dir)
        .arg("clean")
        .arg("-fd")
        .output();

    if let Err(e) = &clean {
        eprintln!("  Warning: git clean failed for {}: {}", scenario_id, e);
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
