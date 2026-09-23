//! syncthing-sync-watchdog — proves that Syncthing is carrying files from the
//! Mac to nimbini. Rust port (2026-09-23) of the bash script of the same name;
//! the launchd plist `com.user.syncthing-sync-watchdog` still runs
//! `~/.local/bin/syncthing-sync-watchdog run` every 5 minutes, unchanged.
//!
//! How it works (as the script, positive-evidence design — audit D2 "verified
//! sound"): write the current epoch seconds to `~/Forge/.syncthing-heartbeat-mac`,
//! then poll nimbini over ssh every 5 s for up to 60 s until the file there
//! carries a value ≥ the one just written. Comparing against the *previous*
//! heartbeat's age produced false alarms after sleep; this compares against
//! this run's own write.
//!
//! Outcomes:
//! - reached → "Heartbeat reached Nimbini: N" in the log, `~/.syncthing-watchdog-status`
//!   = `OK: Last check <date>`, exit 0.
//! - stalled → "ALERT: Sync stalled: …" in the log, a critical banner through
//!   `notify-user --tool syncthing-sync-watchdog` (recorded, D2-23), status file
//!   `STALLED: …`, **exit 0** — the script's documented contract: a detected sync
//!   problem is a monitoring result, not a watchdog failure, so launchd keeps the
//!   StartInterval schedule. The stall is nonetheless visible to the health
//!   checks: the recorded outcome below carries it as `last_error`.
//! - the heartbeat file itself cannot be written → the tool could not do its
//!   job: "ERROR: …" in the log, exit 1 (new; the script's `set -e` died silently).
//!
//! Semantic differences from the script:
//! - Recorded outcome: `~/.local/state/watchers/syncthing-sync-watchdog.json`
//!   (interval_secs 300) after every run; `last_error` on a stall or write failure.
//! - The tool-owned log `~/Library/Logs/syncthing-watchdog.log` is capped at 5 MB
//!   (one predecessor `.1`, via logkeep — audit D2-24). It had no cap.
//! - `ssh` is invoked with an explicit argv, no shell; the remote command is unchanged.
//! - `test` prints the same summary lines and also returns the run's exit status.

mod exec;
mod outcome;

use clap::{Parser, Subcommand};
use exec::Exec;
use std::path::{Path, PathBuf};
use std::time::Duration;

const NAME: &str = "syncthing-sync-watchdog";
const INTERVAL_SECS: u64 = 300;
const PROPAGATION_TIMEOUT_SECONDS: u64 = 60;
const POLL_INTERVAL_SECONDS: u64 = 5;
const NIMBINI_HOST: &str = "will@nimbini";
const REMOTE_FILE: &str = "/home/will/Forge/.syncthing-heartbeat-mac";
const LOG_CAP: u64 = 5 * logkeep::MB;

#[derive(Parser)]
#[command(name = NAME, version, about = "Syncthing Sync Watchdog — proves Mac→nimbini propagation")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write heartbeat and check sync (default)
    Run,
    /// Show current watchdog status
    Status,
    /// Show recent watchdog logs
    Logs,
    /// Run once with verbose output
    Test,
}

/// Wall clock and sleep, injectable so tests neither wait nor depend on time.
pub trait Clock {
    fn now_epoch(&self) -> u64;
    fn sleep(&self, d: Duration);
}

pub struct RealClock;
impl Clock for RealClock {
    fn now_epoch(&self) -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d)
    }
}

pub struct Paths {
    heartbeat: PathBuf,
    log: PathBuf,
    status: PathBuf,
    state_dir: PathBuf,
}

impl Paths {
    fn under(home: &Path) -> Paths {
        Paths {
            heartbeat: home.join("Forge/.syncthing-heartbeat-mac"),
            log: home.join("Library/Logs/syncthing-watchdog.log"),
            status: home.join(".syncthing-watchdog-status"),
            state_dir: outcome::state_dir(home),
        }
    }
}

fn stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// `date` as the script printed it in the status file.
fn date_line() -> String {
    chrono::Local::now().format("%a %e %b %Y %H:%M:%S %Z").to_string()
}

fn log(paths: &Paths, line: &str) {
    use std::io::Write;
    if let Some(d) = paths.log.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&paths.log) {
        let _ = writeln!(f, "{} {line}", stamp());
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The remote value that proved propagation.
    Reached(u64),
    Stalled(String),
}

/// Parse what `bat --style=plain` printed on nimbini: the whole output must be
/// one unsigned integer (the script's `^[0-9]+$` on the trimmed value).
pub fn parse_remote(raw: &str) -> Option<u64> {
    let t = raw.trim();
    if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    t.parse().ok()
}

fn remote_cmd() -> String {
    format!("bat --style=plain --paging=never {REMOTE_FILE}")
}

/// Poll nimbini until the heartbeat written this run has arrived or the
/// deadline passes. Pure over Exec + Clock.
pub fn check_remote(exec: &dyn Exec, clock: &dyn Clock, expected: u64) -> Verdict {
    let deadline = clock.now_epoch() + PROPAGATION_TIMEOUT_SECONDS;
    let cmd = remote_cmd();
    while clock.now_epoch() <= deadline {
        let r = exec.run("ssh", &["-o", "ConnectTimeout=5", NIMBINI_HOST, &cmd]);
        if r.ok() {
            if let Some(v) = parse_remote(&r.stdout) {
                if v >= expected {
                    return Verdict::Reached(v);
                }
            }
        }
        clock.sleep(Duration::from_secs(POLL_INTERVAL_SECONDS));
    }
    Verdict::Stalled(format!("Sync stalled: heartbeat {expected} did not reach Nimbini within {PROPAGATION_TIMEOUT_SECONDS}s"))
}

fn notify(exec: &dyn Exec, msg: &str) {
    // Exit ignored: notify-user records the attempt either way (D2-23).
    let _ = exec.run("notify-user", &["--tool", NAME, "--urgency", "critical", "Syncthing Watchdog", msg]);
}

/// One run. Returns the process exit code.
pub fn run(exec: &dyn Exec, clock: &dyn Clock, paths: &Paths) -> i32 {
    let started_at = outcome::now_rfc3339();
    match logkeep::cap(&paths.log, LOG_CAP) {
        Ok(o) if o.rolled() => log(paths, &format!("log capped at {} MB; predecessor {}", LOG_CAP / logkeep::MB, logkeep::predecessor(&paths.log).display())),
        Ok(_) => {}
        Err(e) => log(paths, &format!("could not cap log: {e}")),
    }

    let now = clock.now_epoch();
    if let Err(e) = std::fs::write(&paths.heartbeat, format!("{now}\n")) {
        let msg = format!("could not write heartbeat {}: {e}", paths.heartbeat.display());
        log(paths, &format!("ERROR: {msg}"));
        record(paths, started_at, None, 0, Some(msg));
        return 1;
    }
    log(paths, &format!("Wrote heartbeat: {now}"));

    match check_remote(exec, clock, now) {
        Verdict::Reached(v) => {
            log(paths, &format!("Heartbeat reached Nimbini: {v}"));
            let _ = std::fs::write(&paths.status, format!("OK: Last check {}\n", date_line()));
            record(paths, started_at, Some(format!("heartbeat {now} reached nimbini")), 1, None);
            0
        }
        Verdict::Stalled(msg) => {
            log(paths, &format!("ALERT: {msg}"));
            notify(exec, &msg);
            let _ = std::fs::write(&paths.status, format!("STALLED: {msg} ({})\n", date_line()));
            record(paths, started_at, None, 0, Some(msg));
            // Documented contract: a stall is a finding, not a failed watchdog run.
            0
        }
    }
}

fn record(paths: &Paths, started_at: String, last_action: Option<String>, actions: u64, last_error: Option<String>) {
    let o = outcome::Outcome { name: NAME, interval_secs: INTERVAL_SECS, started_at, last_action, actions, last_error };
    if let Err(e) = outcome::record(&paths.state_dir, &o) {
        log(paths, &format!("could not record outcome: {e}"));
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME is not set"))
}

fn main() {
    let cli = Cli::parse();
    let paths = Paths::under(&home());
    let code = match cli.cmd.unwrap_or(Cmd::Run) {
        Cmd::Run => run(&exec::Real, &RealClock, &paths),
        Cmd::Status => {
            match std::fs::read_to_string(&paths.status) {
                Ok(s) => print!("{s}"),
                Err(_) => println!("No status yet (watchdog hasn't run)"),
            }
            0
        }
        Cmd::Logs => {
            match std::fs::read_to_string(&paths.log) {
                Ok(s) => {
                    let lines: Vec<&str> = s.lines().collect();
                    let start = lines.len().saturating_sub(50);
                    for l in &lines[start..] {
                        println!("{l}");
                    }
                }
                Err(_) => println!("No logs yet"),
            }
            0
        }
        Cmd::Test => {
            println!("Testing watchdog...");
            let code = run(&exec::Real, &RealClock, &paths);
            let ok = std::fs::read_to_string(&paths.status).map(|s| s.starts_with("OK:")).unwrap_or(false);
            if code == 0 && ok {
                println!("OK: Sync is healthy");
            } else {
                println!("ALERT: Sync appears stalled");
            }
            code
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;
    use exec::{CmdResult, Fake};
    use std::cell::{Cell, RefCell};

    /// A clock that advances by `step` on every read and never sleeps for real.
    struct FakeClock {
        t: Cell<u64>,
        step: u64,
        slept: Cell<u64>,
    }
    impl FakeClock {
        fn new(start: u64, step: u64) -> Self {
            FakeClock { t: Cell::new(start), step, slept: Cell::new(0) }
        }
    }
    impl Clock for FakeClock {
        fn now_epoch(&self) -> u64 {
            let v = self.t.get();
            self.t.set(v + self.step);
            v
        }
        fn sleep(&self, d: Duration) {
            self.slept.set(self.slept.get() + d.as_secs());
        }
    }

    /// An Exec whose ssh answers differ per call, so "arrives on the 2nd poll" is testable.
    struct Sequenced {
        answers: RefCell<Vec<CmdResult>>,
        calls: RefCell<Vec<String>>,
    }
    impl Exec for Sequenced {
        fn run(&self, program: &str, args: &[&str]) -> CmdResult {
            self.calls.borrow_mut().push(Fake::key(program, args));
            if program == "ssh" {
                let mut a = self.answers.borrow_mut();
                if a.is_empty() {
                    CmdResult::success("")
                } else {
                    a.remove(0)
                }
            } else {
                CmdResult::success("")
            }
        }
    }

    fn ssh_args() -> [String; 4] {
        ["-o".into(), "ConnectTimeout=5".into(), NIMBINI_HOST.into(), remote_cmd()]
    }
    fn ssh_key() -> String {
        let a = ssh_args();
        Fake::key("ssh", &[&a[0], &a[1], &a[2], &a[3]])
    }
    fn fake_ssh(r: CmdResult) -> Fake {
        let mut f = Fake::default();
        let a = ssh_args();
        f.respond("ssh", &[&a[0], &a[1], &a[2], &a[3]], r);
        f
    }
    fn heartbeat_json(paths: &Paths) -> serde_json::Value {
        let hb = std::fs::read_to_string(paths.state_dir.join("syncthing-sync-watchdog.json")).unwrap();
        serde_json::from_str(&hb).unwrap()
    }

    #[test]
    fn remote_value_must_be_a_bare_integer() {
        assert_eq!(parse_remote("1758600000\n"), Some(1758600000));
        assert_eq!(parse_remote(""), None);
        assert_eq!(parse_remote("cat: no such file"), None);
        assert_eq!(parse_remote("17586 00000"), None);
    }

    #[test]
    fn green_control_heartbeat_arrives_on_second_poll() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("Forge")).unwrap();
        let paths = Paths::under(d.path());
        let exec = Sequenced {
            answers: RefCell::new(vec![CmdResult::success("1000\n"), CmdResult::success("5000\n")]),
            calls: Default::default(),
        };
        let clock = FakeClock::new(5000, 1);
        let code = run(&exec, &clock, &paths);
        assert_eq!(code, 0);
        assert_eq!(std::fs::read_to_string(&paths.heartbeat).unwrap(), "5000\n");
        let status = std::fs::read_to_string(&paths.status).unwrap();
        assert!(status.starts_with("OK: Last check "), "{status}");
        let log = std::fs::read_to_string(&paths.log).unwrap();
        assert!(log.contains("Wrote heartbeat: 5000"), "{log}");
        assert!(log.contains("Heartbeat reached Nimbini: 5000"), "{log}");
        assert!(!log.contains("ALERT"), "{log}");
        let calls = exec.calls.borrow();
        assert_eq!(calls.iter().filter(|c| **c == ssh_key()).count(), 2, "{calls:?}");
        assert!(!calls.iter().any(|c| c.starts_with("notify-user")), "no banner on a healthy run: {calls:?}");
        let v = heartbeat_json(&paths);
        assert_eq!(v["interval_secs"], 300);
        assert!(v["last_error"].is_null(), "{v}");
        assert_eq!(v["actions"], 1);
        assert_eq!(v["watcher"], "syncthing-sync-watchdog");
    }

    #[test]
    fn red_control_never_arrives_is_a_notified_stall_with_exit_zero() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("Forge")).unwrap();
        let paths = Paths::under(d.path());
        let fake = fake_ssh(CmdResult::failure(255, "ssh: connect timed out"));
        let clock = FakeClock::new(7000, 10);
        let code = run(&fake, &clock, &paths);
        assert_eq!(code, 0, "a stall is a finding, not a failed run");
        let status = std::fs::read_to_string(&paths.status).unwrap();
        assert!(status.starts_with("STALLED: Sync stalled: heartbeat 7000 did not reach Nimbini within 60s"), "{status}");
        let log = std::fs::read_to_string(&paths.log).unwrap();
        assert!(log.contains("ALERT: Sync stalled: heartbeat 7000"), "{log}");
        let calls = fake.calls.borrow();
        assert!(
            calls.iter().any(|c| c.starts_with("notify-user --tool syncthing-sync-watchdog --urgency critical Syncthing Watchdog Sync stalled")),
            "{calls:?}"
        );
        assert!(clock.slept.get() >= 5, "polled with sleeps between attempts");
        let v = heartbeat_json(&paths);
        assert_eq!(v["last_error"].as_str().unwrap(), "Sync stalled: heartbeat 7000 did not reach Nimbini within 60s");
        assert_eq!(v["actions"], 0);
    }

    #[test]
    fn a_stale_remote_value_does_not_count() {
        let fake = fake_ssh(CmdResult::success("4999\n"));
        let clock = FakeClock::new(5000, 20);
        assert!(matches!(check_remote(&fake, &clock, 5000), Verdict::Stalled(_)));
    }

    #[test]
    fn unwritable_heartbeat_is_a_failed_run() {
        let d = tempfile::tempdir().unwrap();
        // No Forge directory → the heartbeat write fails.
        let paths = Paths::under(d.path());
        let fake = Fake::default();
        let clock = FakeClock::new(1, 1);
        let code = run(&fake, &clock, &paths);
        assert_eq!(code, 1);
        assert!(fake.calls.borrow().is_empty(), "no ssh, no banner when the tool could not even write");
        let log = std::fs::read_to_string(&paths.log).unwrap();
        assert!(log.contains("ERROR: could not write heartbeat"), "{log}");
        let v = heartbeat_json(&paths);
        assert!(v["last_error"].as_str().unwrap().contains("could not write heartbeat"), "{v}");
    }

    #[test]
    fn log_is_capped_with_one_predecessor() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("Forge")).unwrap();
        let paths = Paths::under(d.path());
        std::fs::create_dir_all(paths.log.parent().unwrap()).unwrap();
        std::fs::write(&paths.log, vec![b'x'; (LOG_CAP + 1) as usize]).unwrap();
        let fake = fake_ssh(CmdResult::success("99\n"));
        assert_eq!(run(&fake, &FakeClock::new(1, 1), &paths), 0);
        assert!(std::fs::metadata(logkeep::predecessor(&paths.log)).unwrap().len() > LOG_CAP);
        let fresh = std::fs::read_to_string(&paths.log).unwrap();
        assert!(fresh.contains("log capped at 5 MB"), "{fresh}");
        assert!(fresh.len() < 1000);
    }
}
