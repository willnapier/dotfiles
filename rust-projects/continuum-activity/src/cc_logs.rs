use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;

use crate::types::CcSession;

/// Index file schema.
#[derive(Debug, Deserialize)]
struct SessionIndex {
    entries: Vec<IndexEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexEntry {
    session_id: String,
    full_path: String,
    created: String,
    modified: String,
}

/// Parse the sessions-index.json and return sessions whose date range overlaps `target_date`.
fn relevant_sessions(index_path: &Path, target_date: NaiveDate) -> Result<Vec<IndexEntry>> {
    let content = std::fs::read_to_string(index_path)
        .with_context(|| format!("Failed to read {}", index_path.display()))?;
    let index: SessionIndex =
        serde_json::from_str(&content).context("Failed to parse sessions-index.json")?;

    let mut relevant = Vec::new();
    for entry in index.entries {
        let created = DateTime::parse_from_rfc3339(&entry.created)
            .map(|dt| dt.with_timezone(&Utc).date_naive())
            .unwrap_or(NaiveDate::MIN);
        let modified = DateTime::parse_from_rfc3339(&entry.modified)
            .map(|dt| dt.with_timezone(&Utc).date_naive())
            .unwrap_or(NaiveDate::MIN);

        if created <= target_date && modified >= target_date {
            relevant.push(entry);
        }
    }
    Ok(relevant)
}

/// Parse a single JSONL session file and extract activity for `target_date`.
fn parse_session_jsonl(
    path: &Path,
    target_date: NaiveDate,
    verbose: bool,
) -> Result<Option<CcSession>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut session_id = String::new();
    let mut slug = String::new();
    let mut start_time: Option<DateTime<Utc>> = None;
    let mut end_time: Option<DateTime<Utc>> = None;
    let mut skills: Vec<String> = Vec::new();
    let mut files_modified: BTreeMap<String, u32> = BTreeMap::new();
    let mut tool_usage: BTreeMap<String, u32> = BTreeMap::new();
    let mut user_messages: Vec<(DateTime<Utc>, String)> = Vec::new();
    let mut has_activity = false;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.is_empty() {
            continue;
        }

        let entry: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Get timestamp and check it's on target date
        let ts_str = match entry.get("timestamp").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => continue,
        };
        let ts = match DateTime::parse_from_rfc3339(ts_str) {
            Ok(dt) => dt.with_timezone(&Utc),
            Err(_) => continue,
        };
        if ts.date_naive() != target_date {
            continue;
        }

        has_activity = true;

        // Track session metadata from first matching entry
        if session_id.is_empty() {
            if let Some(sid) = entry.get("sessionId").and_then(|v| v.as_str()) {
                session_id = sid.to_string();
            }
            if let Some(s) = entry.get("slug").and_then(|v| v.as_str()) {
                slug = s.to_string();
            }
        }

        // Track time range
        match &start_time {
            None => start_time = Some(ts),
            Some(st) if ts < *st => start_time = Some(ts),
            _ => {}
        }
        match &end_time {
            None => end_time = Some(ts),
            Some(et) if ts > *et => end_time = Some(ts),
            _ => {}
        }

        let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match entry_type {
            "user" => {
                // Skip tool result entries (they have toolUseResult or sourceToolAssistantUUID)
                if entry.get("toolUseResult").is_some()
                    || entry.get("sourceToolAssistantUUID").is_some()
                {
                    continue;
                }

                // Extract user messages
                if let Some(content) = entry.pointer("/message/content") {
                    if let Some(text) = content.as_str() {
                        if is_real_user_message(text) {
                            let msg = if verbose {
                                text.to_string()
                            } else {
                                truncate_message(text, 120)
                            };
                            if !msg.is_empty() {
                                user_messages.push((ts, msg));
                            }
                        }
                    } else if let Some(arr) = content.as_array() {
                        for block in arr {
                            if block.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                                continue;
                            }
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                if is_real_user_message(text) {
                                    let msg = if verbose {
                                        text.to_string()
                                    } else {
                                        truncate_message(text, 120)
                                    };
                                    if !msg.is_empty() {
                                        user_messages.push((ts, msg));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            "assistant" => {
                // Extract tool uses from assistant messages
                if let Some(content) = entry.pointer("/message/content").and_then(|v| v.as_array())
                {
                    for block in content {
                        if block.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
                            continue;
                        }
                        let tool_name = block
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");

                        *tool_usage.entry(tool_name.to_string()).or_insert(0) += 1;

                        // Track skills
                        if tool_name == "Skill" {
                            if let Some(skill) =
                                block.pointer("/input/skill").and_then(|v| v.as_str())
                            {
                                if !skills.contains(&skill.to_string()) {
                                    skills.push(skill.to_string());
                                }
                            }
                        }

                        // Track file modifications
                        if tool_name == "Edit" || tool_name == "Write" {
                            if let Some(fp) =
                                block.pointer("/input/file_path").and_then(|v| v.as_str())
                            {
                                *files_modified.entry(fp.to_string()).or_insert(0) += 1;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if !has_activity {
        return Ok(None);
    }

    // Skip sessions with no meaningful content
    if user_messages.is_empty() && tool_usage.is_empty() && files_modified.is_empty() {
        return Ok(None);
    }

    Ok(Some(CcSession {
        session_id,
        slug,
        start_time,
        end_time,
        skills,
        files_modified,
        tool_usage,
        user_messages,
    }))
}

/// Truncate a message to `max_len` characters, adding "..." if truncated.
fn truncate_message(s: &str, max_len: usize) -> String {
    // Take only the first line
    let first_line = s.lines().next().unwrap_or("");
    let trimmed = first_line.trim();
    if trimmed.chars().count() <= max_len {
        trimmed.to_string()
    } else {
        let truncated: String = trimmed.chars().take(max_len).collect();
        format!("{truncated}...")
    }
}

/// Find JSONL files not in the index that might contain activity for `target_date`.
/// Falls back to checking filesystem mtime since the index can be stale.
fn unindexed_jsonl_files(
    cc_dir: &Path,
    indexed_paths: &[String],
    target_date: NaiveDate,
) -> Vec<std::path::PathBuf> {
    let mut extra = Vec::new();
    let entries = match std::fs::read_dir(cc_dir) {
        Ok(e) => e,
        Err(_) => return extra,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if !name.ends_with(".jsonl") {
            continue;
        }
        let path_str = path.to_string_lossy().to_string();
        if indexed_paths.contains(&path_str) {
            continue;
        }
        // Check if file was modified on or after target_date
        if let Ok(meta) = path.metadata() {
            if let Ok(mtime) = meta.modified() {
                let mtime_dt: DateTime<Utc> = mtime.into();
                if mtime_dt.date_naive() >= target_date {
                    extra.push(path);
                }
            }
        }
    }
    extra
}

/// Check if a message is a real user prompt (not system/meta injection).
fn is_real_user_message(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    // Skip system-injected messages
    if trimmed.starts_with('<') {
        // Allow messages that start with < but aren't XML-like tags
        if trimmed.starts_with("<local-command")
            || trimmed.starts_with("<command-name>")
            || trimmed.starts_with("<task-notification")
            || trimmed.starts_with("<system-reminder")
            || trimmed.starts_with("<user-prompt-submit-hook")
        {
            return false;
        }
    }
    // Skip "[Request interrupted by user...]" meta messages
    if trimmed.starts_with("[Request interrupted") {
        return false;
    }
    true
}

/// Top-level function: find all CC sessions active on `target_date`.
pub fn extract_cc_sessions(target_date: NaiveDate, verbose: bool) -> Result<Vec<CcSession>> {
    let cc_dir = dirs::home_dir()
        .context("No home directory")?
        .join(".claude/projects/-home-will");

    if !cc_dir.exists() {
        return Ok(Vec::new());
    }

    // Gather paths from the index
    let index_path = cc_dir.join("sessions-index.json");
    let entries = if index_path.exists() {
        relevant_sessions(&index_path, target_date)?
    } else {
        Vec::new()
    };

    let mut sessions = Vec::new();
    let indexed_paths: Vec<String> = entries.iter().map(|e| e.full_path.clone()).collect();

    for entry in &entries {
        let path = Path::new(&entry.full_path);
        if !path.exists() {
            continue;
        }
        match parse_session_jsonl(path, target_date, verbose) {
            Ok(Some(session)) => sessions.push(session),
            Ok(None) => {}
            Err(e) => {
                eprintln!("Warning: failed to parse {}: {}", entry.session_id, e);
            }
        }
    }

    // Also scan unindexed JSONL files (index can be stale)
    for path in unindexed_jsonl_files(&cc_dir, &indexed_paths, target_date) {
        match parse_session_jsonl(&path, target_date, verbose) {
            Ok(Some(session)) => sessions.push(session),
            Ok(None) => {}
            Err(e) => {
                eprintln!(
                    "Warning: failed to parse {}: {}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    e
                );
            }
        }
    }

    // Sort by start time
    sessions.sort_by(|a, b| a.start_time.cmp(&b.start_time));

    // Deduplicate: multiple JSONL files can contain entries for the same sessionId.
    // Merge sessions with the same sessionId.
    let mut merged: Vec<CcSession> = Vec::new();
    for session in sessions {
        if let Some(existing) = merged.iter_mut().find(|s| s.session_id == session.session_id) {
            // Merge time range
            if let Some(st) = session.start_time {
                existing.start_time = Some(match existing.start_time {
                    Some(est) if st < est => st,
                    Some(est) => est,
                    None => st,
                });
            }
            if let Some(et) = session.end_time {
                existing.end_time = Some(match existing.end_time {
                    Some(eet) if et > eet => et,
                    Some(eet) => eet,
                    None => et,
                });
            }
            // Merge skills
            for skill in session.skills {
                if !existing.skills.contains(&skill) {
                    existing.skills.push(skill);
                }
            }
            // Merge file edits
            for (path, count) in session.files_modified {
                *existing.files_modified.entry(path).or_insert(0) += count;
            }
            // Merge tool usage
            for (tool, count) in session.tool_usage {
                *existing.tool_usage.entry(tool).or_insert(0) += count;
            }
            // Merge user messages
            existing.user_messages.extend(session.user_messages);
            existing.user_messages.sort_by_key(|(ts, _)| *ts);
        } else {
            merged.push(session);
        }
    }

    Ok(merged)
}
