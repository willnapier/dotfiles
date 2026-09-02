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
use std::collections::BTreeMap;
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
    /// Message ids whose `messages.get` failed after the retry loop
    /// gave up (429 / 5xx / quota / transport), keyed to the number of
    /// ticks that have tried them. Queued *before* the history
    /// checkpoint advances, so a failed fetch is deferred, never lost
    /// (system review F5: 781 ids had been skipped for good). Retried
    /// at the start of every tick; dropped after
    /// [`MAX_PENDING_ATTEMPTS`] with a warning.
    #[serde(default)]
    pub pending: BTreeMap<String, u32>,
}

/// Ticks a queued id is retried before it is abandoned (logged).
pub const MAX_PENDING_ATTEMPTS: u32 = 8;

/// A fetch error that will never succeed on retry: the message is gone
/// (deleted between listing and fetch) or the request is malformed.
/// Everything else — rate limits, 5xx, quota, transport — is transient.
pub fn is_permanent_fetch_error(message: &str) -> bool {
    message.contains("HTTP 404 on messages.get") || message.contains("HTTP 400 on messages.get")
}

/// What a retry of a pending id produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryOutcome {
    /// Written, deduped or filtered — the id is settled.
    Settled,
    /// Permanent error — the id is settled (gone), logged by the caller.
    Gone,
    /// Transient error — try again next tick.
    Failed,
}

impl State {
    /// Queue a failed fetch. Returns `true` if queued, `false` if the
    /// error is permanent and the id was dropped instead.
    pub fn queue_failure(&mut self, id: &str, error: &str) -> bool {
        if is_permanent_fetch_error(error) {
            self.pending.remove(id);
            return false;
        }
        self.pending.entry(id.to_string()).or_insert(0);
        true
    }

    /// Record the result of retrying a pending id. Returns `true` when
    /// the id has been given up on (attempt cap reached).
    pub fn note_retry(&mut self, id: &str, outcome: RetryOutcome) -> bool {
        match outcome {
            RetryOutcome::Settled | RetryOutcome::Gone => {
                self.pending.remove(id);
                false
            }
            RetryOutcome::Failed => {
                let attempts = self.pending.entry(id.to_string()).or_insert(0);
                *attempts += 1;
                if *attempts >= MAX_PENDING_ATTEMPTS {
                    self.pending.remove(id);
                    true
                } else {
                    false
                }
            }
        }
    }
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
            pending: BTreeMap::new(),
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

#[cfg(test)]
mod pending_tests {
    use super::*;

    #[test]
    fn pending_round_trips_and_defaults_empty() {
        let mut s = State::default();
        s.pending.insert("abc".into(), 2);
        let back: State = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.pending.get("abc"), Some(&2));
        let old: State = serde_json::from_str(r#"{"last_history_id":"1"}"#).unwrap();
        assert!(old.pending.is_empty(), "pre-F5 state files load with an empty queue");
    }

    #[test]
    fn transient_failures_queue_and_permanent_ones_do_not() {
        let mut s = State::default();
        assert!(s.queue_failure("t1", "messages.get id=t1: 503 server error on messages.get (attempt 15/15): backend"));
        assert!(s.queue_failure("t2", "messages.get id=t2: 429 rate-limited on messages.get (attempt 15/15): quota"));
        assert!(s.queue_failure("t3", "messages.get id=t3: HTTP GET messages.get: connection reset"));
        assert!(!s.queue_failure("gone", "messages.get id=gone: HTTP 404 on messages.get: Requested entity was not found."));
        assert_eq!(s.pending.len(), 3);
        assert!(!s.pending.contains_key("gone"));
        // Re-queueing an id keeps its attempt count.
        s.pending.insert("t1".into(), 3);
        s.queue_failure("t1", "503 server error on messages.get");
        assert_eq!(s.pending["t1"], 3);
    }

    #[test]
    fn retries_settle_or_are_abandoned_after_the_cap() {
        let mut s = State::default();
        s.queue_failure("a", "503 server error on messages.get");
        s.queue_failure("b", "503 server error on messages.get");
        s.queue_failure("c", "503 server error on messages.get");
        assert!(!s.note_retry("a", RetryOutcome::Settled));
        assert!(!s.note_retry("b", RetryOutcome::Gone));
        assert_eq!(s.pending.keys().collect::<Vec<_>>(), vec!["c"]);
        for attempt in 1..MAX_PENDING_ATTEMPTS {
            assert!(!s.note_retry("c", RetryOutcome::Failed), "attempt {attempt} keeps it queued");
            assert_eq!(s.pending["c"], attempt);
        }
        assert!(s.note_retry("c", RetryOutcome::Failed), "cap reached → abandoned");
        assert!(s.pending.is_empty());
    }
}
