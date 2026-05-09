//! Activity-based mailcurator leadership — mailforge daemon side.
//!
//! Each machine writes ONLY its own per-machine record under
//! `~/Clinical/.mailforge-activity/<hostname>.json`. There's no shared
//! file, no contention, no consensus. The `mailcurator` binary reads
//! all records at run-time and acts only if its own machine is the
//! most-recently-active (with a small tiebreaker window). This module's
//! job is to bump the timestamp on every USER-DRIVEN HTTP request so
//! "I am here right now" is reflected before the next mailcurator tick.
//!
//! Critically, the activity signal must come from PRACTITIONER ACTIONS
//! only — not from automated mbsync pulls, not from polling, not from
//! background fetches. If automated traffic bumped activity, leadership
//! would yo-yo on whichever machine's cron fired last, missing the point
//! of "where is the practitioner actually sitting?". Wire this middleware
//! onto user-facing routes only (the `/mail/*` and `/api/*` subrouter);
//! keep `/healthz` and any background-poll endpoints out of its path.
//!
//! Mirrors `practiceforge::tm3_activity` (the TM3-side equivalent) and
//! `mailcurator::leader` (the read-and-decide side). The two activity
//! directories are deliberately separate:
//!   - `~/Clinical/.tm3-activity/`        TM3 capture leadership
//!   - `~/Clinical/.mailforge-activity/`  mailcurator + mailforge leadership
//!
//! Added 2026-05-09 as a sibling to TM3 task #18.

use chrono::Local;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

/// One write per this many minutes per machine. Avoids Syncthing churn
/// on chatty dashboard usage. Must agree with the constant in
/// `mailcurator::leader` — the two are independent for build-isolation
/// reasons (mailforge has no dep on mailcurator).
const DEBOUNCE_MINUTES: i64 = 5;

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ActivityRecord {
    machine_id: String,
    last_active_ts: chrono::DateTime<chrono::FixedOffset>,
    #[serde(default)]
    source: String,
}

/// In-memory debouncer so the per-request hook doesn't even read the
/// activity file in the common case. First request after process start
/// reads disk; subsequent requests within DEBOUNCE_MINUTES are a no-op.
static LAST_TOUCH: Mutex<Option<std::time::Instant>> = Mutex::new(None);

fn machine_id() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    if let Ok(content) = std::fs::read_to_string("/etc/hostname") {
        let h = content.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    if let Ok(out) = std::process::Command::new("hostname").output() {
        if out.status.success() {
            let h = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !h.is_empty() {
                return h;
            }
        }
    }
    "unknown-machine".to_string()
}

fn activity_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join("Clinical").join(".mailforge-activity"))
        .unwrap_or_else(|| PathBuf::from(".mailforge-activity"))
}

/// Bump this machine's activity record. Heavily debounced (in-memory +
/// on-disk checks) so the common case is a single mutex-acquire-and-release
/// with no I/O at all. Failures are silent — an activity bump miss should
/// never break a user request.
fn touch(source: &str) {
    {
        let cache = LAST_TOUCH.lock().unwrap();
        if let Some(last) = *cache {
            if last.elapsed() < std::time::Duration::from_secs((DEBOUNCE_MINUTES * 60) as u64) {
                return;
            }
        }
    }

    let dir = activity_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }

    let id = machine_id();
    let path = dir.join(format!("{id}.json"));

    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(existing) = serde_json::from_str::<ActivityRecord>(&content) {
            let age = Local::now().fixed_offset() - existing.last_active_ts;
            if age.num_minutes() < DEBOUNCE_MINUTES {
                *LAST_TOUCH.lock().unwrap() = Some(std::time::Instant::now());
                return;
            }
        }
    }

    let record = ActivityRecord {
        machine_id: id,
        last_active_ts: Local::now().fixed_offset(),
        source: source.to_string(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&record) {
        let _ = std::fs::write(&path, json);
    }

    *LAST_TOUCH.lock().unwrap() = Some(std::time::Instant::now());
}

/// axum middleware that bumps the activity record on every request that
/// flows through it. Wire ONLY onto user-facing routes (the `/mail/*`
/// and `/api/*` subrouter) — never onto `/healthz` (health-checker may
/// poll) or background-poll endpoints, since those signals would
/// misrepresent practitioner attention.
pub async fn middleware(
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    touch("mailforge");
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_id_is_nonempty() {
        let id = machine_id();
        assert!(!id.is_empty());
    }

    #[test]
    fn record_serializes_round_trip() {
        let r = ActivityRecord {
            machine_id: "test-host".to_string(),
            last_active_ts: chrono::DateTime::parse_from_rfc3339("2026-05-09T16:00:00+01:00").unwrap(),
            source: "test".to_string(),
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: ActivityRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.machine_id, "test-host");
        assert_eq!(back.source, "test");
    }
}
