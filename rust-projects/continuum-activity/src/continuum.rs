use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::Deserialize;

use crate::types::ContinuumSession;

#[derive(Debug, Deserialize)]
struct SessionMeta {
    id: String,
    assistant: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    start_time: Option<String>,
    #[serde(default)]
    end_time: Option<String>,
    #[serde(default)]
    message_count: Option<u32>,
}

/// Scan the Continuum archive for sessions on `target_date`.
pub fn extract_continuum_sessions(target_date: NaiveDate) -> Result<Vec<ContinuumSession>> {
    let base_dir = dirs::home_dir()
        .context("No home directory")?
        .join("Assistants/continuum-logs");

    if !base_dir.exists() {
        return Ok(Vec::new());
    }

    let date_str = target_date.format("%Y-%m-%d").to_string();
    let mut sessions = Vec::new();

    // Iterate over each assistant directory
    let entries = std::fs::read_dir(&base_dir)
        .with_context(|| format!("Failed to read {}", base_dir.display()))?;

    for entry in entries.flatten() {
        let assistant_dir = entry.path();
        if !assistant_dir.is_dir() {
            continue;
        }
        let assistant_name = entry.file_name().to_string_lossy().to_string();

        // Skip claude-code — those are already covered by CC log parsing
        if assistant_name == "claude-code" {
            continue;
        }

        let date_dir = assistant_dir.join(&date_str);
        if !date_dir.exists() {
            continue;
        }

        // Each subdirectory in the date dir is a session
        let session_dirs = std::fs::read_dir(&date_dir)
            .with_context(|| format!("Failed to read {}", date_dir.display()))?;

        for session_entry in session_dirs.flatten() {
            let session_dir = session_entry.path();
            if !session_dir.is_dir() {
                continue;
            }

            let session_json = session_dir.join("session.json");
            if !session_json.exists() {
                continue;
            }

            match std::fs::read_to_string(&session_json) {
                Ok(content) => match serde_json::from_str::<SessionMeta>(&content) {
                    Ok(meta) => {
                        sessions.push(ContinuumSession {
                            assistant: meta.assistant,
                            session_id: meta.id,
                            title: meta.title,
                            start_time: meta.start_time,
                            end_time: meta.end_time,
                            message_count: meta.message_count,
                        });
                    }
                    Err(e) => {
                        eprintln!(
                            "Warning: failed to parse {}: {}",
                            session_json.display(),
                            e
                        );
                    }
                },
                Err(e) => {
                    eprintln!(
                        "Warning: failed to read {}: {}",
                        session_json.display(),
                        e
                    );
                }
            }
        }
    }

    // Sort by assistant name, then start_time
    sessions.sort_by(|a, b| {
        a.assistant
            .cmp(&b.assistant)
            .then_with(|| a.start_time.cmp(&b.start_time))
    });

    Ok(sessions)
}
