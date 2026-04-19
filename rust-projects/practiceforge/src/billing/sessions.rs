//! Session-JSON-backed billing source.
//!
//! Replaces the legacy notes.md header scraping with the richer session
//! JSON captured from TM3 (`~/.local/share/practiceforge/session-*.json`).
//! Cross-references one-off appointments in the scheduling system to skip
//! free reschedules.
//!
//! ## Policy
//!
//! | Session status                  | Billable? | Reason tag           |
//! |---------------------------------|-----------|----------------------|
//! | pending / done / arrived        | yes       | Attended             |
//! | dna / no-show / noshow          | yes       | DNA (policy)         |
//! | late-cancel / late-cancellation | yes       | LateCancellation     |
//! | cancelled                       | **no**    | — (long-notice)      |
//! | anything else / empty           | no        | — (unknown)          |
//!
//! Per `project_dna_charging_policy.md`:
//! - Original slot always billed, even when client DNAs.
//! - Reschedule is free (48h window). Detected via scheduling one-off
//!   Appointment with `reschedule_for: Some(...)` matching client+date.
//! - Long-notice cancellation: not billed. (The session JSON status
//!   "cancelled" doesn't carry a timestamp, so we default to the
//!   client-friendly reading. If William wants to bill a short-notice
//!   cancellation, the session status must first be changed to
//!   `late-cancel` in the dashboard.)

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Reason this session appears on the invoice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BillReason {
    Attended,
    Dna,
    LateCancellation,
}

impl BillReason {
    /// Short tag appended to the invoice line description.
    pub fn line_item_tag(&self) -> &'static str {
        match self {
            BillReason::Attended => "",
            BillReason::Dna => " (did not attend)",
            BillReason::LateCancellation => " (late cancellation)",
        }
    }
}

/// A single billable session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BillableSession {
    pub date: String,
    pub reason: BillReason,
}

// ---------------------------------------------------------------------------
// Session JSON types — subset of ClinicSession/ClinicClient
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RawSessionFile {
    date: String,
    #[serde(default)]
    clients: Vec<RawSessionClient>,
}

#[derive(Debug, Deserialize)]
struct RawSessionClient {
    id: String,
    #[serde(default)]
    status: String,
}

// ---------------------------------------------------------------------------
// One-off appointment — minimal fields needed for reschedule detection
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RawOneOff {
    client_id: String,
    date: String,
    #[serde(default)]
    reschedule_for: Option<String>,
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Default location of session JSON files: `~/.local/share/practiceforge/`.
pub fn default_session_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_default()
                .join(".local")
                .join("share")
        })
        .join("practiceforge")
}

/// Default scheduling root: `~/Clinical/schedules/`.
pub fn default_schedules_dir() -> PathBuf {
    if let Ok(root) = std::env::var("CLINICAL_ROOT") {
        PathBuf::from(root).join("schedules")
    } else {
        dirs::home_dir()
            .unwrap_or_default()
            .join("Clinical")
            .join("schedules")
    }
}

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

/// Classify a session JSON status string according to billing policy.
/// Returns `None` if the status is not billable (cancelled, unknown, etc.).
pub fn classify_status(status: &str) -> Option<BillReason> {
    match status.to_lowercase().as_str() {
        "pending" | "done" | "arrived" | "completed" => Some(BillReason::Attended),
        "dna" | "no-show" | "noshow" => Some(BillReason::Dna),
        "late-cancel" | "late-cancellation" | "latecancel" => {
            Some(BillReason::LateCancellation)
        }
        // "cancelled" — long-notice cancel, not billed.
        // "tentative" / "confirmed" — future, not yet happened.
        // empty or unknown — skip.
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Session JSON loader
// ---------------------------------------------------------------------------

/// Load every session JSON file in `session_dir` and return the subset
/// belonging to `client_id`. The (date, raw-status) pairs are returned
/// in chronological order.
fn load_client_session_rows(
    client_id: &str,
    session_dir: &Path,
) -> Result<Vec<(String, String)>> {
    if !session_dir.exists() {
        return Ok(Vec::new());
    }

    let mut rows: Vec<(String, String)> = Vec::new();

    for entry in std::fs::read_dir(session_dir)
        .with_context(|| format!("read {}", session_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !name.starts_with("session-") || !name.ends_with(".json") {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let session: RawSessionFile = match serde_json::from_str(&content) {
            Ok(s) => s,
            Err(_) => continue, // skip malformed files rather than aborting
        };

        for c in session.clients {
            if c.id == client_id {
                rows.push((session.date.clone(), c.status.clone()));
            }
        }
    }

    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows.dedup(); // same client twice in one session file is nonsensical; keep first
    Ok(rows)
}

/// Load one-off appointments and return the set of (client_id, date) pairs
/// where `reschedule_for` is populated — these are free-reschedule dates.
fn load_free_reschedule_dates(schedules_dir: &Path) -> Result<HashSet<(String, String)>> {
    let mut out: HashSet<(String, String)> = HashSet::new();
    if !schedules_dir.exists() {
        return Ok(out);
    }

    for entry in std::fs::read_dir(schedules_dir)
        .with_context(|| format!("read {}", schedules_dir.display()))?
    {
        let entry = entry?;
        let prac_dir = entry.path();
        if !prac_dir.is_dir() {
            continue;
        }
        let appts_dir = prac_dir.join("appointments");
        if !appts_dir.exists() {
            continue;
        }

        for appt_entry in std::fs::read_dir(&appts_dir)? {
            let appt_entry = appt_entry?;
            let path = appt_entry.path();
            if !path.extension().is_some_and(|e| e == "yaml" || e == "yml") {
                continue;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let appt: RawOneOff = match serde_yaml::from_str(&content) {
                Ok(a) => a,
                Err(_) => continue,
            };
            if appt.reschedule_for.is_some() {
                out.insert((appt.client_id, appt.date));
            }
        }
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compute the list of billable sessions for `client_id` from session JSON.
///
/// Chronologically ordered, policy-filtered, with free-reschedule dates
/// suppressed. `schedules_dir` may be absent if the scheduling system
/// isn't in use on this machine — in that case no reschedules are detected.
pub fn billable_sessions_for_client(
    client_id: &str,
    session_dir: &Path,
    schedules_dir: Option<&Path>,
) -> Result<Vec<BillableSession>> {
    let rows = load_client_session_rows(client_id, session_dir)?;

    let free_reschedules = match schedules_dir {
        Some(dir) => load_free_reschedule_dates(dir).unwrap_or_default(),
        None => HashSet::new(),
    };

    let mut out: Vec<BillableSession> = Vec::new();
    for (date, status) in rows {
        if free_reschedules.contains(&(client_id.to_string(), date.clone())) {
            continue;
        }
        if let Some(reason) = classify_status(&status) {
            out.push(BillableSession { date, reason });
        }
    }
    Ok(out)
}

/// Filter `all_sessions` down to those whose date is not in `invoiced_dates`.
pub fn uninvoiced_billable(
    all_sessions: &[BillableSession],
    invoiced_dates: &[String],
) -> Vec<BillableSession> {
    let set: HashSet<&str> = invoiced_dates.iter().map(|s| s.as_str()).collect();
    all_sessions
        .iter()
        .filter(|s| !set.contains(s.date.as_str()))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_session(dir: &Path, date: &str, clients: &[(&str, &str)]) {
        let entries: Vec<String> = clients
            .iter()
            .map(|(id, status)| {
                format!(r#"{{"id":"{}","time":"10:00","status":"{}"}}"#, id, status)
            })
            .collect();
        let body = format!(
            r#"{{"date":"{}","started_at":"{}T10:00:00","clients":[{}]}}"#,
            date,
            date,
            entries.join(",")
        );
        let path = dir.join(format!("session-{}.json", date));
        fs::write(path, body).unwrap();
    }

    #[test]
    fn test_classify_status() {
        assert_eq!(classify_status("pending"), Some(BillReason::Attended));
        assert_eq!(classify_status("done"), Some(BillReason::Attended));
        assert_eq!(classify_status("dna"), Some(BillReason::Dna));
        assert_eq!(classify_status("DNA"), Some(BillReason::Dna));
        assert_eq!(classify_status("no-show"), Some(BillReason::Dna));
        assert_eq!(
            classify_status("late-cancel"),
            Some(BillReason::LateCancellation)
        );
        assert_eq!(classify_status("cancelled"), None);
        assert_eq!(classify_status(""), None);
        assert_eq!(classify_status("tentative"), None);
    }

    #[test]
    fn test_billable_sessions_policy() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        write_session(dir, "2026-03-01", &[("EB76", "done")]);
        write_session(dir, "2026-03-08", &[("EB76", "dna")]);
        write_session(dir, "2026-03-15", &[("EB76", "cancelled")]);
        write_session(dir, "2026-03-22", &[("EB76", "late-cancel")]);
        write_session(dir, "2026-03-29", &[("EB76", "pending")]);

        let result = billable_sessions_for_client("EB76", dir, None).unwrap();
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].date, "2026-03-01");
        assert_eq!(result[0].reason, BillReason::Attended);
        assert_eq!(result[1].date, "2026-03-08");
        assert_eq!(result[1].reason, BillReason::Dna);
        // 2026-03-15 cancelled — skipped
        assert_eq!(result[2].date, "2026-03-22");
        assert_eq!(result[2].reason, BillReason::LateCancellation);
        assert_eq!(result[3].date, "2026-03-29");
        assert_eq!(result[3].reason, BillReason::Attended);
    }

    #[test]
    fn test_free_reschedule_is_suppressed() {
        let session_tmp = TempDir::new().unwrap();
        let sched_tmp = TempDir::new().unwrap();
        write_session(session_tmp.path(), "2026-03-15", &[("EB76", "done")]);

        // Build a scheduling one-off marked as a reschedule for this date.
        let prac_dir = sched_tmp.path().join("default");
        let appts_dir = prac_dir.join("appointments");
        fs::create_dir_all(&appts_dir).unwrap();
        let appt_yaml = r#"
id: 00000000-0000-0000-0000-000000000001
series_id: null
practitioner: default
client_id: EB76
client_name: Elizabeth Briscoe
date: 2026-03-15
start_time: "10:00:00"
end_time: "10:50:00"
status: Completed
source: Reschedule
location: remote
reschedule_for: "2026-03-08"
created_at: "2026-03-09T10:00:00Z"
"#;
        fs::write(appts_dir.join("one.yaml"), appt_yaml).unwrap();

        let result = billable_sessions_for_client(
            "EB76",
            session_tmp.path(),
            Some(sched_tmp.path()),
        )
        .unwrap();
        // The 15 March session is a free reschedule, so nothing billable.
        assert!(result.is_empty(), "expected no billable sessions, got {result:?}");
    }

    #[test]
    fn test_ignores_other_clients() {
        let tmp = TempDir::new().unwrap();
        write_session(
            tmp.path(),
            "2026-03-01",
            &[("EB76", "done"), ("AJ83", "done")],
        );
        let result = billable_sessions_for_client("EB76", tmp.path(), None).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].date, "2026-03-01");
    }

    #[test]
    fn test_uninvoiced_billable() {
        let all = vec![
            BillableSession {
                date: "2026-03-01".to_string(),
                reason: BillReason::Attended,
            },
            BillableSession {
                date: "2026-03-08".to_string(),
                reason: BillReason::Dna,
            },
            BillableSession {
                date: "2026-03-15".to_string(),
                reason: BillReason::Attended,
            },
        ];
        let invoiced = vec!["2026-03-01".to_string()];
        let result = uninvoiced_billable(&all, &invoiced);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].date, "2026-03-08");
        assert_eq!(result[1].date, "2026-03-15");
    }

    #[test]
    fn test_missing_session_dir_is_empty() {
        let tmp = TempDir::new().unwrap();
        let bogus = tmp.path().join("no-such-dir");
        let result = billable_sessions_for_client("EB76", &bogus, None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_malformed_session_files_are_skipped() {
        let tmp = TempDir::new().unwrap();
        write_session(tmp.path(), "2026-03-01", &[("EB76", "done")]);
        fs::write(tmp.path().join("session-broken.json"), "{ not json").unwrap();
        let result = billable_sessions_for_client("EB76", tmp.path(), None).unwrap();
        assert_eq!(result.len(), 1);
    }
}
