use anyhow::{Context, Result};
use std::process::Command;

use crate::config::Scenario;
use crate::log_parser::{self, EntryType, LogEntry};

/// Preamble cue prepended to scenario prompts to trigger full session behaviour
const SESSION_CUE: &str = "You are starting a new interactive session. \
Follow all session preamble instructions in your skill file before responding.\n\n";

/// Run a scenario against an AI CLI and return parsed log entries
pub fn run_scenario(cli_name: &str, skill: &str, scenario: &Scenario) -> Result<Vec<LogEntry>> {
    match cli_name {
        "claude" => run_claude(skill, scenario),
        "gemini" => run_gemini(skill, scenario),
        other => anyhow::bail!("CLI '{}' not yet supported for live invocation", other),
    }
}

fn run_claude(skill: &str, scenario: &Scenario) -> Result<Vec<LogEntry>> {
    let skill_flag = format!("/{}", skill);
    let prompt = format!("{}\n{}{}", skill_flag, SESSION_CUE, scenario.prompt);

    eprintln!("  Invoking: claude -p \"{}\" ...", scenario.prompt);

    let output = Command::new("claude")
        .arg("-p")
        .arg(&prompt)
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--dangerously-skip-permissions")
        .arg("--no-session-persistence")
        .output()
        .context("Failed to invoke claude CLI")?;

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

    // Gemini doesn't have stream-json; parse stdout as plain text response
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(vec![LogEntry {
        role: "assistant".to_string(),
        content_type: EntryType::Text,
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
