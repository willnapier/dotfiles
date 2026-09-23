//! pizauth-dump — write the pizauth daemon's encrypted state to disk atomically.
//! Rust port (2026-09-23) of the bash script of the same name; invoked every
//! 15 minutes by `com.williamnapier.pizauth-dump` (Mac) and
//! `pizauth-dump.timer` (nimbini); the ExecStart path is unchanged.
//!
//! Behaviour kept: `~/.local/bin/pizauth dump` → tmp file in the same
//! directory as `~/.cache/pizauth-state.bin`, mode 0600, then rename, so a
//! reader never sees a partial file. On failure the prior dump is untouched,
//! the tmp is removed, the same stderr line is printed, exit 1.
//!
//! Semantic changes versus the script:
//! - An EMPTY dump on exit 0 is a failure (exit 1). The script would have
//!   replaced a good state file with nothing, and the wrapper's next restore
//!   would then silently start the daemon with no tokens.
//! - Failures raise `notify-user --tool pizauth-dump` (recorded, D2-23).
//! - Every run records its outcome to `~/.local/state/watchers/pizauth-dump.json`
//!   (interval_secs 900), so a timer that stops firing or a run that keeps
//!   failing reaches system-health-check Check 9.
//!
//! 0.3.0 (2026-09-23, after the corrupt-dump incident): every dump keeps ONE
//! predecessor, `~/.cache/pizauth-state.bin.1` (a hard link to the previous
//! generation, so the live file is never absent), and a broker that holds no
//! tokens at all — every account "No access token" or "pending authentication",
//! as `pizauth status` reports it — is refused: the prior dump is the recovery
//! material and a dump of an empty broker would erase it. That refusal alerts
//! only when a prior dump exists (nothing to protect otherwise), but always
//! exits 1 and records last_error.
//!
//! Exit status: 0 dump written; 1 pizauth failed, produced no bytes, or holds
//! no tokens.
//! 0.2.1: the dump is copied as raw bytes (0.2.0 passed it through a lossy
//! UTF-8 conversion and wrote a file `pizauth restore` could not read).
//! The dump content is secret: it is never logged or printed.

mod exec;
mod outcome;

use clap::Parser;
use exec::Exec;
use std::path::{Path, PathBuf};

const NAME: &str = "pizauth-dump";
const INTERVAL_SECS: u64 = 900;

#[derive(Parser)]
#[command(name = NAME, version, about = "Write pizauth daemon state to disk atomically (mode 0600)")]
struct Cli {}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// Bytes written to the dump file.
    Written(usize),
    /// pizauth failed (exit code) or produced nothing; prior dump kept.
    Failed(String),
    /// The broker holds no tokens; the prior dump is kept. `bool` = a prior
    /// dump exists (worth a banner).
    Refused(bool),
}

pub fn predecessor(dump: &Path) -> PathBuf {
    let mut s = dump.as_os_str().to_owned();
    s.push(".1");
    PathBuf::from(s)
}

/// Does `pizauth status` output show any account with a token? An account
/// whose access token is merely expired still has a refresh token and counts.
/// Empty or unparseable output counts as "unknown" → true, so a broken status
/// command never blocks a dump (the dump itself will fail if the broker is down).
pub fn broker_holds_tokens(status: &str) -> bool {
    let lines: Vec<&str> = status.lines().filter(|l| l.contains(": ")).collect();
    if lines.is_empty() {
        return true;
    }
    lines.iter().any(|l| l.contains("Active access token") || l.contains("Access token expired"))
}

fn dump_file(home: &Path) -> PathBuf {
    home.join(".cache/pizauth-state.bin")
}

fn pizauth_bin(home: &Path) -> PathBuf {
    home.join(".local/bin/pizauth")
}

/// Run the dump and swap the file in atomically. Pure with respect to the
/// command layer, so tests can can `pizauth dump`.
fn run_dump(exec: &dyn Exec, home: &Path) -> Verdict {
    let dump = dump_file(home);
    let dir = dump.parent().expect("dump file has a parent directory");
    if let Err(e) = std::fs::create_dir_all(dir) {
        return Verdict::Failed(format!("cannot create {}: {e}", dir.display()));
    }
    let bin = pizauth_bin(home);
    let status = exec.run(&bin.to_string_lossy(), &["status"]);
    if status.ok() && !broker_holds_tokens(&status.stdout) {
        return Verdict::Refused(dump.is_file());
    }
    // Raw bytes: the dump is an encrypted blob, and `from_utf8_lossy` would
    // rewrite every non-UTF-8 sequence as U+FFFD (63 of them in the first
    // install's dump, 2026-09-23 — `pizauth restore` refused it).
    let r = exec.run_raw(&bin.to_string_lossy(), &["dump"]);
    if !r.ok() {
        return Verdict::Failed(format!("pizauth dump exited {}", r.exit_code));
    }
    if r.stdout.is_empty() {
        return Verdict::Failed("pizauth dump exited 0 but produced no bytes".into());
    }
    let tmp = dir.join(format!(".pizauth-state.bin.{}.tmp", std::process::id()));
    if let Err(e) = write_private(&tmp, &r.stdout) {
        let _ = std::fs::remove_file(&tmp);
        return Verdict::Failed(format!("cannot write {}: {e}", tmp.display()));
    }
    // Keep exactly one predecessor: link the current generation to `.1`
    // (replacing the old `.1`) before the new one takes its place.
    if dump.is_file() {
        let prev = predecessor(&dump);
        let _ = std::fs::remove_file(&prev);
        if std::fs::hard_link(&dump, &prev).is_err() {
            if let Err(e) = std::fs::copy(&dump, &prev) {
                let _ = std::fs::remove_file(&tmp);
                return Verdict::Failed(format!("cannot keep predecessor {}: {e}", prev.display()));
            }
        }
    }
    if let Err(e) = std::fs::rename(&tmp, &dump) {
        let _ = std::fs::remove_file(&tmp);
        return Verdict::Failed(format!("cannot rename into {}: {e}", dump.display()));
    }
    Verdict::Written(r.stdout.len())
}

/// Create the file with mode 0600 before any bytes land in it.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

fn notify(exec: &dyn Exec, body: &str) {
    let _ = exec.run("notify-user", &["--tool", NAME, "pizauth-dump failed", body]);
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME is set"))
}

fn main() {
    let _ = Cli::parse();
    let home = home();
    let started_at = outcome::now_rfc3339();
    let verdict = run_dump(&exec::Real, &home);
    let (code, last_action, actions, last_error) = match &verdict {
        Verdict::Written(n) => (0, Some(format!("dumped {n} bytes")), 1, None),
        Verdict::Failed(why) => {
            eprintln!("pizauth-dump: pizauth dump failed, leaving prior {} untouched ({why})", dump_file(&home).display());
            notify(&exec::Real, why);
            (1, None, 0, Some(why.clone()))
        }
        Verdict::Refused(prior_exists) => {
            let why = "broker holds no tokens — refusing to overwrite the prior dump; authenticate the accounts".to_string();
            eprintln!("pizauth-dump: {why}");
            if *prior_exists {
                notify(&exec::Real, &why);
            }
            (1, None, 0, Some(why))
        }
    };
    let o = outcome::Outcome { name: NAME, interval_secs: INTERVAL_SECS, started_at, last_action, actions, last_error };
    if let Err(e) = outcome::record(&outcome::state_dir(&home), &o) {
        eprintln!("pizauth-dump: could not record outcome: {e}");
    }
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;
    use exec::{CmdResult, Fake};

    const STATUS_LIVE: &str = "cohs-graph: Active access token (obtained x; expires y)\ncohs: Access token expired (last refresh attempt z)\ngmail: Active access token (obtained x; expires y)\n";
    const STATUS_EMPTY: &str = "cohs-graph: No access token\ncohs: Access token pending authentication (last notification t)\ngmail: No access token\n";

    fn fake(home: &Path, r: CmdResult) -> Fake {
        let mut f = Fake::default();
        f.respond(&pizauth_bin(home).to_string_lossy(), &["dump"], r);
        f.respond(&pizauth_bin(home).to_string_lossy(), &["status"], CmdResult::success(STATUS_LIVE));
        f
    }

    #[test]
    fn status_parsing_expired_counts_as_held_pending_does_not() {
        assert!(broker_holds_tokens(STATUS_LIVE));
        assert!(!broker_holds_tokens(STATUS_EMPTY));
        assert!(broker_holds_tokens("gmail: Access token expired (x)\n"));
        assert!(broker_holds_tokens(""), "unknown output never blocks a dump");
        assert!(broker_holds_tokens("garbage"));
    }

    #[test]
    fn red_control_empty_broker_is_refused_and_prior_dump_kept() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        std::fs::create_dir_all(home.join(".cache")).unwrap();
        std::fs::write(dump_file(home), b"prior-good-state").unwrap();
        let mut f = Fake::default();
        f.respond(&pizauth_bin(home).to_string_lossy(), &["dump"], CmdResult::success("empty-broker-blob"));
        f.respond(&pizauth_bin(home).to_string_lossy(), &["status"], CmdResult::success(STATUS_EMPTY));
        assert_eq!(run_dump(&f, home), Verdict::Refused(true));
        assert_eq!(std::fs::read(dump_file(home)).unwrap(), b"prior-good-state");
        assert!(!f.calls.borrow().iter().any(|c| c.ends_with(" dump")), "dump never even run");
        // No prior dump: still refused, flagged as nothing-to-protect.
        std::fs::remove_file(dump_file(home)).unwrap();
        assert_eq!(run_dump(&f, home), Verdict::Refused(false));
    }

    #[test]
    fn exactly_one_predecessor_is_kept_across_generations() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        let f1 = fake(home, CmdResult::success("gen-1"));
        assert_eq!(run_dump(&f1, home), Verdict::Written(5));
        assert!(!predecessor(&dump_file(home)).exists(), "first dump has no predecessor");
        let f2 = fake(home, CmdResult::success("gen-2!"));
        assert_eq!(run_dump(&f2, home), Verdict::Written(6));
        assert_eq!(std::fs::read(predecessor(&dump_file(home))).unwrap(), b"gen-1");
        let f3 = fake(home, CmdResult::success("gen-3!!"));
        assert_eq!(run_dump(&f3, home), Verdict::Written(7));
        assert_eq!(std::fs::read(dump_file(home)).unwrap(), b"gen-3!!");
        assert_eq!(std::fs::read(predecessor(&dump_file(home))).unwrap(), b"gen-2!", "predecessor replaced, no .1.1 chain");
        assert!(!predecessor(&predecessor(&dump_file(home))).exists());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(predecessor(&dump_file(home))).unwrap().permissions().mode() & 0o777, 0o600);
    }

    fn mode(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn green_dump_is_written_0600_with_the_bytes() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        let f = fake(home, CmdResult::success("synthetic-encrypted-blob-0123"));
        assert_eq!(run_dump(&f, home), Verdict::Written("synthetic-encrypted-blob-0123".len()));
        let dump = dump_file(home);
        assert_eq!(std::fs::read(&dump).unwrap(), b"synthetic-encrypted-blob-0123");
        assert_eq!(mode(&dump), 0o600);
        assert!(leftovers(dump.parent().unwrap()).is_empty());
        let o = outcome::Outcome {
            name: NAME,
            interval_secs: INTERVAL_SECS,
            started_at: outcome::now_rfc3339(),
            last_action: Some("dumped 29 bytes".into()),
            actions: 1,
            last_error: None,
        };
        let p = outcome::record(&outcome::state_dir(home), &o).unwrap();
        let hb: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert!(hb["last_error"].is_null());
        assert_eq!(hb["interval_secs"], 900);
        assert_eq!(hb["watcher"], "pizauth-dump");
    }

    #[test]
    fn red_failed_dump_keeps_prior_file_byte_identical() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        std::fs::create_dir_all(home.join(".cache")).unwrap();
        std::fs::write(dump_file(home), b"prior-good-state").unwrap();
        let f = fake(home, CmdResult::failure(1, "no server"));
        let v = run_dump(&f, home);
        assert_eq!(v, Verdict::Failed("pizauth dump exited 1".into()));
        assert_eq!(std::fs::read(dump_file(home)).unwrap(), b"prior-good-state");
        assert!(leftovers(&home.join(".cache")).is_empty(), "no tmp left behind");
    }

    #[test]
    fn red_empty_dump_on_exit_zero_is_a_failure() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        std::fs::create_dir_all(home.join(".cache")).unwrap();
        std::fs::write(dump_file(home), b"prior-good-state").unwrap();
        let f = fake(home, CmdResult::success(""));
        assert!(matches!(run_dump(&f, home), Verdict::Failed(w) if w.contains("no bytes")));
        assert_eq!(std::fs::read(dump_file(home)).unwrap(), b"prior-good-state");
        assert!(leftovers(&home.join(".cache")).is_empty());
    }

    /// The 0.2.0 defect: bytes that are not valid UTF-8 must land on disk
    /// unchanged, with no U+FFFD substitution.
    #[test]
    fn red_control_non_utf8_dump_bytes_are_written_verbatim() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        let blob: Vec<u8> = (0u8..=255).chain([0xff, 0xfe, 0x80, 0xc0]).collect();
        let mut f = Fake::default();
        f.respond_raw(&pizauth_bin(home).to_string_lossy(), &["dump"], 0, &blob);
        f.respond(&pizauth_bin(home).to_string_lossy(), &["status"], CmdResult::success(STATUS_LIVE));
        assert_eq!(run_dump(&f, home), Verdict::Written(blob.len()));
        let on_disk = std::fs::read(dump_file(home)).unwrap();
        assert_eq!(on_disk, blob, "bytes must be verbatim");
        assert!(!on_disk.windows(3).any(|w| w == [0xef, 0xbf, 0xbd]), "no U+FFFD replacement characters");
    }

    #[test]
    fn unscripted_pizauth_is_reported_not_swallowed() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake::default();
        assert_eq!(run_dump(&f, d.path()), Verdict::Failed("pizauth dump exited 127".into()));
        assert!(!dump_file(d.path()).exists());
    }
}
