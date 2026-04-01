use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

use crate::orient;
use crate::types::{DreamResponse, MemoryState};

const SYSTEM_PROMPT: &str = r#"CRITICAL: You MUST respond with ONLY a raw JSON object. No markdown, no explanation, no preamble, no summary. Just the JSON object starting with { and ending with }.

You are a memory consolidation agent for a multi-vendor AI assistant system. Your job is to update a set of memory files based on new conversation sessions.

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

OUTPUT FORMAT:
Respond with ONLY a JSON object (no markdown fences, no explanation outside the JSON):
{
  "files_to_update": [{"filename": "existing_file.md", "content": "---\nname: ...\ndescription: ...\ntype: ...\n---\n\nbody...", "reason": "why"}],
  "files_to_create": [{"filename": "type_topic.md", "content": "---\nname: ...\ndescription: ...\ntype: ...\n---\n\nbody...", "reason": "why"}],
  "files_to_delete": [{"filename": "stale_file.md", "reason": "why"}],
  "memory_index": "full new MEMORY.md content OR the literal string UNCHANGED",
  "summary": "Human-readable summary of what changed and why"
}

If no changes are warranted, return:
{"files_to_update": [], "files_to_create": [], "files_to_delete": [], "memory_index": "UNCHANGED", "summary": "No consolidation needed."}"#;

/// Run the AI consolidation phase
pub fn run(
    model_cmd: &str,
    memory_state: &MemoryState,
    session_context: &str,
) -> Result<DreamResponse> {
    let memory_context = orient::format_memory_state(memory_state);

    let context_document = format!(
        "# Current Memory State\n\n{}\n---\n\n# New Sessions Since Last Dream\n\n{}",
        memory_context, session_context
    );

    // Split model command: "claude -p" -> ["claude", "-p"]
    let parts: Vec<&str> = model_cmd.split_whitespace().collect();
    if parts.is_empty() {
        anyhow::bail!("Empty model command");
    }
    let cmd = parts[0];
    let args = &parts[1..];

    let mut child = Command::new(cmd)
        .args(args)
        .arg(SYSTEM_PROMPT)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
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

    parse_response(&response_text)
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
