use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

/// A simplified representation of a conversation log entry
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub role: String,
    pub content_type: EntryType,
    pub content: String,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone)]
pub enum EntryType {
    /// Plain text message
    Text,
    /// Tool invocation (name + input)
    ToolUse { tool_name: String, input: String },
    /// Tool result
    ToolResult { tool_name: String, output: String },
    /// Skill/command activation
    SkillActivation { skill_name: String },
    /// Meta (system, file snapshots, etc.)
    Meta,
}

/// Parse a conversation log file into structured entries
pub fn parse_log(path: &Path, cli_type: &str) -> Result<Vec<LogEntry>> {
    match cli_type {
        "claude" => parse_claude_log(path),
        "gemini" => parse_continuum_log(path, "gemini"),
        "codex" => parse_continuum_log(path, "codex"),
        other => anyhow::bail!("Unsupported CLI type: {}", other),
    }
}

/// Parse Claude Code native JSONL log
fn parse_claude_log(path: &Path) -> Result<Vec<LogEntry>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read log file: {}", path.display()))?;

    let mut entries = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let timestamp = v.get("timestamp").and_then(|t| t.as_str()).map(String::from);

        // File history snapshots — skip
        if v.get("type").and_then(|t| t.as_str()) == Some("file-history-snapshot") {
            continue;
        }

        let role = v
            .get("message")
            .and_then(|m| m.get("role"))
            .and_then(|r| r.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Check for skill activation in user messages
        if let Some(content) = v.get("message").and_then(|m| m.get("content")) {
            if let Some(text) = content.as_str() {
                if text.contains("<command-name>") {
                    if let Some(skill) = extract_between(text, "<command-name>/", "</command-name>")
                    {
                        entries.push(LogEntry {
                            role: role.clone(),
                            content_type: EntryType::SkillActivation {
                                skill_name: skill.to_string(),
                            },
                            content: text.to_string(),
                            timestamp: timestamp.clone(),
                        });
                        continue;
                    }
                }
            }

            // Content can be a string or array of content blocks
            if let Some(blocks) = content.as_array() {
                for block in blocks {
                    if let Some(entry) =
                        parse_content_block(block, &role, timestamp.as_deref())
                    {
                        entries.push(entry);
                    }
                }
            } else if let Some(text) = content.as_str() {
                if !text.is_empty() {
                    entries.push(LogEntry {
                        role: role.clone(),
                        content_type: EntryType::Text,
                        content: text.to_string(),
                        timestamp: timestamp.clone(),
                    });
                }
            }
        }
    }

    Ok(entries)
}

/// Parse continuum-style JSONL log (simpler format, used by gemini/codex wrappers)
fn parse_continuum_log(path: &Path, _vendor: &str) -> Result<Vec<LogEntry>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read log file: {}", path.display()))?;

    let mut entries = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let role = v
            .get("role")
            .and_then(|r| r.as_str())
            .unwrap_or("unknown")
            .to_string();
        let content_text = v
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        let timestamp = v.get("timestamp").and_then(|t| t.as_str()).map(String::from);

        if !content_text.is_empty() {
            entries.push(LogEntry {
                role,
                content_type: EntryType::Text,
                content: content_text,
                timestamp,
            });
        }
    }

    Ok(entries)
}

fn extract_between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let start_pos = text.find(start)? + start.len();
    let end_pos = text[start_pos..].find(end)? + start_pos;
    Some(&text[start_pos..end_pos])
}

/// Parse a JSON content block (tool_use, text, tool_result) into a LogEntry.
/// Shared between JSONL log parsing and stream-json parsing.
pub fn parse_content_block(
    block: &serde_json::Value,
    role: &str,
    timestamp: Option<&str>,
) -> Option<LogEntry> {
    let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match block_type {
        "tool_use" => {
            let tool_name = block
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("unknown")
                .to_string();
            let input = block
                .get("input")
                .map(|i| serde_json::to_string_pretty(i).unwrap_or_default())
                .unwrap_or_default();

            Some(LogEntry {
                role: role.to_string(),
                content_type: EntryType::ToolUse {
                    tool_name: tool_name.clone(),
                    input: input.clone(),
                },
                content: format!("Tool: {} Input: {}", tool_name, input),
                timestamp: timestamp.map(String::from),
            })
        }
        "tool_result" => {
            let output = block
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            let tool_use_id = block
                .get("tool_use_id")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();

            Some(LogEntry {
                role: role.to_string(),
                content_type: EntryType::ToolResult {
                    tool_name: tool_use_id,
                    output: output.clone(),
                },
                content: output,
                timestamp: timestamp.map(String::from),
            })
        }
        "text" => {
            let text = block
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();

            if text.is_empty() {
                None
            } else {
                Some(LogEntry {
                    role: role.to_string(),
                    content_type: EntryType::Text,
                    content: text,
                    timestamp: timestamp.map(String::from),
                })
            }
        }
        _ => None,
    }
}
