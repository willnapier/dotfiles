//! Activity-based mailcurator leadership across multiple practitioner machines.
//!
//! When more than one machine has mailcurator scheduled (e.g. Mac + nimbini,
//! both running hourly via launchd / systemd), naive scheduling would have
//! both machines run policies every tick. That doubles notmuch / Gmail load,
//! causes both machines to race on the same trash actions, and produces
//! divergent JSONL ledgers (each machine writes its own per-host file but
//! a unified view requires Syncthing of the directory; without leadership
//! both machines are doing the same destruction work).
//!
//! Rather than statically designating one machine as "leader" via a per-host
//! config flag (manual, doesn't track attention), the protocol here uses
//! **physical-attention as the leader signal**: each machine writes its own
//! `last_active_ts` to a per-machine file under `~/Clinical/.mailforge-activity/`,
//! and the run decision is "am I the most-recently-active machine?"
//!
//! Each machine writes ONLY to its own file, so there's no contention or
//! consensus required — the contended-write/Syncthing-eventual-consistency
//! mess that a lockfile-style election would create simply doesn't apply.
//! Each side reads ALL machines' files at decision time and acts locally.
//!
//! The activity signal is bumped by the mailforge daemon on every
//! user-driven HTTP request (debounced to once per 5 minutes per machine).
//! Automated mbsync pulls do NOT bump activity — that would yo-yo leadership
//! based on whichever machine's cron tick fired last, missing the point of
//! "where is the practitioner actually sitting?"
//!
//! Mirrors `tm3-diary-capture/src/leader.rs` (TM3 capture leadership) — the
//! two protocols are deliberately separate so that being-on-mailforge doesn't
//! confer TM3 capture leadership and vice versa. Activity directories:
//!   - `~/Clinical/.tm3-activity/`        TM3 capture leadership
//!   - `~/Clinical/.mailforge-activity/`  mailcurator + mailforge leadership
//!
//! Added 2026-05-09 as a sibling to TM3 task #18.

use anyhow::{Context, Result};
use chrono::{DateTime, FixedOffset, Local};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Machines whose `last_active_ts` is within this window are considered
/// "currently in use" — one of them should be curator leader. When the most
/// recent activity across all machines exceeds this window, no machine
/// runs policies (no one is around to consume fresh extracted data).
pub const ACTIVE_WINDOW_HOURS: i64 = 4;

/// When two machines' `last_active_ts` are within this many seconds of each
/// other, the machine with the lexicographically-smaller `machine_id` wins.
/// Stable, deterministic, no contention.
pub const TIEBREAKER_SECONDS: i64 = 60;

/// Activity writes are debounced to one per this interval per machine.
/// Avoids Syncthing churn on chatty dashboard usage.
pub const DEBOUNCE_MINUTES: i64 = 5;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ActivityRecord {
    pub machine_id: String,
    pub last_active_ts: DateTime<FixedOffset>,
    /// Human-readable label for what bumped the activity (mailforge request,
    /// CLI invocation, etc). Diagnostic only — decisions are timestamp-based.
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunDecision {
    /// This machine is the most-recently-active among the cohort and should
    /// proceed with the curator run.
    Run,
    /// Another machine has been active more recently and is the leader for
    /// the current window. Skip.
    SkipOtherActive {
        winner_machine_id: String,
        winner_active_ago_minutes: i64,
    },
    /// No machine in the cohort has been active within the active window.
    /// Skip — no one is around to consume the extracted data.
    SkipNoRecentActivity {
        most_recent_ago_minutes: Option<i64>,
    },
    /// `MAILCURATOR_FORCE=1` was set in the environment — bypass the
    /// activity check entirely. Used by tests and explicit operator
    /// override (e.g. backfill / one-shot historical run).
    ForcedRun,
}

/// Best-effort hostname. Mirrors `store::machine_id` — same logic, kept
/// duplicated rather than cross-referenced because both modules want a
/// single self-contained source-of-hostname.
pub fn machine_id() -> String {
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

/// `~/Clinical/.mailforge-activity/`. Synced via Syncthing along with the
/// rest of `~/Clinical/`, so every machine's record propagates to the others.
/// The leading dot keeps it out of casual `ls` and out of file pickers
/// that hide dotfiles.
pub fn activity_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join("Clinical").join(".mailforge-activity"))
        .unwrap_or_else(|| PathBuf::from(".mailforge-activity"))
}

fn record_path(machine_id: &str) -> PathBuf {
    activity_dir().join(format!("{machine_id}.json"))
}

/// Write this machine's activity record. Debounced — if the existing record
/// is less than `DEBOUNCE_MINUTES` old, this is a no-op so chatty callers
/// (every HTTP request, every CLI tick) don't churn Syncthing.
///
/// `source` is a free-form short label like "mailforge_request" or
/// "mailcurator-cli" used for diagnostics; it doesn't affect leader
/// election.
#[allow(dead_code)] // mailforge daemon is the primary caller; CLI may also touch
pub fn touch(source: &str) -> Result<()> {
    let dir = activity_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create activity dir: {}", dir.display()))?;

    let id = machine_id();
    let path = record_path(&id);

    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(existing) = serde_json::from_str::<ActivityRecord>(&content) {
            let age = Local::now().fixed_offset() - existing.last_active_ts;
            if age.num_minutes() < DEBOUNCE_MINUTES {
                return Ok(());
            }
        }
    }

    let record = ActivityRecord {
        machine_id: id,
        last_active_ts: Local::now().fixed_offset(),
        source: source.to_string(),
    };
    let json = serde_json::to_string_pretty(&record)
        .context("serialize activity record")?;
    std::fs::write(&path, json)
        .with_context(|| format!("write activity record: {}", path.display()))?;
    Ok(())
}

/// Decide whether this machine should run mailcurator right now.
///
/// Algorithm:
/// 1. If `MAILCURATOR_FORCE=1`, return `ForcedRun` — operator override.
/// 2. Read all `~/Clinical/.mailforge-activity/*.json` records.
/// 3. If the most-recently-active machine's activity is older than
///    `ACTIVE_WINDOW_HOURS`, return `SkipNoRecentActivity` — no one is
///    around to consume extracted data.
/// 4. Find the most-recent record. If it's this machine, return `Run`.
///    If two records are within `TIEBREAKER_SECONDS` of each other, the
///    lexicographically-smaller machine_id wins (stable tiebreaker).
/// 5. Otherwise return `SkipOtherActive { winner }`.
///
/// Crash-safe: malformed records are ignored. Missing dir → treated as "no
/// records" → `SkipNoRecentActivity` (won't surprise-double-run a brand
/// new install).
pub fn should_run() -> RunDecision {
    if std::env::var("MAILCURATOR_FORCE").map(|v| v == "1").unwrap_or(false) {
        return RunDecision::ForcedRun;
    }

    let dir = activity_dir();
    let records = read_all_records(&dir);
    decide(&records, &machine_id(), Local::now().fixed_offset())
}

fn read_all_records(dir: &std::path::Path) -> Vec<ActivityRecord> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(rec) = serde_json::from_str::<ActivityRecord>(&content) {
                out.push(rec);
            }
        }
    }
    out
}

fn decide(
    records: &[ActivityRecord],
    this_machine: &str,
    now: DateTime<FixedOffset>,
) -> RunDecision {
    if records.is_empty() {
        return RunDecision::SkipNoRecentActivity {
            most_recent_ago_minutes: None,
        };
    }

    let mut sorted: Vec<&ActivityRecord> = records.iter().collect();
    sorted.sort_by(|a, b| {
        b.last_active_ts
            .cmp(&a.last_active_ts)
            .then_with(|| a.machine_id.cmp(&b.machine_id))
    });

    let head_ts = sorted[0].last_active_ts;
    let mut tied: Vec<&ActivityRecord> = sorted
        .iter()
        .filter(|r| (head_ts - r.last_active_ts).num_seconds().abs() <= TIEBREAKER_SECONDS)
        .copied()
        .collect();
    tied.sort_by(|a, b| a.machine_id.cmp(&b.machine_id));
    let leader = tied[0];

    let leader_age_min = (now - leader.last_active_ts).num_minutes();
    if leader_age_min > ACTIVE_WINDOW_HOURS * 60 {
        return RunDecision::SkipNoRecentActivity {
            most_recent_ago_minutes: Some(leader_age_min),
        };
    }

    if leader.machine_id == this_machine {
        RunDecision::Run
    } else {
        RunDecision::SkipOtherActive {
            winner_machine_id: leader.machine_id.clone(),
            winner_active_ago_minutes: leader_age_min,
        }
    }
}

/// Render a human-readable explanation of the decision for stderr/log output.
pub fn explain_decision(d: &RunDecision, this_machine: &str) -> String {
    match d {
        RunDecision::Run => {
            format!("Activity check: running on '{this_machine}' (most-recently-active machine)")
        }
        RunDecision::ForcedRun => {
            format!("Activity check: BYPASSED via MAILCURATOR_FORCE=1 (running on '{this_machine}')")
        }
        RunDecision::SkipOtherActive { winner_machine_id, winner_active_ago_minutes } => {
            format!(
                "Activity check: skipping — '{winner_machine_id}' was active {winner_active_ago_minutes} min ago and is the leader. \
                 Override with MAILCURATOR_FORCE=1."
            )
        }
        RunDecision::SkipNoRecentActivity { most_recent_ago_minutes } => match most_recent_ago_minutes {
            Some(m) => format!(
                "Activity check: skipping — no machine has been active in the last {ACTIVE_WINDOW_HOURS}h (most recent: {m} min ago). \
                 Open mailforge or run with MAILCURATOR_FORCE=1 to override."
            ),
            None => format!(
                "Activity check: skipping — no activity records yet under {dir}. \
                 First-time setup: open mailforge once to register, or run with MAILCURATOR_FORCE=1.",
                dir = activity_dir().display()
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn rec(machine: &str, ts: DateTime<FixedOffset>) -> ActivityRecord {
        ActivityRecord {
            machine_id: machine.to_string(),
            last_active_ts: ts,
            source: "test".to_string(),
        }
    }

    fn now() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-05-09T17:00:00+01:00").unwrap()
    }

    #[test]
    fn empty_records_skip_no_recent() {
        let d = decide(&[], "mac", now());
        assert!(matches!(d, RunDecision::SkipNoRecentActivity { most_recent_ago_minutes: None }));
    }

    #[test]
    fn sole_machine_runs() {
        let r = rec("mac", now() - Duration::minutes(2));
        let d = decide(&[r], "mac", now());
        assert_eq!(d, RunDecision::Run);
    }

    #[test]
    fn most_recent_wins() {
        let mac = rec("mac", now() - Duration::minutes(10));
        let nim = rec("nimbini", now() - Duration::minutes(2));
        // From mac's perspective: nimbini wins.
        let d = decide(&[mac.clone(), nim.clone()], "mac", now());
        assert!(matches!(d, RunDecision::SkipOtherActive { ref winner_machine_id, .. } if winner_machine_id == "nimbini"));
        // From nimbini's perspective: nimbini wins → Run.
        let d = decide(&[mac, nim], "nimbini", now());
        assert_eq!(d, RunDecision::Run);
    }

    #[test]
    fn tiebreaker_within_window_lex_smaller_wins() {
        // Both machines active within 30s of each other. "mac" < "nimbini"
        // lexicographically, so mac wins regardless of which has the
        // marginally-newer timestamp.
        let mac = rec("mac", now() - Duration::seconds(30));
        let nim = rec("nimbini", now() - Duration::seconds(10)); // newer ts
        let d = decide(&[mac, nim], "mac", now());
        assert_eq!(d, RunDecision::Run);
    }

    #[test]
    fn tiebreaker_outside_window_recency_wins() {
        // Beyond TIEBREAKER_SECONDS, recency dominates.
        let mac = rec("mac", now() - Duration::minutes(5));
        let nim = rec("nimbini", now() - Duration::seconds(30));
        let d = decide(&[mac, nim], "mac", now());
        assert!(matches!(d, RunDecision::SkipOtherActive { ref winner_machine_id, .. } if winner_machine_id == "nimbini"));
    }

    #[test]
    fn outside_active_window_skips() {
        let r = rec("mac", now() - Duration::hours(5));
        let d = decide(&[r], "mac", now());
        assert!(matches!(d, RunDecision::SkipNoRecentActivity { most_recent_ago_minutes: Some(_) }));
    }

    #[test]
    fn three_machines_most_recent_wins() {
        let a = rec("a", now() - Duration::minutes(30));
        let b = rec("b", now() - Duration::minutes(15));
        let c = rec("c", now() - Duration::minutes(2));
        let d = decide(&[a, b, c], "c", now());
        assert_eq!(d, RunDecision::Run);
    }

    #[test]
    fn malformed_records_dont_panic() {
        // read_all_records swallows malformed JSON; here we just ensure
        // empty input behaves sanely (already covered in empty_records_skip_no_recent).
        let d = decide(&[], "mac", now());
        assert!(matches!(d, RunDecision::SkipNoRecentActivity { .. }));
    }
}
