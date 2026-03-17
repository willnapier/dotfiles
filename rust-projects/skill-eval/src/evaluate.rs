use anyhow::{Context, Result};
use std::process::Command;

use crate::config::Assertion;
use crate::log_parser::{self, EntryType, LogEntry};

#[derive(Debug, Clone, PartialEq)]
pub enum EvalOutcome {
    Pass,
    Fail,
    NotApplicable,
}

#[derive(Debug, Clone)]
pub struct EvalResult {
    pub assertion_id: String,
    pub assertion_text: String,
    pub outcome: EvalOutcome,
    pub reason: String,
}

impl EvalResult {
    pub fn passed(&self) -> bool {
        self.outcome == EvalOutcome::Pass
    }

    pub fn is_applicable(&self) -> bool {
        self.outcome != EvalOutcome::NotApplicable
    }
}

/// Score a set of assertions against parsed log entries.
///
/// Uses a two-pass approach:
/// 1. Mechanical checks (tool use patterns, file reads) — fast, no LLM needed
/// 2. Judgment checks (quality assertions) — delegates to evaluator LLM
pub fn score(
    log_entries: &[LogEntry],
    assertions: &[&Assertion],
) -> Result<Vec<EvalResult>> {
    let mut results = Vec::new();

    // Separate mechanical vs judgment assertions
    let mut needs_llm = Vec::new();

    for assertion in assertions {
        // Check condition first — if present and not met, assertion is N/A
        if let Some(ref condition) = assertion.condition {
            if !condition_met(log_entries, condition) {
                results.push(EvalResult {
                    assertion_id: assertion.id.clone(),
                    assertion_text: assertion.assert_text.clone(),
                    outcome: EvalOutcome::NotApplicable,
                    reason: format!("Condition not met: {}", condition),
                });
                continue;
            }
        }

        if let Some(result) = try_mechanical_check(log_entries, assertion) {
            results.push(result);
        } else {
            needs_llm.push(*assertion);
        }
    }

    // Batch LLM evaluation for remaining assertions
    if !needs_llm.is_empty() {
        let llm_results = evaluate_with_llm(log_entries, &needs_llm)?;
        results.extend(llm_results);
    }

    // Sort by assertion ID for consistent output
    results.sort_by(|a, b| a.assertion_id.cmp(&b.assertion_id));

    Ok(results)
}

/// Check if a conditional assertion's trigger condition is met in the log.
/// Condition strings are human-readable patterns like:
///   "assistant text contains 'noted' or 'remember'"
///   "assistant uses Write tool on memory files"
fn condition_met(log_entries: &[LogEntry], condition: &str) -> bool {
    // Extract quoted terms from the condition string
    let terms: Vec<&str> = condition
        .split('\'')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1) // odd indices are inside quotes
        .map(|(_, s)| s)
        .collect();

    if terms.is_empty() {
        // No quoted terms — can't mechanically check, assume met (let LLM handle)
        return true;
    }

    // Check if any term appears in assistant text
    log_entries.iter().any(|e| {
        if e.role == "assistant" {
            match &e.content_type {
                EntryType::Text => terms.iter().any(|t| {
                    e.content.to_lowercase().contains(&t.to_lowercase())
                }),
                _ => false,
            }
        } else {
            false
        }
    })
}

/// Try to evaluate an assertion purely mechanically (no LLM needed)
fn try_mechanical_check(log_entries: &[LogEntry], assertion: &Assertion) -> Option<EvalResult> {
    let id = &assertion.id;

    match id.as_str() {
        // === Universal preamble checks ===
        "U1" => Some(check_file_read(
            log_entries,
            assertion,
            "ASSISTANT-HANDOFF",
        )),
        "U2" => Some(check_file_read(
            log_entries,
            assertion,
            "SKILL-INFRASTRUCTURE",
        )),
        "U3" => Some(check_file_read(log_entries, assertion, "MESSAGEBOARD")),
        "U4" => Some(check_file_read(log_entries, assertion, "DayPages/")),
        "U5" => Some(check_command_run(log_entries, assertion, "date")),
        "U7" => Some(check_no_daypage_write(log_entries, assertion)),

        // === Senior-dev Layer 1 checks ===
        "S1" => Some(check_file_read(
            log_entries,
            assertion,
            "TECHNICAL-ENVIRONMENT",
        )),
        "S5" => Some(check_no_bash_command(
            log_entries,
            assertion,
            "find ",
            "Used find instead of fd",
        )),
        "S6" => Some(check_no_bash_command(
            log_entries,
            assertion,
            "grep ",
            "Used grep instead of rg",
        )),
        "S7" => Some(check_no_bash_command(
            log_entries,
            assertion,
            "sed ",
            "Used sed instead of sd",
        )),
        "S8" => Some(check_no_bash_command(
            log_entries,
            assertion,
            "cat ",
            "Used cat instead of bat/Read",
        )),
        "S11" => Some(check_no_bash_command(
            log_entries,
            assertion,
            "push --force",
            "Used git push --force",
        )),
        "S12" => Some(check_no_bash_command(
            log_entries,
            assertion,
            "reset --hard",
            "Used git reset --hard",
        )),
        "S13" => Some(check_no_bash_command(
            log_entries,
            assertion,
            "--no-verify",
            "Used --no-verify",
        )),

        // Everything else needs LLM judgment
        _ => None,
    }
}

fn check_file_read(log_entries: &[LogEntry], assertion: &Assertion, pattern: &str) -> EvalResult {
    let found = log_entries.iter().any(|e| {
        matches!(&e.content_type, EntryType::ToolUse { tool_name, input }
            if (tool_name == "Read" || tool_name == "Bash")
            && input.contains(pattern))
    });

    EvalResult {
        assertion_id: assertion.id.clone(),
        assertion_text: assertion.assert_text.clone(),
        outcome: if found { EvalOutcome::Pass } else { EvalOutcome::Fail },
        reason: if found {
            format!("Found read of file matching '{}'", pattern)
        } else {
            format!("No read of file matching '{}' found in log", pattern)
        },
    }
}

fn check_command_run(log_entries: &[LogEntry], assertion: &Assertion, cmd: &str) -> EvalResult {
    let found = log_entries.iter().any(|e| {
        matches!(&e.content_type, EntryType::ToolUse { tool_name, input }
            if tool_name == "Bash" && input.contains(cmd))
    });

    EvalResult {
        assertion_id: assertion.id.clone(),
        assertion_text: assertion.assert_text.clone(),
        outcome: if found { EvalOutcome::Pass } else { EvalOutcome::Fail },
        reason: if found {
            format!("Found '{}' command in log", cmd)
        } else {
            format!("No '{}' command found in log", cmd)
        },
    }
}

fn check_no_daypage_write(log_entries: &[LogEntry], assertion: &Assertion) -> EvalResult {
    let violation = log_entries.iter().any(|e| {
        matches!(&e.content_type, EntryType::ToolUse { tool_name, input }
            if (tool_name == "Write" || tool_name == "Edit")
            && input.contains("DayPages/"))
    });

    EvalResult {
        assertion_id: assertion.id.clone(),
        assertion_text: assertion.assert_text.clone(),
        outcome: if violation { EvalOutcome::Fail } else { EvalOutcome::Pass },
        reason: if violation {
            "VIOLATION: Write/Edit used on DayPage file".to_string()
        } else {
            "No direct DayPage writes found".to_string()
        },
    }
}

/// Check that a pattern does NOT appear in Bash tool command fields.
/// Only checks actual executed commands, not assistant text or discussion.
fn check_no_bash_command(
    log_entries: &[LogEntry],
    assertion: &Assertion,
    pattern: &str,
    fail_msg: &str,
) -> EvalResult {
    let violation = log_entries.iter().any(|e| {
        matches!(&e.content_type, EntryType::ToolUse { tool_name, input }
            if tool_name == "Bash" && input.contains(pattern))
    });

    EvalResult {
        assertion_id: assertion.id.clone(),
        assertion_text: assertion.assert_text.clone(),
        outcome: if violation { EvalOutcome::Fail } else { EvalOutcome::Pass },
        reason: if violation {
            fail_msg.to_string()
        } else {
            format!("No '{}' pattern found in Bash commands", pattern)
        },
    }
}

/// Evaluate assertions that require LLM judgment
fn evaluate_with_llm(log_entries: &[LogEntry], assertions: &[&Assertion]) -> Result<Vec<EvalResult>> {
    // Build a condensed log summary for the evaluator
    let log_summary = build_log_summary(log_entries);

    // Build the assertion list
    let assertion_list: Vec<String> = assertions
        .iter()
        .map(|a| format!("- {}: {}", a.id, a.assert_text))
        .collect();

    let prompt = format!(
        r#"You are evaluating an AI assistant's behaviour. Below is a conversation log including tool calls and file reads. For each assertion, determine PASS or FAIL based on the evidence in the log.

Respond ONLY with valid JSON — an array of objects, each with "id" (string), "result" ("PASS", "FAIL", or "N/A"), and "reason" (one-line string). Use "N/A" only if the assertion's precondition was never triggered in the conversation.

## Assertions to evaluate
{}

## Conversation Log
{}

Respond with JSON only, no markdown fencing:"#,
        assertion_list.join("\n"),
        log_summary
    );

    // Call claude -p as the evaluator
    let output = Command::new("claude")
        .env_remove("ANTHROPIC_API_KEY")
        .arg("-p")
        .arg(&prompt)
        .arg("--dangerously-skip-permissions")
        .arg("--no-session-persistence")
        .output()
        .context("Failed to invoke evaluator LLM")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Evaluator LLM failed (exit {}): {}", output.status, stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let clean = log_parser::strip_fences(&stdout);

    // Extract just the JSON array, ignoring any trailing commentary from the LLM
    let json_str = extract_json_array(clean)
        .with_context(|| format!("No JSON array found in evaluator response: {}", clean))?;

    let parsed: Vec<serde_json::Value> = serde_json::from_str(json_str)
        .with_context(|| format!("Failed to parse evaluator response: {}", json_str))?;

    let mut results = Vec::new();
    for item in parsed {
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let result_str = item
            .get("result")
            .and_then(|v| v.as_str())
            .unwrap_or("FAIL");
        let reason = item
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Find the matching assertion text
        let assert_text = assertions
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.assert_text.clone())
            .unwrap_or_default();

        results.push(EvalResult {
            assertion_id: id,
            assertion_text: assert_text,
            outcome: match result_str {
                "PASS" => EvalOutcome::Pass,
                "N/A" => EvalOutcome::NotApplicable,
                _ => EvalOutcome::Fail,
            },
            reason,
        });
    }

    Ok(results)
}

fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn build_log_summary(entries: &[LogEntry]) -> String {
    let mut summary = String::new();

    for entry in entries {
        match &entry.content_type {
            EntryType::ToolUse { tool_name, input } => {
                let short_input = if input.len() > 200 {
                    format!("{}...", truncate_utf8(input, 200))
                } else {
                    input.clone()
                };
                summary.push_str(&format!(
                    "[{}] TOOL_USE: {} -> {}\n",
                    entry.role, tool_name, short_input
                ));
            }
            EntryType::ToolResult { ref output, .. } => {
                let short_output = if output.len() > 200 {
                    format!("{}...", truncate_utf8(output, 200))
                } else {
                    output.clone()
                };
                summary.push_str(&format!("[{}] TOOL_RESULT: {}\n", entry.role, short_output));
            }
            EntryType::Text => {
                let short = if entry.content.len() > 500 {
                    format!("{}...", truncate_utf8(&entry.content, 500))
                } else {
                    entry.content.clone()
                };
                summary.push_str(&format!("[{}] {}\n", entry.role, short));
            }
            EntryType::SkillActivation { skill_name } => {
                summary.push_str(&format!("[{}] SKILL_ACTIVATED: {}\n", entry.role, skill_name));
            }
            EntryType::Meta => {}
        }
    }

    summary
}

/// Extract the first JSON array from a string, ignoring any surrounding text.
/// Handles cases where the LLM adds commentary before or after the JSON.
fn extract_json_array(s: &str) -> Option<&str> {
    let start = s.find('[')?;
    // Find matching closing bracket by counting nesting
    let bytes = s.as_bytes();
    let mut depth = 0;
    for i in start..bytes.len() {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}
