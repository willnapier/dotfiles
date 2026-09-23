//! Parking: a recorded decision about a known problem. A parked problem stays
//! in `problem_history` (its age keeps counting) but leaves `problems`, the
//! exit-1 verdict, the desktop alert and the Messageboard until the park's
//! end date, when it returns to red and the entry is pruned. Parks live in
//! `~/Assistants/health/parked.<host>.toml` — one writer per file, so
//! Syncthing carries them without conflict, like the status files beside them.
//!
//! The point (2026-09-23, after a follower error sat unread in every startup
//! brief for a week): a red that is old is either being worked, or parked
//! with a reason and an end date, or ignored — and only the third should look
//! like the third.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Park {
    /// `history::problem_key` of the parked problem.
    pub key: String,
    /// Last day the park holds, `YYYY-MM-DD` (local).
    pub until: String,
    pub reason: String,
    /// RFC 3339, local.
    pub parked_at: String,
    /// Short hostname of the machine the park was made on.
    pub by: String,
}

#[derive(Serialize, Deserialize, Default)]
struct ParkFile {
    #[serde(default)]
    parks: Vec<Park>,
}

/// What the status file records for a parked problem this run.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ParkedProblem {
    pub key: String,
    pub text: String,
    pub until: String,
    pub reason: String,
    pub parked_at: String,
}

pub fn path(home: &Path, host: &str) -> PathBuf {
    home.join("Assistants/health").join(format!("parked.{host}.toml"))
}

/// Missing file → no parks. A malformed file is an error: silently treating
/// it as empty would un-park everything without a word.
pub fn load(path: &Path) -> Result<Vec<Park>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("reading {}: {e}", path.display())),
    };
    toml::from_str::<ParkFile>(&text).map(|f| f.parks).map_err(|e| format!("parsing {}: {e}", path.display()))
}

/// Atomic replace, as the status file does.
pub fn save(path: &Path, parks: &[Park]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = toml::to_string_pretty(&ParkFile { parks: parks.to_vec() }).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

pub fn parse_until(s: &str) -> Result<chrono::NaiveDate, String> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| format!("--until must be YYYY-MM-DD, got {s:?}"))
}

/// A park holds through its `until` day inclusive.
pub fn is_expired(park: &Park, today: chrono::NaiveDate) -> bool {
    match parse_until(&park.until) {
        Ok(d) => d < today,
        Err(_) => true, // an unreadable date never holds a problem off the board
    }
}

/// (still holding, expired) — the caller logs and saves the first list.
pub fn prune_expired(parks: Vec<Park>, today: chrono::NaiveDate) -> (Vec<Park>, Vec<Park>) {
    parks.into_iter().partition(|p| !is_expired(p, today))
}

/// Keys containing the needle (case-insensitive). A park must name exactly
/// one problem; the caller refuses on 0 or several and prints these.
pub fn candidates<'a>(keys: &[&'a str], needle: &str) -> Vec<&'a str> {
    let n = needle.to_lowercase();
    let mut out: Vec<&str> = keys.iter().copied().filter(|k| k.to_lowercase().contains(&n)).collect();
    out.sort();
    out.dedup();
    out
}

pub struct Split {
    /// Problems that count: reported, alerted, posted, exit 1.
    pub active: Vec<String>,
    /// Problems held by an unexpired park.
    pub parked: Vec<ParkedProblem>,
}

pub fn split_problems(problems: &[String], parks: &[Park]) -> Split {
    let mut active = Vec::new();
    let mut parked = Vec::new();
    for text in problems {
        let key = crate::history::problem_key(text);
        match parks.iter().find(|p| p.key == key) {
            Some(p) => parked.push(ParkedProblem {
                key,
                text: text.clone(),
                until: p.until.clone(),
                reason: p.reason.clone(),
                parked_at: p.parked_at.clone(),
            }),
            None => active.push(text.clone()),
        }
    }
    Split { active, parked }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn park(key: &str, until: &str) -> Park {
        Park { key: key.into(), until: until.into(), reason: "coverage work queued".into(), parked_at: "2026-09-23T01:40:00+01:00".into(), by: "macos".into() }
    }

    #[test]
    fn a_park_holds_through_its_last_day_and_expires_after() {
        let p = park("service failed: mailcurator-drift", "2026-09-30");
        assert!(!is_expired(&p, NaiveDate::from_ymd_opt(2026, 9, 30).unwrap()));
        assert!(is_expired(&p, NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()));
        assert!(is_expired(&park("x", "not-a-date"), NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()));
        let (kept, gone) = prune_expired(vec![park("a", "2026-09-30"), park("b", "2026-09-01")], NaiveDate::from_ymd_opt(2026, 9, 23).unwrap());
        assert_eq!(kept.len(), 1);
        assert_eq!(gone[0].key, "b");
    }

    #[test]
    fn a_park_must_name_exactly_one_problem() {
        let keys = ["service failed: mailcurator-drift", "agent errored: com.williamnapier.mailcurator-drift [exit #]", "watcher f: last error: x"];
        assert_eq!(candidates(&keys, "mailcurator").len(), 2, "ambiguous");
        assert_eq!(candidates(&keys, "watcher f"), vec!["watcher f: last error: x"]);
        assert!(candidates(&keys, "nothing-like-this").is_empty());
        assert_eq!(candidates(&keys, "WATCHER F").len(), 1, "case-insensitive");
    }

    #[test]
    fn parked_problems_leave_the_active_list_but_keep_their_text() {
        let problems = vec!["Service failed: mailcurator-drift".to_string(), "Watcher f: last error: x".to_string()];
        let parks = vec![park(&crate::history::problem_key("Service failed: mailcurator-drift"), "2026-09-30")];
        let s = split_problems(&problems, &parks);
        assert_eq!(s.active, vec!["Watcher f: last error: x".to_string()]);
        assert_eq!(s.parked.len(), 1);
        assert_eq!(s.parked[0].text, "Service failed: mailcurator-drift");
        assert_eq!(s.parked[0].until, "2026-09-30");
        // Still in history with its age: the board excludes it, history does not.
        let (records, _) = crate::history::merge_history(&[], &problems, "2026-09-23T01:40:00+01:00");
        let parked_keys: Vec<&str> = s.parked.iter().map(|p| p.key.as_str()).collect();
        let board_records: Vec<_> = records.iter().filter(|r| !parked_keys.contains(&r.key.as_str())).cloned().collect();
        assert_eq!(records.len(), 2);
        assert_eq!(board_records.len(), 1);
    }

    #[test]
    fn park_file_round_trips_and_missing_means_none() {
        let d = tempfile::tempdir().unwrap();
        let p = path(d.path(), "macos");
        assert_eq!(load(&p).unwrap(), vec![]);
        let parks = vec![park("service failed: mailcurator-drift", "2026-09-30")];
        save(&p, &parks).unwrap();
        assert_eq!(load(&p).unwrap(), parks);
        assert!(p.ends_with("Assistants/health/parked.macos.toml"));
        std::fs::write(&p, "parks = 3\n").unwrap();
        assert!(load(&p).is_err(), "a malformed park file must not silently un-park everything");
    }
}
