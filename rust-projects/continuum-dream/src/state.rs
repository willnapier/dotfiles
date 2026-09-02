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
/// How far back dream looks. Sessions dated before this are neither
/// counted, gathered nor remembered as processed — they are simply outside
/// the horizon. Until 2026-09-02 only the *processed* list honoured it, so
/// 4,800 archive imports dated 2023 → 2026-05 stood as "new" forever while
/// anything that was processed fell off the list after 90 days.
pub const TRACKING_DAYS: i64 = 90;

pub fn tracking_cutoff() -> NaiveDate {
    Utc::now().date_naive() - chrono::Duration::days(TRACKING_DAYS)
}

/// True when a `YYYY-MM-DD` date directory is inside the horizon. An
/// unparseable name is kept (visible, not silently dropped).
pub fn within_horizon(date_str: &str, cutoff: NaiveDate) -> bool {
    match NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        Ok(d) => d >= cutoff,
        Err(_) => true,
    }
}

#[cfg(test)]
mod horizon_tests {
    use super::*;

    #[test]
    fn horizon_keeps_recent_drops_old_and_keeps_unparseable() {
        let cutoff = NaiveDate::from_ymd_opt(2026, 6, 4).unwrap();
        assert!(within_horizon("2026-06-04", cutoff));
        assert!(within_horizon("2026-09-02", cutoff));
        assert!(!within_horizon("2026-06-03", cutoff));
        assert!(!within_horizon("2023-04-01", cutoff));
        assert!(within_horizon("not-a-date", cutoff));
    }
}

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

    // Prune entries older than the tracking horizon (the same horizon the
    // gatherer applies, so a pruned session can never come back as "new").
    let cutoff = tracking_cutoff();
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
