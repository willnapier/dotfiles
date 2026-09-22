//! Problem persistence across runs — the answer to "how long has this been
//! red?" Before 2026-09-22 the status file was a snapshot: a follower error that
//! had sat for a week looked identical to a watcher that had not ticked since
//! the Mac woke sixteen minutes earlier, and both were skimmed past in every
//! startup brief. Each problem now carries a stable key, when it was first seen
//! and how many consecutive runs it has been seen; ai-brief renders the age, and
//! a problem confirmed on its second run is posted to the Messageboard where
//! the unclaimed sweep will keep raising it until someone acts.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProblemRecord {
    pub text: String,
    /// `problem_key(text)`: volatile numbers and hashes stripped.
    pub key: String,
    /// RFC 3339, local time, of the first run that reported this key.
    pub first_seen: String,
    /// Consecutive runs (including this one) that reported this key.
    pub runs: u32,
}

/// A key that survives the parts of a problem string that change every run:
/// ages ("15 min ago"), exit codes, counts and Git hashes. Lower-cased; every
/// run of hex ≥ 7 chars or of digits becomes `#`; whitespace collapsed.
pub fn problem_key(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_was_hash = false;
    for token in text.split_whitespace() {
        let t = token.to_lowercase();
        let is_hex_run = t.len() >= 7 && t.chars().all(|c| c.is_ascii_hexdigit());
        let mut piece = String::new();
        if is_hex_run {
            piece.push('#');
        } else {
            let mut in_digits = false;
            for c in t.chars() {
                if c.is_ascii_digit() {
                    if !in_digits {
                        piece.push('#');
                        in_digits = true;
                    }
                } else {
                    in_digits = false;
                    piece.push(c);
                }
            }
        }
        if !(piece == "#" && last_was_hash) {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&piece);
        }
        last_was_hash = piece == "#";
    }
    out
}

/// Short stable tag for the Messageboard: `HEALTH-<host>-<8 hex>` (FNV-1a of
/// the key), so `archive-containing` can find exactly this post later.
pub fn board_tag(host: &str, key: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("HEALTH-{host}-{:08x}", (h >> 32) as u32 ^ h as u32)
}

/// Fold this run's problems into the previous run's records.
/// Returns (current records, records that were present last run and are gone).
pub fn merge_history(previous: &[ProblemRecord], current: &[String], now: &str) -> (Vec<ProblemRecord>, Vec<ProblemRecord>) {
    let mut records = Vec::with_capacity(current.len());
    for text in current {
        let key = problem_key(text);
        let prior = previous.iter().find(|r| r.key == key);
        records.push(ProblemRecord {
            text: text.clone(),
            first_seen: prior.map(|r| r.first_seen.clone()).unwrap_or_else(|| now.to_string()),
            runs: prior.map(|r| r.runs + 1).unwrap_or(1),
            key,
        });
    }
    let cleared = previous.iter().filter(|r| !records.iter().any(|c| c.key == r.key)).cloned().collect();
    (records, cleared)
}

/// Keys of the problems confirmed on at least two consecutive runs — the set
/// the Messageboard summary describes. Sorted so equality is order-free.
pub fn confirmed_keys(records: &[ProblemRecord]) -> Vec<String> {
    let mut keys: Vec<String> = records.iter().filter(|r| r.runs >= 2).map(|r| r.key.clone()).collect();
    keys.sort();
    keys.dedup();
    keys
}

/// One rolling Messageboard section per host, tagged `HEALTH-<host>`, replaced
/// only when the confirmed set changes and archived when it empties. One
/// section, not one per problem: the first live run posted 33 entries in a
/// minute and would have buried the board head in every startup brief.
/// Service/agent/watcher problems come first; `Code …` housekeeping (dirty or
/// unpushed branches) last; six lines then "+N more".
pub fn board_summary(host: &str, records: &[ProblemRecord]) -> String {
    let mut confirmed: Vec<&ProblemRecord> = records.iter().filter(|r| r.runs >= 2).collect();
    confirmed.sort_by(|a, b| {
        let housekeeping = |r: &ProblemRecord| r.text.starts_with("Code ");
        housekeeping(a).cmp(&housekeeping(b)).then(a.first_seen.cmp(&b.first_seen)).then(a.text.cmp(&b.text))
    });
    let oldest = confirmed.iter().map(|r| day(&r.first_seen)).min().unwrap_or_default();
    let mut lines = vec![format!(
        "HEALTH-{host} — {} confirmed problem{} (oldest since {oldest}) — from system-health-check on {host}; replaced when the set changes, archived when clean; full list ~/Assistants/health/{host}.json",
        confirmed.len(),
        if confirmed.len() == 1 { "" } else { "s" }
    )];
    for r in confirmed.iter().take(6) {
        lines.push(format!("• {} [since {}, {}×]", r.text, day(&r.first_seen), r.runs));
    }
    if confirmed.len() > 6 {
        lines.push(format!("• +{} more", confirmed.len() - 6));
    }
    lines.join("\n")
}

/// `2026-09-15T08:03:54+01:00` → `15 Sep`.
fn day(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339).map(|d| d.format("%-d %b").to_string()).unwrap_or_else(|_| rfc3339.get(..10).unwrap_or(rfc3339).to_string())
}

/// What to do to the board this run, given what it currently shows.
#[derive(Debug, PartialEq, Eq)]
pub enum BoardAction {
    Nothing,
    /// Archive the existing section (confirmed set now empty).
    Archive,
    /// Archive the existing section if any, then post the new summary.
    Replace,
}

pub fn board_action(previously_posted: &[String], now_confirmed: &[String]) -> BoardAction {
    if previously_posted == now_confirmed {
        BoardAction::Nothing
    } else if now_confirmed.is_empty() {
        BoardAction::Archive
    } else {
        BoardAction::Replace
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_strips_ages_hashes_and_exit_codes_but_keeps_identity() {
        let a = problem_key("Watcher git-auto-pull-watcher: last cycle 16 min ago (interval 120s) — dead or hung");
        let b = problem_key("Watcher git-auto-pull-watcher: last cycle 5 min ago (interval 120s) — dead or hung");
        assert_eq!(a, b);
        assert_eq!(a, "watcher git-auto-pull-watcher: last cycle # min ago (interval #s) — dead or hung");
        let f1 = problem_key("Watcher assistants-git-sync-follower: last error: follower must not create history: HEAD b744482989496e7e4c67941cc0c643b17e2aa867 and origin/main 35c541ed6c5e3c70d4485a69e054c4384d329213 have diverged or local HEAD is ahead");
        let f2 = problem_key("Watcher assistants-git-sync-follower: last error: follower must not create history: HEAD 33be740fd82bf1815ae3f5f93ba05b971658d336 and origin/main c5cb942bb4541e4b38cb07096324992d351162de have diverged or local HEAD is ahead");
        assert_eq!(f1, f2);
        assert_eq!(problem_key("Agent errored: com.williamnapier.x [exit 1]"), problem_key("Agent errored: com.williamnapier.x [exit 78]"));
        assert_ne!(problem_key("Service failed: a"), problem_key("Service failed: b"));
        assert_ne!(problem_key("DNA: cargo-crates drift +1/-0"), problem_key("DNA: pacman drift +1/-0"));
    }

    #[test]
    fn board_tag_is_stable_and_host_scoped() {
        let k = problem_key("Service failed: mailcurator-drift");
        assert_eq!(board_tag("nimbini", &k), board_tag("nimbini", &k));
        assert_ne!(board_tag("nimbini", &k), board_tag("macos", &k));
        assert!(board_tag("macos", &k).starts_with("HEALTH-macos-"));
        assert_eq!(board_tag("macos", &k).len(), "HEALTH-macos-".len() + 8);
    }

    #[test]
    fn merge_counts_consecutive_runs_and_reports_cleared() {
        let day1 = ["Service failed: a".to_string(), "Watcher w: last cycle 16 min ago (interval 120s) — dead or hung".to_string()];
        let (r1, cleared1) = merge_history(&[], &day1, "2026-09-15T08:00:00+01:00");
        assert!(cleared1.is_empty());
        assert!(r1.iter().all(|r| r.runs == 1 && r.first_seen == "2026-09-15T08:00:00+01:00"));

        // Day 2: `a` persists (age text changed on the watcher line, which has cleared).
        let day2 = ["Service failed: a".to_string(), "Service failed: b".to_string()];
        let (r2, cleared2) = merge_history(&r1, &day2, "2026-09-16T08:00:00+01:00");
        let a = r2.iter().find(|r| r.text == "Service failed: a").unwrap();
        assert_eq!((a.runs, a.first_seen.as_str()), (2, "2026-09-15T08:00:00+01:00"));
        let b = r2.iter().find(|r| r.text == "Service failed: b").unwrap();
        assert_eq!(b.runs, 1);
        assert_eq!(cleared2.len(), 1);
        assert!(cleared2[0].text.starts_with("Watcher w"));

        // Day 3: `a` gone → cleared with runs 2 (was posted); `b` confirmed.
        let day3 = ["Service failed: b".to_string()];
        let (r3, cleared3) = merge_history(&r2, &day3, "2026-09-17T08:00:00+01:00");
        assert_eq!(r3[0].runs, 2);
        assert_eq!(cleared3[0].text, "Service failed: a");
        assert_eq!(cleared3[0].runs, 2);
    }

    #[test]
    fn board_is_one_summary_replaced_only_when_the_confirmed_set_changes() {
        let rec = |text: &str, runs: u32, seen: &str| ProblemRecord { text: text.into(), key: problem_key(text), first_seen: seen.into(), runs };
        let records = vec![
            rec("Code unpushed: x/y has 1 commit unreachable from origin", 5, "2026-09-10T08:00:00+01:00"),
            rec("Watcher f: last error: boom", 8, "2026-09-15T08:03:00+01:00"),
            rec("Service failed: fresh", 1, "2026-09-22T08:00:00+01:00"),
            rec("Agent errored: a [exit 1]", 2, "2026-09-21T08:00:00+01:00"),
        ];
        let keys = confirmed_keys(&records);
        assert_eq!(keys.len(), 3, "runs==1 is not confirmed");
        let s = board_summary("macos", &records);
        let lines: Vec<&str> = s.lines().collect();
        assert!(lines[0].starts_with("HEALTH-macos — 3 confirmed problems (oldest since 10 Sep)"), "{}", lines[0]);
        // Service/agent/watcher first, by age; Code housekeeping last.
        assert!(lines[1].starts_with("• Watcher f: last error: boom [since 15 Sep, 8×]"), "{}", lines[1]);
        assert!(lines[2].starts_with("• Agent errored: a [exit 1] [since 21 Sep, 2×]"), "{}", lines[2]);
        assert!(lines[3].starts_with("• Code unpushed"), "{}", lines[3]);
        assert_eq!(lines.len(), 4);

        assert_eq!(board_action(&keys, &keys), BoardAction::Nothing);
        assert_eq!(board_action(&[], &keys), BoardAction::Replace);
        assert_eq!(board_action(&keys, &keys[..2]), BoardAction::Replace);
        assert_eq!(board_action(&keys, &[]), BoardAction::Archive);
        assert_eq!(board_action(&[], &[]), BoardAction::Nothing);
    }

    #[test]
    fn board_summary_caps_at_six_lines_plus_more() {
        let records: Vec<ProblemRecord> = (0..9)
            .map(|i| ProblemRecord { text: format!("Service failed: s{i}"), key: format!("k{i}"), first_seen: "2026-09-20T08:00:00+01:00".into(), runs: 3 })
            .collect();
        let s = board_summary("nimbini", &records);
        assert_eq!(s.lines().count(), 1 + 6 + 1);
        assert!(s.ends_with("• +3 more"));
    }
}
