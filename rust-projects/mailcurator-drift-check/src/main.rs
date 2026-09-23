//! mailcurator-drift-check — weekly health check for the vendor extractors.
//! Rust port (2026-09-23) of the bash script of the same name; the launchd
//! agent `com.williamnapier.mailcurator-drift` (Sunday 09:00, RunAtLoad) and
//! `mailcurator-drift.timer` on nimbini run `~/.local/bin/mailcurator-drift-check`
//! unchanged.
//!
//! Runs `mailcurator coverage --drift --threshold 10.0 --floor 50
//! --floor-min-records 5`, appends the same log block the script wrote
//! (separator, stamped header, `exit=RC`, the output) to
//! `~/Library/Logs/mailcurator-drift.log` (Darwin) or
//! `~/.local/share/mailcurator-drift.log` (Linux), and on a non-zero exit
//! raises `mailcurator: N extractor drift(s) detected` through `notify-user`
//! with the first finding row as the body. Quiet when healthy. Exits with
//! mailcurator's own status (review D1-11: the last exit code is what
//! system-health-check reads).
//!
//! What changed versus the script:
//! - mailcurator not on PATH, or not spawnable, is its own failure: logged,
//!   alerted (`mailcurator-drift-check failed`), `last_error` recorded, exit 1.
//!   The script would have logged `exit=127` and raised a "? drift(s)" banner.
//! - Every run writes `~/.local/state/watchers/mailcurator-drift-check.json`
//!   (interval_secs 604800). A drift verdict is recorded as `last_error` so it
//!   stays visible in Check 9 until the next clean Sunday run; a healthy run
//!   clears it.
//! - The tool-owned log is capped at 5 MB via `logkeep` (one predecessor,
//!   `mailcurator-drift.log.1`). Note the Mac plist also points its
//!   StandardOutPath/StandardErrorPath at the same file; this tool writes
//!   nothing to stdout/stderr on a normal run, so the block is not doubled.
//!
//! Exit status: 0 = no drift; mailcurator's status (normally 1) = drift
//! detected; 1 = mailcurator could not be run.

mod exec;
mod outcome;

use clap::Parser;
use exec::Exec;
use regex::Regex;
use std::path::{Path, PathBuf};

const NAME: &str = "mailcurator-drift-check";
/// Sunday 09:00 on both hosts: one week.
const INTERVAL_SECS: u64 = 604_800;
/// Drop in percentage points to flag; matches mailcurator's default.
const THRESHOLD: &str = "10.0";
/// Absolute health floor (%): a policy below this is reported every run (review D1-11).
const FLOOR: &str = "50";
/// Below this many records the floor is not applied (marriott at 0% on one record).
const FLOOR_MIN_RECORDS: &str = "5";
const LOG_CAP: u64 = 5 * logkeep::MB;

#[derive(Parser)]
#[command(name = NAME, version, about = "Weekly drift check for mailcurator's vendor extractors (silent when healthy)")]
struct Cli {}

fn log_path(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Logs/mailcurator-drift.log")
    } else if cfg!(target_os = "linux") {
        home.join(".local/share/mailcurator-drift.log")
    } else {
        PathBuf::from("/tmp/mailcurator-drift.log")
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// mailcurator ran and reported no drift.
    Clean,
    /// mailcurator ran and exited `rc`; `count` is the "DRIFT DETECTED in N" figure
    /// ("?" when absent), `first` the first finding row, spaces squeezed.
    Drift { rc: i32, count: String, first: Option<String> },
    /// mailcurator could not be run at all.
    Unrunnable(String),
}

/// The count from the `DRIFT DETECTED in N field/policy combination(s):` header
/// and the first finding row (`policy field prev current drop prev_ts`), as the
/// script's grep and awk extracted them.
fn parse_findings(out: &str) -> (String, Option<String>) {
    let count = Regex::new(r"DRIFT DETECTED in ([0-9]+)").unwrap().captures(out).map(|c| c[1].to_string()).unwrap_or_else(|| "?".into());
    let row = Regex::new(r"^[a-z][a-z-]+ +<?[a-z_>]+ +[0-9]").unwrap();
    let first = out.lines().find(|l| row.is_match(l)).map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "));
    (count, first)
}

/// Runs the coverage check; returns the verdict and the combined output for the log.
fn assess(exec: &dyn Exec) -> (Verdict, i32, String) {
    let r = exec.run("mailcurator", &["coverage", "--drift", "--threshold", THRESHOLD, "--floor", FLOOR, "--floor-min-records", FLOOR_MIN_RECORDS]);
    let mut out = r.stdout.clone();
    if !r.stderr.is_empty() {
        out.push_str(&r.stderr);
    }
    if r.exit_code == 127 {
        return (Verdict::Unrunnable(format!("mailcurator could not be run (exit 127): {}", r.stderr.trim())), r.exit_code, out);
    }
    if r.exit_code == 0 {
        return (Verdict::Clean, 0, out);
    }
    let (count, first) = parse_findings(&out);
    (Verdict::Drift { rc: r.exit_code, count, first }, r.exit_code, out)
}

/// The block the script appended, verbatim in shape.
fn append_log_block(log: &Path, cap: u64, now: &str, rc: i32, out: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if let Ok(o) = logkeep::cap(log, cap) {
        if o.rolled() {
            // The new file's first line says where the rest went.
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(log)?;
            writeln!(f, "[{now}] log capped at {} bytes; predecessor {}", cap, logkeep::predecessor(log).display())?;
        }
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(log)?;
    writeln!(f, "===========================================================")?;
    writeln!(f, "[{now}] mailcurator-drift-check (threshold={THRESHOLD}pp floor={FLOOR}% min-records={FLOOR_MIN_RECORDS})")?;
    writeln!(f, "exit={rc}")?;
    writeln!(f, "{out}")?;
    Ok(())
}

/// Log, alert, record, and return the exit code.
fn finish(exec: &dyn Exec, log: &Path, state_dir: &Path, started_at: String, verdict: &Verdict, rc: i32, out: &str) -> i32 {
    let now = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
    if let Err(e) = append_log_block(log, LOG_CAP, &now, rc, out) {
        eprintln!("{NAME}: could not append to {}: {e}", log.display());
    }
    let (code, last_error, last_action) = match verdict {
        Verdict::Clean => (0, None, Some("no drift".to_string())),
        Verdict::Drift { rc, count, first } => {
            let title = format!("mailcurator: {count} extractor drift(s) detected");
            let body = first.clone().unwrap_or_else(|| "Run `mailcurator coverage --drift` to investigate.".into());
            let _ = exec.run("notify-user", &["--tool", NAME, &title, &body]);
            (*rc, Some(format!("{title}: {body}")), Some(title))
        }
        Verdict::Unrunnable(why) => {
            let _ = exec.run("notify-user", &["--tool", NAME, "--urgency", "critical", "mailcurator-drift-check failed", why]);
            // Also in the log, after the block, so a reader of the log sees why exit=127.
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(log) {
                use std::io::Write;
                let _ = writeln!(f, "[{now}] FAILED: {why}");
            }
            (1, Some(why.clone()), None)
        }
    };
    let o = outcome::Outcome { name: NAME, interval_secs: INTERVAL_SECS, started_at, last_action, actions: 1, last_error };
    if let Err(e) = outcome::record(state_dir, &o) {
        eprintln!("{NAME}: could not record outcome: {e}");
    }
    code
}

fn main() {
    let _ = Cli::parse();
    let started_at = outcome::now_rfc3339();
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));
    let (verdict, rc, out) = assess(&exec::Real);
    std::process::exit(finish(&exec::Real, &log_path(&home), &outcome::state_dir(&home), started_at, &verdict, rc, &out));
}

#[cfg(test)]
mod tests {
    use super::*;
    use exec::{CmdResult, Fake};

    const ARGS: &[&str] = &["coverage", "--drift", "--threshold", "10.0", "--floor", "50", "--floor-min-records", "5"];

    const DRIFT_OUT: &str = "Coverage drift vs snapshot 2026-09-14T09:00:00\n\nDRIFT DETECTED in 2 field/policy combination(s):\n\npolicy        field        prev   current  drop   prev_ts\nbooking-com   check_in     82.0   60.0     -22.0  2026-09-14T09:00:00\ntravelodge    <total>      55.0   40.0     -15.0  2026-09-14T09:00:00\n";
    const CLEAN_OUT: &str = "Coverage drift vs snapshot 2026-09-14T09:00:00\nno drift detected\n";

    fn heartbeat(state: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(state.join("mailcurator-drift-check.json")).unwrap()).unwrap()
    }

    #[test]
    fn parses_the_count_and_the_first_finding_row_like_the_awk() {
        let (count, first) = parse_findings(DRIFT_OUT);
        assert_eq!(count, "2");
        assert_eq!(first.as_deref(), Some("booking-com check_in 82.0 60.0 -22.0 2026-09-14T09:00:00"));
        // No header → "?"; header-like column line is not a finding row.
        let (count, first) = parse_findings("policy field prev\nsomething odd\n");
        assert_eq!(count, "?");
        assert_eq!(first, None);
        // A <total> pseudo-field row is a finding row too.
        let (_, first) = parse_findings("travelodge   <total>   55.0 40.0 -15.0 x\n");
        assert_eq!(first.as_deref(), Some("travelodge <total> 55.0 40.0 -15.0 x"));
    }

    #[test]
    fn green_control_no_drift_is_silent_exit_zero_with_a_clean_heartbeat() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("Library/Logs/mailcurator-drift.log");
        let state = d.path().join("state");
        let mut f = Fake::default();
        f.respond("mailcurator", ARGS, CmdResult::success(CLEAN_OUT));
        let (v, rc, out) = assess(&f);
        assert_eq!(v, Verdict::Clean);
        let code = finish(&f, &log, &state, outcome::now_rfc3339(), &v, rc, &out);
        assert_eq!(code, 0);
        assert!(!f.calls.borrow().iter().any(|c| c.starts_with("notify-user")), "{:?}", f.calls.borrow());
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("===========================================================\n["), "{text}");
        assert!(text.contains("] mailcurator-drift-check (threshold=10.0pp floor=50% min-records=5)\nexit=0\nCoverage drift"), "{text}");
        let hb = heartbeat(&state);
        assert_eq!(hb["interval_secs"], 604800);
        assert!(hb["last_error"].is_null(), "{hb}");
        assert_eq!(hb["last_action"], "no drift");
    }

    #[test]
    fn red_control_drift_alerts_with_the_parsed_title_and_exits_with_mailcurators_status() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("mailcurator-drift.log");
        let state = d.path().join("state");
        let mut f = Fake::default();
        f.respond("mailcurator", ARGS, CmdResult { exit_code: 1, stdout: DRIFT_OUT.into(), stderr: String::new() });
        let (v, rc, out) = assess(&f);
        assert!(matches!(&v, Verdict::Drift { rc: 1, count, first: Some(_) } if count == "2"), "{v:?}");
        let code = finish(&f, &log, &state, outcome::now_rfc3339(), &v, rc, &out);
        assert_eq!(code, 1);
        let calls = f.calls.borrow();
        let n = calls.iter().find(|c| c.starts_with("notify-user")).expect("an alert was raised");
        assert_eq!(n, "notify-user --tool mailcurator-drift-check mailcurator: 2 extractor drift(s) detected booking-com check_in 82.0 60.0 -22.0 2026-09-14T09:00:00");
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("exit=1\n"), "{text}");
        let hb = heartbeat(&state);
        assert_eq!(hb["last_error"], "mailcurator: 2 extractor drift(s) detected: booking-com check_in 82.0 60.0 -22.0 2026-09-14T09:00:00");
    }

    #[test]
    fn drift_without_a_parseable_row_falls_back_to_the_investigate_body() {
        let d = tempfile::tempdir().unwrap();
        let mut f = Fake::default();
        f.respond("mailcurator", ARGS, CmdResult { exit_code: 1, stdout: "DRIFT DETECTED in 1 field/policy combination(s):\n".into(), stderr: String::new() });
        let (v, rc, out) = assess(&f);
        let code = finish(&f, &d.path().join("l.log"), &d.path().join("s"), outcome::now_rfc3339(), &v, rc, &out);
        assert_eq!(code, 1);
        let calls = f.calls.borrow();
        assert!(calls.iter().any(|c| c == "notify-user --tool mailcurator-drift-check mailcurator: 1 extractor drift(s) detected Run `mailcurator coverage --drift` to investigate."), "{calls:?}");
    }

    #[test]
    fn missing_mailcurator_is_its_own_failure_not_a_drift_banner() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("mailcurator-drift.log");
        let state = d.path().join("state");
        let f = Fake::default(); // unscripted → 127, as a missing binary
        let (v, rc, out) = assess(&f);
        assert!(matches!(&v, Verdict::Unrunnable(w) if w.starts_with("mailcurator could not be run (exit 127)")), "{v:?}");
        let code = finish(&f, &log, &state, outcome::now_rfc3339(), &v, rc, &out);
        assert_eq!(code, 1);
        let calls = f.calls.borrow();
        assert!(calls.iter().any(|c| c.starts_with("notify-user --tool mailcurator-drift-check --urgency critical mailcurator-drift-check failed ")), "{calls:?}");
        assert!(!calls.iter().any(|c| c.contains("drift(s) detected")), "{calls:?}");
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("exit=127\n"), "{text}");
        assert!(text.contains("FAILED: mailcurator could not be run"), "{text}");
        let hb = heartbeat(&state);
        assert!(hb["last_error"].as_str().unwrap().starts_with("mailcurator could not be run"), "{hb}");
    }

    #[test]
    fn the_log_is_capped_with_one_predecessor() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("mailcurator-drift.log");
        std::fs::write(&log, "old ".repeat(50)).unwrap();
        append_log_block(&log, 100, "2026-09-23T01:00:00", 0, "fresh").unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.starts_with("[2026-09-23T01:00:00] log capped at 100 bytes; predecessor "), "{text}");
        assert!(text.ends_with("exit=0\nfresh\n"), "{text}");
        assert_eq!(std::fs::read_to_string(logkeep::predecessor(&log)).unwrap(), "old ".repeat(50));
    }
}
