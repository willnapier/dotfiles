//! pizauth-server-wrapper — supervisor entry point for the pizauth OAuth broker.
//! Rust port (2026-09-23) of the bash script of the same name; started by
//! `com.williamnapier.pizauth` (Mac, KeepAlive) and `pizauth.service`
//! (nimbini, Restart=on-failure); the ExecStart path is unchanged.
//!
//! Behaviour kept, in order:
//!   1. spawn `~/.local/bin/pizauth server -d` (foreground daemon) with
//!      inherited stdio, so the supervisor observes the process lifecycle;
//!   2. poll `pizauth status` every 500 ms, up to 10 tries, for the socket;
//!   3. if `~/.cache/pizauth-state.bin` exists, pipe it to `pizauth restore`
//!      so refresh tokens survive restarts, reboots and logouts;
//!      "pizauth-wrapper: restored state from …" on success, "pizauth-wrapper:
//!      restore failed (continuing with empty daemon)" on stderr otherwise;
//!   4. wait on the child and exit with its exit code.
//!
//! Semantic changes versus the script:
//! - A failed restore raises `notify-user --tool pizauth-server-wrapper`
//!   (critical). The script printed one line into a log nobody reads and the
//!   daemon then ran with no tokens until the next `pizauth show` failed —
//!   "reboot wipes pizauth tokens" was this silent path.
//! - The socket never answering within the window is logged
//!   ("pizauth-wrapper: socket not responsive after N tries (continuing)");
//!   the restore is still attempted, as the script did.
//! - The outcome of the restore step is recorded once per start to
//!   `~/.local/state/watchers/pizauth-server-wrapper.json` (interval_secs 0:
//!   a supervisor, not a timer) with `last_error` set on restore failure.
//! - Child killed by a signal → exit 1 (bash `wait` would have returned 128+n).
//!
//! Exit status: the daemon's own exit code; 1 if it was killed by a signal or
//! could not be spawned. The dump content is secret: never logged or printed.

mod exec;
mod outcome;

use clap::Parser;
use exec::{CmdResult, Exec};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const NAME: &str = "pizauth-server-wrapper";
const SOCKET_TRIES: usize = 10;
const SOCKET_POLL_MS: u64 = 500;

#[derive(Parser)]
#[command(name = NAME, version, about = "Run pizauth server in the foreground, restore saved state, hold its lifetime for launchd/systemd")]
struct Cli {}

/// A command fed bytes on stdin — the one shape `Exec` lacks. Real for the
/// daemon, canned in tests so no dump bytes and no server are needed.
pub trait Piped {
    fn run_with_stdin(&self, program: &str, args: &[&str], stdin: &[u8]) -> CmdResult;
}

pub struct RealPiped;

impl Piped for RealPiped {
    fn run_with_stdin(&self, program: &str, args: &[&str], stdin: &[u8]) -> CmdResult {
        use std::io::Write;
        let child = Command::new(program).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => return CmdResult { exit_code: 127, stdout: String::new(), stderr: e.to_string() },
        };
        if let Some(mut si) = child.stdin.take() {
            let _ = si.write_all(stdin);
        }
        match child.wait_with_output() {
            Ok(o) => CmdResult {
                exit_code: o.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            },
            Err(e) => CmdResult { exit_code: -1, stdout: String::new(), stderr: e.to_string() },
        }
    }
}

fn dump_file(home: &Path) -> PathBuf {
    home.join(".cache/pizauth-state.bin")
}

fn pizauth_bin(home: &Path) -> PathBuf {
    home.join(".local/bin/pizauth")
}

/// Poll `pizauth status` until it answers or the tries run out. `sleep` is
/// injected so tests do not wait.
fn wait_for_socket(exec: &dyn Exec, bin: &str, tries: usize, sleep: &dyn Fn()) -> bool {
    for i in 0..tries {
        if exec.run(bin, &["status"]).ok() {
            return true;
        }
        if i + 1 < tries {
            sleep();
        }
    }
    false
}

#[derive(Debug, PartialEq, Eq)]
enum Restore {
    Restored,
    NoDump,
    Failed(String),
}

/// Step 3: restore the last dump if there is one. The dump bytes go straight
/// from disk to the daemon's stdin and nowhere else.
fn restore_step(piped: &dyn Piped, home: &Path) -> Restore {
    let dump = dump_file(home);
    let bytes = match std::fs::read(&dump) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Restore::NoDump,
        Err(e) => return Restore::Failed(format!("cannot read {}: {e}", dump.display())),
    };
    let bin = pizauth_bin(home);
    let r = piped.run_with_stdin(&bin.to_string_lossy(), &["restore"], &bytes);
    if r.ok() {
        Restore::Restored
    } else {
        Restore::Failed(format!("pizauth restore exited {}", r.exit_code))
    }
}

/// Human lines and the heartbeat fields for a restore outcome. Pure, so the
/// red/green controls assert on it directly.
struct Report {
    stdout: Option<String>,
    stderr: Option<String>,
    notify: bool,
    last_action: &'static str,
    actions: u64,
    last_error: Option<String>,
}

fn report(r: &Restore, home: &Path) -> Report {
    match r {
        Restore::Restored => Report {
            stdout: Some(format!("pizauth-wrapper: restored state from {}", dump_file(home).display())),
            stderr: None,
            notify: false,
            last_action: "restored",
            actions: 1,
            last_error: None,
        },
        Restore::NoDump => Report { stdout: None, stderr: None, notify: false, last_action: "no dump", actions: 0, last_error: None },
        Restore::Failed(why) => Report {
            stdout: None,
            stderr: Some("pizauth-wrapper: restore failed (continuing with empty daemon)".into()),
            notify: true,
            last_action: "restore failed",
            actions: 0,
            last_error: Some(why.clone()),
        },
    }
}

fn notify(exec: &dyn Exec, why: &str) {
    let body = format!("{why} — the broker is running with no tokens; re-auth the accounts (pizauth-dump will snapshot them again)");
    let _ = exec.run("notify-user", &["--tool", NAME, "--urgency", "critical", "pizauth restore failed", &body]);
}

/// Steps 2–3 with their reporting; returns nothing the caller needs beyond
/// side effects. Split from `main` so the whole path after the spawn is tested.
fn after_spawn(exec: &dyn Exec, piped: &dyn Piped, home: &Path, sleep: &dyn Fn(), state_dir: &Path, started_at: String) {
    let bin = pizauth_bin(home);
    if !wait_for_socket(exec, &bin.to_string_lossy(), SOCKET_TRIES, sleep) {
        eprintln!("pizauth-wrapper: socket not responsive after {SOCKET_TRIES} tries (continuing)");
    }
    let r = restore_step(piped, home);
    let rep = report(&r, home);
    if let Some(s) = &rep.stdout {
        println!("{s}");
    }
    if let Some(s) = &rep.stderr {
        eprintln!("{s}");
    }
    if rep.notify {
        notify(exec, rep.last_error.as_deref().unwrap_or("restore failed"));
    }
    let o = outcome::Outcome {
        name: NAME,
        interval_secs: 0,
        started_at,
        last_action: Some(rep.last_action.to_string()),
        actions: rep.actions,
        last_error: rep.last_error,
    };
    if let Err(e) = outcome::record(state_dir, &o) {
        eprintln!("pizauth-wrapper: could not record outcome: {e}");
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME is set"))
}

fn main() {
    let _ = Cli::parse();
    let home = home();
    let started_at = outcome::now_rfc3339();
    let bin = pizauth_bin(&home);
    // Step 1: the daemon, foregrounded via -d (inverse-named flag: *don't*
    // daemonise), stdio inherited so its output lands in the supervisor's log.
    let mut child = match Command::new(&bin).args(["server", "-d"]).spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("pizauth-wrapper: cannot start {}: {e}", bin.display());
            let o = outcome::Outcome {
                name: NAME,
                interval_secs: 0,
                started_at,
                last_action: None,
                actions: 0,
                last_error: Some(format!("cannot start pizauth server: {e}")),
            };
            let _ = outcome::record(&outcome::state_dir(&home), &o);
            std::process::exit(1);
        }
    };
    after_spawn(
        &exec::Real,
        &RealPiped,
        &home,
        &|| std::thread::sleep(std::time::Duration::from_millis(SOCKET_POLL_MS)),
        &outcome::state_dir(&home),
        started_at,
    );
    // Step 4: hand the process's lifetime back to the supervisor.
    let code = match child.wait() {
        Ok(status) => status.code().unwrap_or(1),
        Err(e) => {
            eprintln!("pizauth-wrapper: wait failed: {e}");
            1
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;
    use exec::Fake;
    use std::cell::{Cell, RefCell};

    /// `pizauth status` fails `fail_first` times, then succeeds.
    struct FlakyStatus {
        fail_first: usize,
        calls: Cell<usize>,
    }
    impl Exec for FlakyStatus {
        fn run(&self, _program: &str, args: &[&str]) -> CmdResult {
            assert_eq!(args, ["status"]);
            let n = self.calls.get() + 1;
            self.calls.set(n);
            if n > self.fail_first {
                CmdResult::success("ok")
            } else {
                CmdResult::failure(1, "no socket")
            }
        }
    }

    struct FakePiped {
        exit: i32,
        calls: RefCell<Vec<(String, usize)>>,
    }
    impl Piped for FakePiped {
        fn run_with_stdin(&self, _program: &str, args: &[&str], stdin: &[u8]) -> CmdResult {
            self.calls.borrow_mut().push((args.join(" "), stdin.len()));
            if self.exit == 0 {
                CmdResult::success("")
            } else {
                CmdResult::failure(self.exit, "bad")
            }
        }
    }

    fn with_dump(home: &Path) {
        std::fs::create_dir_all(home.join(".cache")).unwrap();
        std::fs::write(dump_file(home), b"synthetic-dump-bytes").unwrap();
    }

    #[test]
    fn socket_poll_succeeds_on_the_third_try_and_sleeps_between() {
        let ex = FlakyStatus { fail_first: 2, calls: Cell::new(0) };
        let slept = Cell::new(0);
        assert!(wait_for_socket(&ex, "pizauth", SOCKET_TRIES, &|| slept.set(slept.get() + 1)));
        assert_eq!(ex.calls.get(), 3);
        assert_eq!(slept.get(), 2);
    }

    #[test]
    fn socket_poll_gives_up_after_the_tries() {
        let ex = FlakyStatus { fail_first: 99, calls: Cell::new(0) };
        assert!(!wait_for_socket(&ex, "pizauth", SOCKET_TRIES, &|| {}));
        assert_eq!(ex.calls.get(), SOCKET_TRIES);
    }

    #[test]
    fn green_dump_present_and_restore_ok() {
        let d = tempfile::tempdir().unwrap();
        with_dump(d.path());
        let p = FakePiped { exit: 0, calls: RefCell::new(vec![]) };
        let r = restore_step(&p, d.path());
        assert_eq!(r, Restore::Restored);
        assert_eq!(p.calls.borrow().as_slice(), [("restore".to_string(), "synthetic-dump-bytes".len())]);
        let rep = report(&r, d.path());
        assert!(rep.stdout.unwrap().starts_with("pizauth-wrapper: restored state from "));
        assert!(rep.stderr.is_none() && !rep.notify && rep.last_error.is_none());
        assert_eq!((rep.last_action, rep.actions), ("restored", 1));
    }

    #[test]
    fn red_dump_present_but_restore_fails_alerts_and_records() {
        let d = tempfile::tempdir().unwrap();
        with_dump(d.path());
        let p = FakePiped { exit: 3, calls: RefCell::new(vec![]) };
        let r = restore_step(&p, d.path());
        assert_eq!(r, Restore::Failed("pizauth restore exited 3".into()));
        let rep = report(&r, d.path());
        assert_eq!(rep.stderr.as_deref(), Some("pizauth-wrapper: restore failed (continuing with empty daemon)"));
        assert!(rep.notify);
        assert_eq!(rep.last_error.as_deref(), Some("pizauth restore exited 3"));
        // The full post-spawn path: notify-user is called and the heartbeat carries the error.
        let mut ex = Fake::default();
        ex.respond(&pizauth_bin(d.path()).to_string_lossy(), &["status"], CmdResult::success("ok"));
        let state = d.path().join("state");
        after_spawn(&ex, &p, d.path(), &|| {}, &state, outcome::now_rfc3339());
        let calls = ex.calls.borrow();
        assert!(calls.iter().any(|c| c.starts_with("notify-user --tool pizauth-server-wrapper --urgency critical")), "{calls:?}");
        let hb: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(state.join("pizauth-server-wrapper.json")).unwrap()).unwrap();
        assert_eq!(hb["last_error"], "pizauth restore exited 3");
        assert_eq!(hb["interval_secs"], 0);
    }

    #[test]
    fn no_dump_means_no_restore_call_and_a_clean_heartbeat() {
        let d = tempfile::tempdir().unwrap();
        let p = FakePiped { exit: 0, calls: RefCell::new(vec![]) };
        assert_eq!(restore_step(&p, d.path()), Restore::NoDump);
        assert!(p.calls.borrow().is_empty(), "restore must not run without a dump");
        let mut ex = Fake::default();
        ex.respond(&pizauth_bin(d.path()).to_string_lossy(), &["status"], CmdResult::success("ok"));
        let state = d.path().join("state");
        after_spawn(&ex, &p, d.path(), &|| {}, &state, outcome::now_rfc3339());
        assert!(!ex.calls.borrow().iter().any(|c| c.starts_with("notify-user")));
        let hb: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(state.join("pizauth-server-wrapper.json")).unwrap()).unwrap();
        assert!(hb["last_error"].is_null());
        assert_eq!(hb["last_action"], "no dump");
    }
}
