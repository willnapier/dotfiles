use anyhow::{Context, Result};
use chrono::{NaiveDate, Utc};
use std::fs;
use std::path::PathBuf;

use crate::types::DreamState;

fn state_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("No home directory")?;
    Ok(home.join(".local/share/continuum-dream"))
}

pub fn state_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("state.json"))
}

pub fn load() -> Result<DreamState> {
    let path = state_path()?;
    if !path.exists() {
        return Ok(DreamState::default());
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read state file: {}", path.display()))?;
    let state: DreamState = serde_json::from_str(&content)
        .with_context(|| "Failed to parse state file")?;
    Ok(state)
}

pub fn save(state: &DreamState) -> Result<()> {
    let dir = state_dir()?;
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create state dir: {}", dir.display()))?;
    let path = state_path()?;
    let content = serde_json::to_string_pretty(state)?;
    fs::write(&path, content)
        .with_context(|| format!("Failed to write state file: {}", path.display()))?;
    Ok(())
}

/// Record a completed dream run
pub fn record_dream(
    state: &mut DreamState,
    new_sessions: &[String],
    summary: &str,
) -> Result<()> {
    state.last_dream_time = Some(Utc::now().to_rfc3339());
    state.total_dreams += 1;
    state.last_dream_summary = Some(summary.to_string());

    // Append new session paths
    state.sessions_processed.extend(new_sessions.iter().cloned());

    // Prune entries older than 90 days
    let cutoff = Utc::now().date_naive() - chrono::Duration::days(90);
    state.sessions_processed.retain(|path| {
        // Extract date from path: "vendor/YYYY-MM-DD/session-id"
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() >= 2 {
            if let Ok(date) = NaiveDate::parse_from_str(parts[1], "%Y-%m-%d") {
                return date >= cutoff;
            }
        }
        true // keep if we can't parse the date
    });

    state.last_session_count = state.sessions_processed.len();
    save(state)
}
