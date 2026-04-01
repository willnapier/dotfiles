use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

use crate::orient;
use crate::types::{DreamResponse, MemoryState};

const SYSTEM_PROMPT: &str = r#"You are a memory consolidation agent for a multi-vendor AI assistant system. Your job is to update a set of memory files based on new conversation sessions.

MEMORY FILE FORMAT:
Each memory file has YAML frontmatter (name, description, type) followed by a markdown body.
Types: user (preferences/habits), feedback (behavioral corrections), project (active project state), reference (stable reference info).
Filename convention: [type]_[topic].md
Index file (MEMORY.md) has entries like: - [Title](filename.md) — one-line hook

RULES:
1. Only create/update/delete memories when sessions contain PERSISTENT information — preferences, corrections, project decisions, reference facts. Do NOT memorize ephemeral conversation content.
2. When updating a file, preserve the YAML frontmatter format exactly. Always include name, description, and type fields.
3. When sessions contradict existing memories, update the memory to reflect the LATEST state. Add a date annotation like "(updated 2026-04-01)" for significant changes.
4. Merge related memories rather than creating many small files. Prefer fewer, richer files.
5. The MEMORY.md index MUST stay under 200 lines. If currently over, consolidate sections or remove stale entries. Each entry should be one line, under 150 characters.
6. Do NOT delete feedback memories unless explicitly superseded by a newer feedback. Feedback represents behavioral corrections that should persist.
7. File content in your response must be COMPLETE — not patches or diffs. Include the full frontmatter and body.
8. For the memory_index field, provide the COMPLETE new MEMORY.md content.
9. MEMORY.md is an index, not a knowledge store. Move substantive content from MEMORY.md into topic files and leave only short pointer entries in the index.
10. Fix any integrity issues (orphaned references, unindexed files) in your proposed changes.
11. Convert any relative dates ("last week", "yesterday") to absolute dates.

If no changes are warranted, return empty arrays and "UNCHANGED" for memory_index."#;

/// JSON Schema for DreamResponse, used with claude's --json-schema flag to force structured output.
const DREAM_RESPONSE_SCHEMA: &str = r#"{"type":"object","properties":{"files_to_update":{"type":"array","items":{"type":"object","properties":{"filename":{"type":"string"},"content":{"type":"string"},"reason":{"type":"string"}},"required":["filename","content","reason"]}},"files_to_create":{"type":"array","items":{"type":"object","properties":{"filename":{"type":"string"},"content":{"type":"string"},"reason":{"type":"string"}},"required":["filename","content","reason"]}},"files_to_delete":{"type":"array","items":{"type":"object","properties":{"filename":{"type":"string"},"reason":{"type":"string"}},"required":["filename","reason"]}},"memory_index":{"type":"string"},"summary":{"type":"string"}},"required":["files_to_update","files_to_create","files_to_delete","memory_index","summary"]}"#;

/// Wrapper structure for `claude -p --output-format json` responses.
/// The actual model output lives in `structured_output` when --json-schema is used,
/// or in `result` as a plain string otherwise.
#[derive(Deserialize)]
struct ClaudeJsonEnvelope {
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    structured_output: Option<serde_json::Value>,
}

/// Check if the model command is a `claude` invocation (i.e. supports --output-format/--json-schema).
fn is_claude_cmd(model_cmd: &str) -> bool {
    let first = model_cmd.split_whitespace().next().unwrap_or("");
    first == "claude" || first.ends_with("/claude")
}

/// Run the AI consolidation phase
pub fn run(
    model_cmd: &str,
    memory_state: &MemoryState,
    session_context: &str,
) -> Result<DreamResponse> {
    let memory_context = orient::format_memory_state(memory_state);

    let index_note = if memory_state.index_line_count > 200 {
        format!(
            "\n\nURGENT: MEMORY.md is {} lines (limit: 200). It contains substantive content that MUST be extracted into topic files. The index should contain ONLY short pointer entries.\n",
            memory_state.index_line_count
        )
    } else {
        String::new()
    };

    let context_document = format!(
        "# Current Memory State\n\n{}{}\n---\n\n# New Sessions Since Last Dream\n\n{}",
        memory_context, index_note, session_context
    );

    let use_structured = is_claude_cmd(model_cmd);

    // Split model command: "claude -p" -> ["claude", "-p"]
    let parts: Vec<&str> = model_cmd.split_whitespace().collect();
    if parts.is_empty() {
        anyhow::bail!("Empty model command");
    }
    let cmd = parts[0];
    let args = &parts[1..];

    let mut command = Command::new(cmd);
    command
        .args(args)
        .arg(SYSTEM_PROMPT)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if use_structured {
        command
            .arg("--output-format")
            .arg("json")
            .arg("--json-schema")
            .arg(DREAM_RESPONSE_SCHEMA);
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("Failed to spawn '{}'. Is it in PATH?", cmd))?;

    child
        .stdin
        .take()
        .expect("stdin not captured")
        .write_all(context_document.as_bytes())
        .context("Failed to write to model stdin")?;

    let output = child
        .wait_with_output()
        .context("Failed to wait for model process")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Model process failed (exit {}): {}", output.status, stderr.trim());
    }

    let response_text = String::from_utf8(output.stdout)
        .context("Model output is not valid UTF-8")?;

    if use_structured {
        parse_structured_response(&response_text)
    } else {
        parse_response(&response_text)
    }
}

/// Parse a structured response from `claude -p --output-format json --json-schema`.
/// The response is a JSON envelope with the actual data in `structured_output`.
fn parse_structured_response(text: &str) -> Result<DreamResponse> {
    let trimmed = text.trim();

    let envelope: ClaudeJsonEnvelope = serde_json::from_str(trimmed)
        .context("Failed to parse claude JSON envelope")?;

    if envelope.is_error {
        let msg = envelope.result.unwrap_or_else(|| "unknown error".to_string());
        anyhow::bail!("Claude returned an error: {}", msg);
    }

    // When --json-schema is used, the structured data is in structured_output
    if let Some(structured) = envelope.structured_output {
        let response: DreamResponse = serde_json::from_value(structured)
            .context("Failed to parse structured_output as DreamResponse")?;
        return Ok(response);
    }

    // Fallback: try parsing the result field as JSON (shouldn't happen with --json-schema, but be defensive)
    if let Some(result_text) = &envelope.result {
        return parse_response(result_text);
    }

    anyhow::bail!("Claude JSON envelope contained neither structured_output nor result")
}

/// Try multiple strategies to extract JSON from the AI response
fn parse_response(text: &str) -> Result<DreamResponse> {
    let trimmed = text.trim();

    // Strategy 1: Direct JSON parse
    if let Ok(response) = serde_json::from_str::<DreamResponse>(trimmed) {
        return Ok(response);
    }

    // Strategy 2: Extract from ```json ... ``` fences
    if let Some(start) = trimmed.find("```json") {
        let after_fence = &trimmed[start + 7..];
        if let Some(end) = after_fence.find("```") {
            let json_str = after_fence[..end].trim();
            if let Ok(response) = serde_json::from_str::<DreamResponse>(json_str) {
                return Ok(response);
            }
        }
    }

    // Strategy 3: Find first { and last } for brace matching
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if start < end {
            let json_str = &trimmed[start..=end];
            if let Ok(response) = serde_json::from_str::<DreamResponse>(json_str) {
                return Ok(response);
            }
        }
    }

    // All strategies failed — save raw response for debugging
    let debug_path = dirs::home_dir()
        .map(|h| h.join(".local/share/continuum-dream/last-failed-response.txt"))
        .unwrap_or_else(|| "/tmp/continuum-dream-failed-response.txt".into());

    if let Some(parent) = debug_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&debug_path, text);

    anyhow::bail!(
        "Failed to parse AI response as JSON. Raw response saved to {}",
        debug_path.display()
    )
}
