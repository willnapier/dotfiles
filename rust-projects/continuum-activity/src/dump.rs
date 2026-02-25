use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct SessionMeta {
    id: String,
    assistant: String,
    start_time: Option<String>,
    end_time: Option<String>,
    message_count: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct Message {
    role: String,
    content: String,
}

struct SessionInfo {
    path: PathBuf,
    meta: SessionMeta,
}

pub fn dump_session(
    session_id: Option<&str>,
    last: bool,
    assistant_filter: Option<&str>,
) -> Result<()> {
    let base_dir = dirs::home_dir()
        .context("No home directory")?
        .join("Assistants/continuum-logs");

    if !base_dir.exists() {
        bail!("Continuum logs directory not found: {}", base_dir.display());
    }

    let session = if last {
        find_last_session(&base_dir, assistant_filter)?
    } else if let Some(id) = session_id {
        find_session_by_id(&base_dir, id)?
    } else {
        bail!("Specify --last or provide a session ID");
    };

    output_session(&session)
}

fn collect_sessions(base_dir: &Path, assistant_filter: Option<&str>) -> Result<Vec<SessionInfo>> {
    let mut all_sessions = Vec::new();

    for assistant_entry in std::fs::read_dir(base_dir)?.flatten() {
        let assistant_dir = assistant_entry.path();
        if !assistant_dir.is_dir() {
            continue;
        }

        let assistant_name = assistant_entry.file_name().to_string_lossy().to_string();
        if let Some(filter) = assistant_filter {
            if assistant_name != filter {
                continue;
            }
        }

        for date_entry in std::fs::read_dir(&assistant_dir)?.flatten() {
            let date_dir = date_entry.path();
            if !date_dir.is_dir() {
                continue;
            }

            for session_entry in std::fs::read_dir(&date_dir)?.flatten() {
                let session_dir = session_entry.path();
                let session_json = session_dir.join("session.json");
                if !session_json.exists() {
                    continue;
                }

                if let Ok(content) = std::fs::read_to_string(&session_json) {
                    if let Ok(meta) = serde_json::from_str::<SessionMeta>(&content) {
                        all_sessions.push(SessionInfo {
                            path: session_dir,
                            meta,
                        });
                    }
                }
            }
        }
    }

    Ok(all_sessions)
}

fn find_last_session(base_dir: &Path, assistant_filter: Option<&str>) -> Result<SessionInfo> {
    let mut sessions = collect_sessions(base_dir, assistant_filter)?;

    if sessions.is_empty() {
        bail!(
            "No sessions found{}",
            assistant_filter
                .map(|a| format!(" for assistant '{}'", a))
                .unwrap_or_default()
        );
    }

    // Sort by start_time descending, take most recent
    sessions.sort_by(|a, b| b.meta.start_time.cmp(&a.meta.start_time));

    Ok(sessions.into_iter().next().unwrap())
}

fn find_session_by_id(base_dir: &Path, id: &str) -> Result<SessionInfo> {
    let sessions = collect_sessions(base_dir, None)?;

    for session in sessions {
        // Match full ID or prefix
        if session.meta.id == id || session.meta.id.starts_with(id) {
            return Ok(session);
        }
        // Also match directory name
        if let Some(dir_name) = session.path.file_name().and_then(|n| n.to_str()) {
            if dir_name == id || dir_name.starts_with(id) {
                return Ok(session);
            }
        }
    }

    bail!("No session found matching ID '{}'", id);
}

fn output_session(session: &SessionInfo) -> Result<()> {
    let messages_path = session.path.join("messages.jsonl");
    if !messages_path.exists() {
        bail!("No messages file found for session {}", session.meta.id);
    }

    // Header on stderr (so stdout is clean for piping)
    let time_range = format_time_range(&session.meta.start_time, &session.meta.end_time);
    let msg_count = session
        .meta
        .message_count
        .map(|c| format!(", {} messages", c))
        .unwrap_or_default();
    eprintln!("Session: {} | {}{}", session.meta.assistant, time_range, msg_count);

    // Messages to stdout
    let content = std::fs::read_to_string(&messages_path)?;
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }

        if let Ok(msg) = serde_json::from_str::<Message>(line) {
            let role_label = match msg.role.as_str() {
                "user" => "User",
                "assistant" => "Assistant",
                _ => &msg.role,
            };
            println!("[{}]\n{}\n", role_label, msg.content);
        }
    }

    Ok(())
}

fn format_time_range(start: &Option<String>, end: &Option<String>) -> String {
    let parse = |s: &str| -> Option<String> {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
    };

    match (
        start.as_deref().and_then(parse),
        end.as_deref().and_then(parse),
    ) {
        (Some(s), Some(e)) => format!("{}–{}", s, e),
        (Some(s), None) => s,
        _ => "unknown time".to_string(),
    }
}
