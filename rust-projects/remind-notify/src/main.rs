//! remind-notify — notification daemon for DayPage reminders. Rust port
//! (2026-09-23) of the bash script of the same name; runs every minute from
//! launchd (`com.user.remind-notify`) and systemd (`remind-notify.timer`) at
//! the unchanged path `~/.local/bin/remind-notify`.
//!
//! Reads `~/Forge/NapierianLogs/Reminders/<today>.md`. Each open item
//! (`- [ ] …`) is a reminder: with `(at HH:MM)` it fires once the wall clock
//! reaches that time (string comparison on HH:MM, as before); without, it
//! fires once on the first run of the day. Fired items are recorded, one raw
//! line each, in `~/.local/share/remind-notified/<today>.txt`; state files
//! older than seven days are removed. `(at HH:MM)` and `(from [[…]])` are
//! stripped from the displayed text.
//!
//! The Mac dialog behaviour is kept: a Finder `display dialog` through
//! `osascript` (steals focus, works from launchd, gives up after 30 s). Linux
//! uses `notify-send … --urgency=normal`. These are user-facing reminders, not
//! tool alerts, so they do not go through `notify-user`.
//!
//! What changed versus the script:
//! - A notification whose command exits non-zero is NOT written to the state
//!   file, so it is retried on the next minute; the failure is recorded as
//!   `last_error` in `~/.local/state/watchers/remind-notify.json`. The script
//!   ran under `set -e`, so a failed `osascript` aborted the whole run before
//!   the state line — and every later reminder that minute — silently.
//! - A recorded outcome is written on every run (interval 60 s), so a timer
//!   that stops firing shows up in system-health-check Check 9.
//! - Quotes and backslashes in a reminder are escaped for AppleScript instead
//!   of being spliced raw into the `osascript` source.
//! - An open item with no text after `- [ ]` is skipped rather than shown.
//!
//! Exit status: 0 = ran (including "no reminder file today" and "a
//! notification failed" — the latter is a recorded finding); 1 = the state
//! directory could not be created or today's reminder file exists but could
//! not be read.

mod exec;
mod outcome;

use clap::Parser;
use exec::Exec;
use std::path::{Path, PathBuf};

const NAME: &str = "remind-notify";
const INTERVAL_SECS: u64 = 60;
const STATE_KEEP_DAYS: u64 = 7;

#[derive(Parser)]
#[command(name = NAME, version, about = "Fire desktop reminders from today's DayPage reminder file (every-minute job)")]
struct Cli {}

/// One reminder that is due now and has not been notified.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Fire {
    pub title: String,
    pub body: String,
    /// The raw message (line minus `- [ ] `), the key in the state file.
    pub raw: String,
}

/// `(at HH:MM)` → Some("HH:MM"), first occurrence, two-digit fields only (as
/// the script's regex); anything else is a date-only reminder.
fn at_time(msg: &str) -> Option<(usize, usize, String)> {
    let bytes = msg.as_bytes();
    let mut start = 0;
    while let Some(i) = msg[start..].find("(at ") {
        let s = start + i;
        // "(at " is 4 bytes, then HH:MM (5), then ")"
        if s + 10 <= bytes.len() {
            let t = &msg[s + 4..s + 9];
            let tb = t.as_bytes();
            let digits = tb[0].is_ascii_digit() && tb[1].is_ascii_digit() && tb[2] == b':' && tb[3].is_ascii_digit() && tb[4].is_ascii_digit();
            if digits && bytes[s + 9] == b')' {
                return Some((s, s + 10, t.to_string()));
            }
        }
        start = s + 1;
    }
    None
}

/// Remove ` *(from [[…]])` (greedy to the last `]])`, as `sed -E 's/ *\(from \[\[.*\]\]\)//'`).
fn strip_from(msg: &str) -> String {
    let Some(s) = msg.find("(from [[") else { return msg.to_string() };
    let Some(e_rel) = msg[s..].rfind("]])") else { return msg.to_string() };
    let e = s + e_rel + 3;
    let head = msg[..s].trim_end_matches(' ');
    format!("{head}{}", &msg[e..])
}

/// Remove ` *(at HH:MM)` at the span `at_time` found.
fn strip_at(msg: &str, span: (usize, usize)) -> String {
    let head = msg[..span.0].trim_end_matches(' ');
    format!("{head}{}", &msg[span.1..])
}

/// Pure decision: which open reminders fire now. `state` is the state file's
/// text (one raw message per line); `now_hhmm` is the wall clock as HH:MM.
pub fn decide(reminders: &str, state: &str, now_hhmm: &str) -> Vec<Fire> {
    let notified: std::collections::HashSet<&str> = state.lines().collect();
    let mut out = vec![];
    for line in reminders.lines() {
        let Some(msg) = line.strip_prefix("- [ ] ") else { continue };
        if msg.is_empty() || notified.contains(msg) {
            continue;
        }
        match at_time(msg) {
            Some((s, e, time)) => {
                if now_hhmm < time.as_str() {
                    continue;
                }
                let body = strip_from(&strip_at(msg, (s, e)));
                out.push(Fire { title: format!("Reminder ({time})"), body, raw: msg.to_string() });
            }
            None => out.push(Fire { title: "Reminder".into(), body: strip_from(msg), raw: msg.to_string() }),
        }
    }
    out
}

fn applescript_quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The Finder dialog source, as the script built it.
pub fn dialog_script(title: &str, body: &str) -> String {
    format!(
        "tell application \"Finder\"\n            activate\n            display dialog \"{}\" with title \"{}\" giving up after 30 buttons {{\"OK\"}} default button \"OK\"\n        end tell",
        applescript_quote(body),
        applescript_quote(title)
    )
}

/// Deliver one reminder. Returns the command's exit code (0 = shown).
fn notify(exec: &dyn Exec, mac: bool, f: &Fire) -> exec::CmdResult {
    if mac {
        exec.run("osascript", &["-e", &dialog_script(&f.title, &f.body)])
    } else {
        exec.run("notify-send", &[&f.title, &f.body, "--urgency=normal"])
    }
}

/// Remove `*.txt` state files older than `keep_days` (best-effort).
fn prune_state(dir: &Path, keep_days: u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(keep_days * 86_400);
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("txt") {
            continue;
        }
        if let Ok(m) = p.metadata() {
            if m.is_file() && m.modified().map(|t| t < cutoff).unwrap_or(false) {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

struct Run {
    exit_code: i32,
    fired: u64,
    last_error: Option<String>,
}

/// One tick. `today` is YYYY-MM-DD, `now_hhmm` is HH:MM.
fn run(exec: &dyn Exec, home: &Path, mac: bool, today: &str, now_hhmm: &str) -> Run {
    let reminders_dir = home.join("Forge/NapierianLogs/Reminders");
    let state_dir = home.join(".local/share/remind-notified");
    let reminder_file = reminders_dir.join(format!("{today}.md"));
    let state_file = state_dir.join(format!("{today}.txt"));

    if let Err(e) = std::fs::create_dir_all(&state_dir) {
        let msg = format!("cannot create {}: {e}", state_dir.display());
        eprintln!("{NAME}: {msg}");
        return Run { exit_code: 1, fired: 0, last_error: Some(msg) };
    }
    prune_state(&state_dir, STATE_KEEP_DAYS);

    if !reminder_file.exists() {
        return Run { exit_code: 0, fired: 0, last_error: None };
    }
    let text = match std::fs::read_to_string(&reminder_file) {
        Ok(t) => t,
        Err(e) => {
            let msg = format!("cannot read {}: {e}", reminder_file.display());
            eprintln!("{NAME}: {msg}");
            return Run { exit_code: 1, fired: 0, last_error: Some(msg) };
        }
    };
    let state = std::fs::read_to_string(&state_file).unwrap_or_default();
    if !state_file.exists() {
        let _ = std::fs::write(&state_file, "");
    }

    let mut fired = 0;
    let mut last_error = None;
    for f in decide(&text, &state, now_hhmm) {
        let r = notify(exec, mac, &f);
        if r.ok() {
            use std::io::Write;
            match std::fs::OpenOptions::new().create(true).append(true).open(&state_file) {
                Ok(mut fh) => {
                    let _ = writeln!(fh, "{}", f.raw);
                }
                Err(e) => last_error = Some(format!("cannot record {}: {e}", state_file.display())),
            }
            println!("fired: {} — {}", f.title, f.body);
            fired += 1;
        } else {
            let msg = format!("notification failed (exit {}) for \"{}\": {}", r.exit_code, f.body, r.stderr.trim());
            eprintln!("{NAME}: {msg}");
            last_error = Some(msg);
        }
    }
    Run { exit_code: 0, fired, last_error }
}

fn record(state_dir: &Path, started_at: String, r: &Run) {
    let o = outcome::Outcome {
        name: NAME,
        interval_secs: INTERVAL_SECS,
        started_at,
        last_action: if r.fired > 0 { Some(format!("fired {}", r.fired)) } else { None },
        actions: r.fired,
        last_error: r.last_error.clone(),
    };
    if let Err(e) = outcome::record(state_dir, &o) {
        eprintln!("{NAME}: could not record outcome in {}: {e}", state_dir.display());
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn main() {
    let _ = Cli::parse();
    let started_at = outcome::now_rfc3339();
    let home = home();
    let now = chrono::Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    let hhmm = now.format("%H:%M").to_string();
    let r = run(&exec::Real, &home, cfg!(target_os = "macos"), &today, &hhmm);
    record(&outcome::state_dir(&home), started_at, &r);
    std::process::exit(r.exit_code);
}

#[cfg(test)]
mod tests {
    use super::*;
    use exec::{CmdResult, Fake};

    const FILE: &str = "# Reminders 2026-09-23\n\n- [ ] Call the bank (at 09:30)\n- [ ] Post the letter (at 14:00) (from [[2026-09-22]])\n- [x] Done already (at 08:00)\n- [X] Also done\n- [ ] Water the plants (from [[Garden]])\n- [ ] Bad time (at 9:30)\n- [ ] \nnot a task line\n";

    fn raws(v: &[Fire]) -> Vec<&str> {
        v.iter().map(|f| f.raw.as_str()).collect()
    }

    #[test]
    fn timed_reminders_fire_only_when_due() {
        let early = decide(FILE, "", "09:29");
        assert_eq!(raws(&early), vec!["Water the plants (from [[Garden]])", "Bad time (at 9:30)"], "{early:?}");
        let at = decide(FILE, "", "09:30");
        assert!(raws(&at).contains(&"Call the bank (at 09:30)"));
        assert!(!raws(&at).contains(&"Post the letter (at 14:00) (from [[2026-09-22]])"));
        let late = decide(FILE, "", "23:59");
        assert!(raws(&late).contains(&"Post the letter (at 14:00) (from [[2026-09-22]])"));
    }

    #[test]
    fn titles_and_bodies_are_stripped_as_the_script_did() {
        let v = decide(FILE, "", "23:59");
        let bank = v.iter().find(|f| f.raw.starts_with("Call the bank")).unwrap();
        assert_eq!(bank.title, "Reminder (09:30)");
        assert_eq!(bank.body, "Call the bank");
        let post = v.iter().find(|f| f.raw.starts_with("Post the letter")).unwrap();
        assert_eq!(post.title, "Reminder (14:00)");
        assert_eq!(post.body, "Post the letter");
        let plants = v.iter().find(|f| f.raw.starts_with("Water")).unwrap();
        assert_eq!(plants.title, "Reminder");
        assert_eq!(plants.body, "Water the plants");
    }

    #[test]
    fn malformed_time_is_a_date_only_reminder() {
        let v = decide("- [ ] Bad time (at 9:30)\n", "", "00:00");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].title, "Reminder");
        assert_eq!(v[0].body, "Bad time (at 9:30)");
    }

    #[test]
    fn already_notified_checked_headings_blank_and_empty_are_skipped() {
        let state = "Call the bank (at 09:30)\nWater the plants (from [[Garden]])\n";
        let v = decide(FILE, state, "23:59");
        let r = raws(&v);
        assert!(!r.contains(&"Call the bank (at 09:30)"));
        assert!(!r.contains(&"Water the plants (from [[Garden]])"));
        assert!(!r.iter().any(|x| x.contains("Done already") || x.contains("Also done") || x.starts_with('#') || x.is_empty()));
        assert_eq!(r, vec!["Post the letter (at 14:00) (from [[2026-09-22]])", "Bad time (at 9:30)"]);
    }

    #[test]
    fn dialog_script_matches_the_script_and_escapes_quotes() {
        let s = dialog_script("Reminder (09:30)", "Say \"hi\"");
        assert!(s.starts_with("tell application \"Finder\"\n            activate\n            display dialog \"Say \\\"hi\\\"\" with title \"Reminder (09:30)\" giving up after 30 buttons {\"OK\"} default button \"OK\""));
        assert!(s.ends_with("end tell"));
    }

    fn home_with(reminders: &str, today: &str) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("Forge/NapierianLogs/Reminders");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{today}.md")), reminders).unwrap();
        d
    }

    fn heartbeat(home: &Path) -> serde_json::Value {
        let s = std::fs::read_to_string(outcome::state_dir(home).join(format!("{NAME}.json"))).unwrap();
        serde_json::from_str(&s).unwrap()
    }

    #[test]
    fn green_control_mac_dialog_shown_and_state_recorded() {
        let d = home_with("- [ ] Call the bank (at 09:30)\n- [ ] Later (at 18:00)\n", "2026-09-23");
        let mut f = Fake::default();
        f.respond("osascript", &["-e", &dialog_script("Reminder (09:30)", "Call the bank")], CmdResult::success("button returned:OK\n"));
        let r = run(&f, d.path(), true, "2026-09-23", "10:00");
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.fired, 1);
        assert!(r.last_error.is_none(), "{:?}", r.last_error);
        let state = std::fs::read_to_string(d.path().join(".local/share/remind-notified/2026-09-23.txt")).unwrap();
        assert_eq!(state, "Call the bank (at 09:30)\n");
        assert_eq!(f.calls.borrow().len(), 1, "{:?}", f.calls.borrow());
        record(&outcome::state_dir(d.path()), "t0".into(), &r);
        let hb = heartbeat(d.path());
        assert!(hb["last_error"].is_null());
        assert_eq!(hb["actions"], 1);
        assert_eq!(hb["interval_secs"], 60);
        // Second run the same minute: nothing fires again.
        let r2 = run(&f, d.path(), true, "2026-09-23", "10:00");
        assert_eq!(r2.fired, 0);
        assert_eq!(f.calls.borrow().len(), 1);
    }

    #[test]
    fn linux_uses_notify_send() {
        let d = home_with("- [ ] Water the plants\n", "2026-09-23");
        let mut f = Fake::default();
        f.respond("notify-send", &["Reminder", "Water the plants", "--urgency=normal"], CmdResult::success(""));
        let r = run(&f, d.path(), false, "2026-09-23", "10:00");
        assert_eq!((r.exit_code, r.fired), (0, 1));
    }

    #[test]
    fn red_control_failed_dialog_is_retried_not_recorded() {
        let d = home_with("- [ ] Call the bank (at 09:30)\n", "2026-09-23");
        let mut f = Fake::default();
        f.respond("osascript", &["-e", &dialog_script("Reminder (09:30)", "Call the bank")], CmdResult::failure(1, "execution error: No user interaction allowed"));
        let r = run(&f, d.path(), true, "2026-09-23", "10:00");
        assert_eq!(r.exit_code, 0, "a failed notification is a finding, not a failed run");
        assert_eq!(r.fired, 0);
        let err = r.last_error.clone().unwrap();
        assert!(err.contains("notification failed (exit 1)") && err.contains("No user interaction allowed"), "{err}");
        let state = std::fs::read_to_string(d.path().join(".local/share/remind-notified/2026-09-23.txt")).unwrap();
        assert_eq!(state, "", "not recorded, so the next minute retries");
        record(&outcome::state_dir(d.path()), "t0".into(), &r);
        assert!(heartbeat(d.path())["last_error"].as_str().unwrap().contains("notification failed"));
        // Next minute: fires again (retry).
        let r2 = run(&f, d.path(), true, "2026-09-23", "10:01");
        assert_eq!(f.calls.borrow().len(), 2);
        assert_eq!(r2.fired, 0);
    }

    #[test]
    fn no_reminder_file_is_quiet_and_green() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake::default();
        let r = run(&f, d.path(), true, "2026-09-23", "10:00");
        assert_eq!((r.exit_code, r.fired), (0, 0));
        assert!(r.last_error.is_none());
        assert!(f.calls.borrow().is_empty());
        assert!(d.path().join(".local/share/remind-notified").is_dir(), "state dir is created as before");
    }

    #[test]
    fn unreadable_reminder_file_exits_one() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("Forge/NapierianLogs/Reminders");
        std::fs::create_dir_all(dir.join("2026-09-23.md")).unwrap(); // a directory where the file should be
        let r = run(&Fake::default(), d.path(), true, "2026-09-23", "10:00");
        assert_eq!(r.exit_code, 1);
        assert!(r.last_error.unwrap().contains("cannot read"));
    }

    #[test]
    fn state_files_older_than_seven_days_are_pruned() {
        let d = tempfile::tempdir().unwrap();
        let sd = d.path().join(".local/share/remind-notified");
        std::fs::create_dir_all(&sd).unwrap();
        let old = sd.join("2026-09-01.txt");
        let fresh = sd.join("2026-09-22.txt");
        let other = sd.join("keep.json");
        for p in [&old, &fresh, &other] {
            std::fs::write(p, "x").unwrap();
        }
        let ten_days_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(10 * 86_400);
        std::fs::File::options().write(true).open(&old).unwrap().set_modified(ten_days_ago).unwrap();
        std::fs::File::options().write(true).open(&other).unwrap().set_modified(ten_days_ago).unwrap();
        let r = run(&Fake::default(), d.path(), true, "2026-09-23", "10:00");
        assert_eq!(r.exit_code, 0);
        assert!(!old.exists(), "old .txt pruned");
        assert!(fresh.exists(), "recent .txt kept");
        assert!(other.exists(), "only .txt files are pruned");
    }
}
