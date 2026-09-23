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
//! Exit status: 0 dump written; 1 pizauth failed or produced no bytes.
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
    let r = exec.run(&bin.to_string_lossy(), &["dump"]);
    if !r.ok() {
        return Verdict::Failed(format!("pizauth dump exited {}", r.exit_code));
    }
    if r.stdout.is_empty() {
        return Verdict::Failed("pizauth dump exited 0 but produced no bytes".into());
    }
    let tmp = dir.join(format!(".pizauth-state.bin.{}.tmp", std::process::id()));
    if let Err(e) = write_private(&tmp, r.stdout.as_bytes()) {
        let _ = std::fs::remove_file(&tmp);
        return Verdict::Failed(format!("cannot write {}: {e}", tmp.display()));
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

    fn fake(home: &Path, r: CmdResult) -> Fake {
        let mut f = Fake::default();
        f.respond(&pizauth_bin(home).to_string_lossy(), &["dump"], r);
        f
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

    #[test]
    fn unscripted_pizauth_is_reported_not_swallowed() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake::default();
        assert_eq!(run_dump(&f, d.path()), Verdict::Failed("pizauth dump exited 127".into()));
        assert!(!dump_file(d.path()).exists());
    }
}
