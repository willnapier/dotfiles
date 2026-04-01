use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use crate::types::{DreamState, LogMessage, SessionMeta, SessionSummary};

/// Find all sessions in continuum-logs that haven't been processed yet
pub fn collect_sessions(
    state: &DreamState,
    since: Option<&str>,
) -> Result<Vec<SessionSummary>> {
    let base_dir = dirs::home_dir()
        .context("No home directory")?
        .join("Assistants/continuum-logs");

    if !base_dir.exists() {
        return Ok(Vec::new());
    }

    let processed: HashSet<&str> = state.sessions_processed.iter().map(|s| s.as_str()).collect();

    // Parse --since into a cutoff date
    let cutoff_date = since.and_then(|s| parse_since(s));

    let mut sessions = Vec::new();

    // Walk vendor directories
    for vendor_entry in fs::read_dir(&base_dir)?.flatten() {
        let vendor_dir = vendor_entry.path();
        if !vendor_dir.is_dir() {
            continue;
        }
        let vendor_name = vendor_entry.file_name().to_string_lossy().to_string();

        // Walk date directories
        for date_entry in fs::read_dir(&vendor_dir)?.flatten() {
            let date_dir = date_entry.path();
            if !date_dir.is_dir() {
                continue;
            }
            let date_str = date_entry.file_name().to_string_lossy().to_string();

            // Apply date cutoff
            if let Some(cutoff) = &cutoff_date {
                if date_str < *cutoff {
                    continue;
                }
            }

            // Walk session directories
            for session_entry in fs::read_dir(&date_dir)?.flatten() {
                let session_dir = session_entry.path();
                if !session_dir.is_dir() {
                    continue;
                }
                let session_name = session_entry.file_name().to_string_lossy().to_string();
                let relative_path = format!("{}/{}/{}", vendor_name, date_str, session_name);

                // Skip already-processed sessions
                if processed.contains(relative_path.as_str()) {
                    continue;
                }

                // Try to read session
                match read_session(&session_dir, &relative_path, &vendor_name, &date_str) {
                    Ok(summary) => sessions.push(summary),
                    Err(_) => continue, // skip malformed sessions
                }
            }
        }
    }

    // Sort by date (newest first)
    sessions.sort_by(|a, b| b.date.cmp(&a.date));

    Ok(sessions)
}

/// Count new (unprocessed) sessions without reading their content
pub fn count_new_sessions(state: &DreamState) -> Result<usize> {
    let base_dir = dirs::home_dir()
        .context("No home directory")?
        .join("Assistants/continuum-logs");

    if !base_dir.exists() {
        return Ok(0);
    }

    let processed: HashSet<&str> = state.sessions_processed.iter().map(|s| s.as_str()).collect();
    let mut count = 0;

    for vendor_entry in fs::read_dir(&base_dir)?.flatten() {
        let vendor_dir = vendor_entry.path();
        if !vendor_dir.is_dir() {
            continue;
        }
        let vendor_name = vendor_entry.file_name().to_string_lossy().to_string();

        for date_entry in fs::read_dir(&vendor_dir)?.flatten() {
            let date_dir = date_entry.path();
            if !date_dir.is_dir() {
                continue;
            }
            let date_str = date_entry.file_name().to_string_lossy().to_string();

            for session_entry in fs::read_dir(&date_dir)?.flatten() {
                if !session_entry.path().is_dir() {
                    continue;
                }
                let session_name = session_entry.file_name().to_string_lossy().to_string();
                let relative_path = format!("{}/{}/{}", vendor_name, date_str, session_name);

                if !processed.contains(relative_path.as_str()) {
                    // Verify it has session.json
                    if session_entry.path().join("session.json").exists() {
                        count += 1;
                    }
                }
            }
        }
    }

    Ok(count)
}

fn read_session(
    session_dir: &PathBuf,
    relative_path: &str,
    vendor_name: &str,
    date_str: &str,
) -> Result<SessionSummary> {
    let session_json = session_dir.join("session.json");
    let messages_jsonl = session_dir.join("messages.jsonl");

    let meta_content = fs::read_to_string(&session_json)?;
    let meta: SessionMeta = serde_json::from_str(&meta_content)?;

    let mut user_messages = Vec::new();
    let mut assistant_first_reply = String::new();
    let mut msg_count = 0;

    if messages_jsonl.exists() {
        let content = fs::read_to_string(&messages_jsonl)?;
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(msg) = serde_json::from_str::<LogMessage>(line) {
                msg_count += 1;
                match msg.role.as_str() {
                    "user" => {
                        // Skip noise: system reminders, very short messages
                        let trimmed = msg.content.trim();
                        if trimmed.len() > 10
                            && !trimmed.starts_with("<system-reminder>")
                            && !trimmed.starts_with("Thought for")
                        {
                            // Truncate to 300 chars
                            let truncated = if trimmed.len() > 300 {
                                format!("{}...", &trimmed[..300])
                            } else {
                                trimmed.to_string()
                            };
                            user_messages.push(truncated);
                        }
                    }
                    "assistant" => {
                        if assistant_first_reply.is_empty() {
                            let trimmed = msg.content.trim();
                            if trimmed.len() > 20 {
                                assistant_first_reply = if trimmed.len() > 300 {
                                    format!("{}...", &trimmed[..300])
                                } else {
                                    trimmed.to_string()
                                };
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(SessionSummary {
        id: meta.id,
        relative_path: relative_path.to_string(),
        assistant: vendor_name.to_string(),
        date: date_str.to_string(),
        start_time: meta.start_time,
        end_time: meta.end_time,
        message_count: if meta.message_count.unwrap_or(0) > 0 {
            meta.message_count.unwrap() as usize
        } else {
            msg_count
        },
        user_messages,
        assistant_first_reply,
        skills: meta.skills.unwrap_or_default(),
    })
}

/// Parse --since flag: "7d" -> date string 7 days ago, "24h" -> date string 1 day ago
fn parse_since(s: &str) -> Option<String> {
    let now = chrono::Utc::now().date_naive();
    if let Some(days_str) = s.strip_suffix('d') {
        if let Ok(days) = days_str.parse::<i64>() {
            let cutoff = now - chrono::Duration::days(days);
            return Some(cutoff.format("%Y-%m-%d").to_string());
        }
    }
    if let Some(hours_str) = s.strip_suffix('h') {
        if let Ok(hours) = hours_str.parse::<i64>() {
            let cutoff = now - chrono::Duration::hours(hours);
            return Some(cutoff.format("%Y-%m-%d").to_string());
        }
    }
    None
}

/// Format session summaries as a context string for the AI prompt
pub fn format_sessions(sessions: &[SessionSummary]) -> String {
    if sessions.is_empty() {
        return "No new sessions since last dream.\n".to_string();
    }

    let date_range = format!(
        "{} to {}",
        sessions.last().map(|s| s.date.as_str()).unwrap_or("?"),
        sessions.first().map(|s| s.date.as_str()).unwrap_or("?"),
    );

    let mut out = format!(
        "## New Sessions ({} sessions, {})\n\n",
        sessions.len(),
        date_range
    );

    for s in sessions {
        out.push_str(&format!("### {} | {}", s.assistant, s.date));
        if !s.skills.is_empty() {
            out.push_str(&format!(" | skills: {}", s.skills.join(", ")));
        }
        out.push_str(&format!(" | {} messages\n", s.message_count));

        // Show user messages (max 5)
        for (i, msg) in s.user_messages.iter().take(5).enumerate() {
            if i == 0 {
                out.push_str(&format!("User asked: {}\n", msg));
            } else {
                out.push_str(&format!("User also: {}\n", msg));
            }
        }
        if s.user_messages.len() > 5 {
            out.push_str(&format!(
                "({} more user messages omitted)\n",
                s.user_messages.len() - 5
            ));
        }

        if !s.assistant_first_reply.is_empty() {
            out.push_str(&format!("Assistant: {}\n", s.assistant_first_reply));
        }
        out.push('\n');
    }

    out
}
