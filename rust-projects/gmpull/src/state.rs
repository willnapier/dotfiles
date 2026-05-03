//! Checkpoint state — atomic JSON file at platform config dir.
//!
//! Mac: `~/Library/Application Support/gmpull/state.json`
//! Linux: `~/.config/gmpull/state.json`
//!
//! Saved every N messages so a crash mid-pull resumes cleanly.
//! Atomicity is the same trick as the maildir writer: write to
//! `state.json.tmp`, `rename(tmp → final)`.
//!
//! The state contains:
//!  - `last_page_token` — the most recent `nextPageToken` we *finished
//!    processing*. On resume we re-fetch this same page (which is
//!    cheap; tokens are valid for a few hours and pages are 500
//!    items max) and skip messages that already exist in the maildir.
//!  - `messages_pulled` — running total written this session and
//!    historically.
//!  - `last_history_id` — captured at start of pull from
//!    `users.getProfile`. Phase 3 uses this for incremental sync.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct State {
    /// nextPageToken returned by the most recently completed page.
    /// `None` after a clean full pull (we hit the end of the list).
    #[serde(default)]
    pub last_page_token: Option<String>,
    /// Total messages we've successfully written across all sessions.
    #[serde(default)]
    pub messages_pulled: u64,
    /// `historyId` captured at the start of the most recent pull.
    /// Reserved for Phase 3 (`users.history.list`-based incremental).
    #[serde(default)]
    pub last_history_id: Option<String>,
}

/// Resolve the state file path. Always under `gmpull/` inside the
/// platform's config dir.
pub fn state_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("locating config dir")?;
    Ok(base.join("gmpull").join("state.json"))
}

/// Load state from disk. Returns `Default` if no file exists yet.
pub async fn load() -> Result<State> {
    let path = state_path()?;
    if !path.exists() {
        return Ok(State::default());
    }
    let body = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("reading {}", path.display()))?;
    let s: State = serde_json::from_str(&body)
        .with_context(|| format!("parsing state JSON at {}", path.display()))?;
    Ok(s)
}

/// Save state atomically.
pub async fn save(state: &State) -> Result<()> {
    let path = state_path()?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(state).context("serialising state")?;
    tokio::fs::write(&tmp, body)
        .await
        .with_context(|| format!("writing {}", tmp.display()))?;
    tokio::fs::rename(&tmp, &path)
        .await
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Convenience: like [`save`] but logs and swallows failures so a
/// disk burp mid-pull doesn't abort the whole session.
pub async fn save_lossy(state: &State) {
    if let Err(e) = save(state).await {
        tracing::warn!(error = %e, "checkpoint save failed");
    }
}

/// Path to the maildir, with `~` expanded.
pub fn default_maildir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("locating home dir")?;
    Ok(home.join("Mail").join("gmail-rs"))
}

/// Helper used at startup to make sure the state directory's parent
/// exists. Separate from `save` because we want to fail fast at boot
/// rather than at first checkpoint.
pub async fn ensure_state_dir() -> Result<()> {
    let p = state_path()?;
    if let Some(parent) = p.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    Ok(())
}

/// Wrapper used to avoid pulling the path twice.
pub fn _state_dir() -> Result<&'static Path> {
    // Not currently used — placeholder if we add a `--state-dir`
    // flag later. Underscore prefix silences unused warnings.
    unreachable!("placeholder")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_state_json() {
        let s = State {
            last_page_token: Some("ABC".to_string()),
            messages_pulled: 12345,
            last_history_id: Some("999".to_string()),
        };
        let body = serde_json::to_string(&s).unwrap();
        let back: State = serde_json::from_str(&body).unwrap();
        assert_eq!(back.last_page_token.as_deref(), Some("ABC"));
        assert_eq!(back.messages_pulled, 12345);
        assert_eq!(back.last_history_id.as_deref(), Some("999"));
    }

    #[test]
    fn missing_fields_default() {
        let s: State = serde_json::from_str("{}").unwrap();
        assert!(s.last_page_token.is_none());
        assert_eq!(s.messages_pulled, 0);
    }
}
