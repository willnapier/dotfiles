use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

use crate::config::Scenario;

/// Run a scenario against an AI CLI and return the path to the conversation log
pub fn run_scenario(cli_name: &str, skill: &str, scenario: &Scenario) -> Result<PathBuf> {
    match cli_name {
        "claude" => run_claude(skill, scenario),
        "gemini" => run_gemini(skill, scenario),
        other => anyhow::bail!("CLI '{}' not yet supported for live invocation", other),
    }
}

fn run_claude(skill: &str, scenario: &Scenario) -> Result<PathBuf> {
    // Record which logs exist before we run
    let logs_dir = cc_logs_dir()?;
    let before: Vec<_> = list_jsonl_files(&logs_dir)?;

    // Invoke claude -p with the skill
    let skill_flag = format!("/{}",  skill);
    let prompt = format!("{}\n{}", skill_flag, scenario.prompt);

    eprintln!("  Invoking: claude -p \"{}\" ...", scenario.prompt);

    let output = Command::new("claude")
        .arg("-p")
        .arg(&prompt)
        .arg("--dangerously-skip-permissions")
        .output()
        .context("Failed to invoke claude CLI")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("claude -p failed: {}", stderr);
    }

    // Find the new log file (the one that wasn't there before)
    let after: Vec<_> = list_jsonl_files(&logs_dir)?;
    let new_logs: Vec<_> = after
        .into_iter()
        .filter(|p| !before.contains(p))
        .collect();

    match new_logs.len() {
        0 => {
            // Fallback: use the most recently modified JSONL
            let latest = most_recent_jsonl(&logs_dir)?;
            Ok(latest)
        }
        1 => Ok(new_logs.into_iter().next().unwrap()),
        _ => {
            // Multiple new logs — take the most recent
            let mut sorted = new_logs;
            sorted.sort_by_key(|p| {
                std::fs::metadata(p)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
            });
            Ok(sorted.pop().unwrap())
        }
    }
}

fn run_gemini(skill: &str, scenario: &Scenario) -> Result<PathBuf> {
    // Gemini CLI uses -p for prompt mode
    // Skills are loaded differently — via the prompt itself
    let prompt = format!(
        "Please read and follow the skill instructions in ~/.claude/skills/{}/SKILL.md\n\n{}",
        skill, scenario.prompt
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

    // Find the most recent continuum log for gemini
    let home = dirs::home_dir().context("No home directory")?;
    let gemini_logs = home.join("Assistants/continuum-logs/gemini-cli");

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let today_dir = gemini_logs.join(&today);

    if today_dir.exists() {
        // Find most recent session directory
        let mut entries: Vec<_> = std::fs::read_dir(&today_dir)?
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });

        if let Some(latest) = entries.last() {
            let messages = latest.path().join("messages.jsonl");
            if messages.exists() {
                return Ok(messages);
            }
        }
    }

    anyhow::bail!("Could not find gemini conversation log")
}

fn cc_logs_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("No home directory")?;
    // CC stores logs per-project; find the most likely one
    let projects_dir = home.join(".claude/projects");
    if !projects_dir.exists() {
        anyhow::bail!("No .claude/projects directory found");
    }

    // Find the project dir with the most recent JSONL
    let mut best_dir = None;
    let mut best_time = std::time::SystemTime::UNIX_EPOCH;

    for entry in std::fs::read_dir(&projects_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if let Ok(latest) = most_recent_jsonl(&entry.path()) {
                if let Ok(meta) = std::fs::metadata(&latest) {
                    if let Ok(modified) = meta.modified() {
                        if modified > best_time {
                            best_time = modified;
                            best_dir = Some(entry.path());
                        }
                    }
                }
            }
        }
    }

    best_dir.context("No CC project directory with logs found")
}

fn list_jsonl_files(dir: &PathBuf) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
    Ok(files)
}

fn most_recent_jsonl(dir: &PathBuf) -> Result<PathBuf> {
    let mut files = list_jsonl_files(dir)?;
    if files.is_empty() {
        anyhow::bail!("No JSONL files found in {}", dir.display());
    }
    files.sort_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    Ok(files.pop().unwrap())
}
