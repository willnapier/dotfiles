//! gmail-push-tags-watchdog — periodic health check for the gmail-push-tags
//! service. Rust port (2026-09-22) of the bash script of the same name; the
//! log paths, thresholds, log-line formats and always-exit-0 contract are
//! unchanged, so the systemd timer and launchd plist that run
//! `~/.local/bin/gmail-push-tags-watchdog` every 30 minutes need no change.
//!
//! Three failure modes, each raised through `notify-user` (recorded, D2-23):
//!   1. Wedged while running: the service is running but its log has not
//!      advanced for HUNG_THRESHOLD — the 2026-04-27 SENT-loop signature.
//!   2. Failed last run: non-zero exit, ignoring 15 (SIGTERM from the next
//!      interval interrupting a long catch-up run).
//!   3. Timer not firing: not running and the log is older than STALE_THRESHOLD.
//!
//! Audit D2-21: the script's Linux wedge branch required
//! `ActiveState=activating` for over 15 minutes, a state a Type=simple/oneshot
//! unit essentially never occupies, so a wedged service on nimbini was never
//! reported. Linux now mirrors the (correct) macOS rule: running with a
//! MainPID and a stale log is wedged.
//!
//! Healthy state is silent — one "ok" log line, no banner.

mod exec;

use clap::Parser;
use exec::Exec;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Parser)]
#[command(name = "gmail-push-tags-watchdog", version, about = "Health check for the gmail-push-tags service (silent when healthy)")]
struct Cli {}

/// Seconds a running process may leave its log untouched before it is wedged.
/// The original SENT loop hung 3.5 h; 15 min catches it within the next tick,
/// and is generous enough for the long notmuch scan on a 173k-message corpus.
const HUNG_THRESHOLD: u64 = 900;
/// Seconds the log may be untouched while nothing runs: three missed 15-minute runs.
const STALE_THRESHOLD: u64 = 2700;
const MISSING_LOG_AGE: u64 = 999_999;

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Alert(String),
    Ok(String),
}

#[derive(Debug, Default, PartialEq, Eq)]
struct MacState {
    loaded: bool,
    pid: Option<u64>,
    /// Raw `LastExitStatus` as launchctl prints it (a wait status, so exit 1 shows as 256; 15 is SIGTERM).
    last_exit: Option<i64>,
}

/// Parse `launchctl list <label>` (plist-ish `"PID" = 123;` lines). Empty
/// output means the agent is not loaded.
fn parse_launchctl(raw: &str) -> MacState {
    if raw.trim().is_empty() {
        return MacState::default();
    }
    let mut st = MacState { loaded: true, ..Default::default() };
    for line in raw.lines() {
        let t = line.trim_start();
        let value = |t: &str| -> Option<i64> {
            let (_, rhs) = t.split_once('=')?;
            let digits: String = rhs.chars().filter(|c| c.is_ascii_digit() || *c == '-').collect();
            digits.parse().ok()
        };
        if t.starts_with("\"PID\"") {
            st.pid = value(t).and_then(|v| u64::try_from(v).ok());
        } else if t.contains("LastExitStatus") {
            st.last_exit = value(t);
        }
    }
    st
}

fn assess_mac(state: &MacState, log_age: u64) -> Verdict {
    if !state.loaded {
        return Verdict::Alert("launchd agent not loaded".into());
    }
    let running = matches!(state.pid, Some(p) if p > 0);
    if running && log_age > HUNG_THRESHOLD {
        return Verdict::Alert(format!("running but log unchanged for {} min — likely wedged", log_age / 60));
    }
    if let Some(code) = state.last_exit {
        if code != 0 && code != 15 {
            return Verdict::Alert(format!("last run exited with status {code}"));
        }
    }
    if !running && log_age > STALE_THRESHOLD {
        return Verdict::Alert(format!("log not updated in {} min — timer may not be firing", log_age / 60));
    }
    Verdict::Ok(format!(
        "ok: mac (pid={} last_exit={} log_age={log_age}s)",
        state.pid.map(|p| p.to_string()).unwrap_or_default(),
        state.last_exit.map(|c| c.to_string()).unwrap_or_default()
    ))
}

/// `KEY=value` lines from `systemctl show`, looked up by key.
fn show_value<'a>(show_output: &'a str, key: &str) -> &'a str {
    show_output.lines().find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('='))).unwrap_or("").trim()
}

/// D2-21: judge the Linux service the way the macOS branch always did.
fn assess_linux(show_output: &str, log_age: u64) -> Verdict {
    let active = show_value(show_output, "ActiveState");
    let main_pid: u64 = show_value(show_output, "MainPID").parse().unwrap_or(0);
    let running = matches!(active, "active" | "activating" | "reloading") && main_pid > 0;
    if running && log_age > HUNG_THRESHOLD {
        return Verdict::Alert(format!("running but log unchanged for {} min — likely wedged", log_age / 60));
    }
    if active == "failed" {
        let code: i64 = show_value(show_output, "ExecMainStatus").parse().unwrap_or(0);
        if code != 15 {
            return Verdict::Alert(format!("service in failed state (exit {code})"));
        }
    }
    if !running && log_age > STALE_THRESHOLD {
        return Verdict::Alert(format!("log not updated in {} min — timer may not be firing", log_age / 60));
    }
    Verdict::Ok(format!("ok: linux (active={active} main_pid={main_pid} log_age={log_age}s)"))
}

fn log_age(path: &Path, now: u64) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| now.saturating_sub(d.as_secs()))
        .unwrap_or(MISSING_LOG_AGE)
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn watchdog_log(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Logs/gmail-push-tags-watchdog.log")
    } else if cfg!(target_os = "linux") {
        home.join(".local/share/gmail-push-tags-watchdog.log")
    } else {
        PathBuf::from("/tmp/gmail-push-tags-watchdog.log")
    }
}

fn service_log(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Logs/gmail-push-tags.log")
    } else {
        home.join(".local/share/gmail-push-tags.log")
    }
}

fn append_log(path: &Path, line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {line}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}

/// Through `notify-user`, which records whether the banner arrived (D2-23).
fn notify(msg: &str) {
    let _ = Command::new("notify-user")
        .args(["--tool", "gmail-push-tags-watchdog", "--urgency", "critical", "gmail-push-tags health", msg])
        .output();
}

fn assess(exec: &dyn Exec, home: &Path, now: u64) -> Verdict {
    let age = log_age(&service_log(home), now);
    if cfg!(target_os = "macos") {
        let r = exec.run("launchctl", &["list", "com.williamnapier.gmail-push-tags"]);
        let raw = if r.ok() { r.stdout } else { String::new() };
        assess_mac(&parse_launchctl(&raw), age)
    } else {
        let r = exec.run(
            "systemctl",
            &["--user", "show", "gmail-push-tags.service", "-p", "ActiveState", "-p", "SubState", "-p", "MainPID", "-p", "ExecMainStatus", "-p", "ExecMainExitTimestamp"],
        );
        assess_linux(&r.stdout, age)
    }
}

fn main() {
    let _ = Cli::parse();
    if !(cfg!(target_os = "macos") || cfg!(target_os = "linux")) {
        append_log(&watchdog_log(&home()), &format!("unsupported platform: {}", std::env::consts::OS));
        return;
    }
    let home = home();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    match assess(&exec::Real, &home, now) {
        Verdict::Ok(line) => append_log(&watchdog_log(&home), &line),
        Verdict::Alert(msg) => {
            append_log(&watchdog_log(&home), &format!("ALERT: {msg}"));
            notify(&msg);
        }
    }
    // As the script: always exit 0. A finding is a banner and a log line, not
    // a failed watchdog unit.
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAUNCHCTL_RUNNING: &str = "{\n\t\"LimitLoadToSessionType\" = \"Aqua\";\n\t\"Label\" = \"com.williamnapier.gmail-push-tags\";\n\t\"LastExitStatus\" = 0;\n\t\"PID\" = 4242;\n\t\"Program\" = \"/x\";\n};\n";
    const LAUNCHCTL_IDLE_OK: &str = "{\n\t\"Label\" = \"com.williamnapier.gmail-push-tags\";\n\t\"LastExitStatus\" = 0;\n};\n";
    const LAUNCHCTL_IDLE_FAILED: &str = "{\n\t\"Label\" = \"com.williamnapier.gmail-push-tags\";\n\t\"LastExitStatus\" = 256;\n};\n";
    const LAUNCHCTL_IDLE_SIGTERM: &str = "{\n\t\"Label\" = \"com.williamnapier.gmail-push-tags\";\n\t\"LastExitStatus\" = 15;\n};\n";

    fn linux(active: &str, pid: u64, exit: i64) -> String {
        format!("ActiveState={active}\nSubState=x\nMainPID={pid}\nExecMainStatus={exit}\nExecMainExitTimestamp=\n")
    }

    #[test]
    fn launchctl_parses_pid_and_exit_and_not_loaded() {
        assert_eq!(parse_launchctl(LAUNCHCTL_RUNNING), MacState { loaded: true, pid: Some(4242), last_exit: Some(0) });
        assert_eq!(parse_launchctl(LAUNCHCTL_IDLE_FAILED), MacState { loaded: true, pid: None, last_exit: Some(256) });
        assert_eq!(parse_launchctl(""), MacState::default());
    }

    #[test]
    fn mac_verdicts_known_red_and_known_green() {
        assert!(matches!(assess_mac(&parse_launchctl(""), 60), Verdict::Alert(m) if m.contains("not loaded")));
        assert!(matches!(assess_mac(&parse_launchctl(LAUNCHCTL_RUNNING), 20 * 60), Verdict::Alert(m) if m.contains("likely wedged")));
        assert!(matches!(assess_mac(&parse_launchctl(LAUNCHCTL_IDLE_FAILED), 60), Verdict::Alert(m) if m == "last run exited with status 256"));
        assert!(matches!(assess_mac(&parse_launchctl(LAUNCHCTL_IDLE_OK), 50 * 60), Verdict::Alert(m) if m.contains("timer may not be firing")));
        assert!(matches!(assess_mac(&parse_launchctl(LAUNCHCTL_RUNNING), 2 * 60), Verdict::Ok(l) if l == "ok: mac (pid=4242 last_exit=0 log_age=120s)"));
        // SIGTERM from the next interval is not a failure.
        assert!(matches!(assess_mac(&parse_launchctl(LAUNCHCTL_IDLE_SIGTERM), 60), Verdict::Ok(_)));
    }

    #[test]
    fn linux_wedged_is_detected_while_active_not_only_activating() {
        // D2-21 known-red: active + MainPID + 20-minute-old log → wedged.
        assert!(matches!(assess_linux(&linux("active", 555, 0), 20 * 60), Verdict::Alert(m) if m == "running but log unchanged for 20 min — likely wedged"));
        assert!(matches!(assess_linux(&linux("activating", 555, 0), 20 * 60), Verdict::Alert(m) if m.contains("wedged")));
        // Known-green: active, log 2 minutes old → silent ok line.
        assert!(matches!(assess_linux(&linux("active", 555, 0), 120), Verdict::Ok(l) if l == "ok: linux (active=active main_pid=555 log_age=120s)"));
    }

    #[test]
    fn linux_failed_and_stale_and_sigterm() {
        assert!(matches!(assess_linux(&linux("failed", 0, 1), 60), Verdict::Alert(m) if m == "service in failed state (exit 1)"));
        assert!(matches!(assess_linux(&linux("failed", 0, 15), 60), Verdict::Ok(_)), "SIGTERM is not a failure");
        assert!(matches!(assess_linux(&linux("inactive", 0, 0), 50 * 60), Verdict::Alert(m) if m == "log not updated in 50 min — timer may not be firing"));
        assert!(matches!(assess_linux(&linux("inactive", 0, 0), 5 * 60), Verdict::Ok(_)));
        // A missing log reads as very old: stale when idle.
        assert!(matches!(assess_linux(&linux("inactive", 0, 0), MISSING_LOG_AGE), Verdict::Alert(_)));
    }

    #[test]
    fn log_age_uses_mtime_and_missing_is_huge() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("gmail-push-tags.log");
        std::fs::write(&p, "x").unwrap();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let earlier = SystemTime::now() - std::time::Duration::from_secs(1200);
        std::fs::File::open(&p).unwrap().set_modified(earlier).unwrap();
        let age = log_age(&p, now);
        assert!((1199..=1202).contains(&age), "{age}");
        assert_eq!(log_age(&d.path().join("absent.log"), now), MISSING_LOG_AGE);
    }

    #[test]
    fn end_to_end_with_fake_exec_and_temp_log() {
        use exec::{CmdResult, Fake};
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        std::fs::create_dir_all(service_log(home).parent().unwrap()).unwrap();
        std::fs::write(service_log(home), "x").unwrap();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let mut fake = Fake::default();
        fake.respond("launchctl", &["list", "com.williamnapier.gmail-push-tags"], CmdResult::success(LAUNCHCTL_RUNNING));
        fake.respond(
            "systemctl",
            &["--user", "show", "gmail-push-tags.service", "-p", "ActiveState", "-p", "SubState", "-p", "MainPID", "-p", "ExecMainStatus", "-p", "ExecMainExitTimestamp"],
            CmdResult::success(&linux("active", 555, 0)),
        );
        // Fresh log: ok on either platform.
        assert!(matches!(assess(&fake, home, now), Verdict::Ok(_)));
        // Twenty minutes stale while running: wedged on either platform.
        let earlier = SystemTime::now() - std::time::Duration::from_secs(1200);
        std::fs::File::open(service_log(home)).unwrap().set_modified(earlier).unwrap();
        assert!(matches!(assess(&fake, home, now), Verdict::Alert(m) if m.contains("wedged")));
    }
}
