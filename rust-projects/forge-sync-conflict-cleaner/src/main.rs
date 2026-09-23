//! forge-sync-conflict-cleaner — deletes Syncthing `sync-conflict` files under
//! `~/Forge` that are older than 7 days (the review window). Rust port
//! (2026-09-23) of the bash script of the same name; the launchd plist
//! `com.user.forge-sync-conflict-cleaner` still runs
//! `~/.local/bin/forge-sync-conflict-cleaner` daily at 04:00, unchanged.
//!
//! As the script: walk `~/Forge` for regular files whose name contains
//! `sync-conflict` (any position — Syncthing writes
//! `name.sync-conflict-YYYYMMDD-HHMMSS-DEVICE.ext`) with an mtime older than
//! 7 days; delete each with a `DELETED: <path>` log line; then either
//! `No sync-conflict files older than 7 days found` or `Cleaned N sync-conflict file(s)`.
//! Symlinks are never followed (a linked directory could point outside Forge).
//!
//! Semantic differences from the script:
//! - A delete that fails is recorded (`FAILED: <path>: <reason>` in the log,
//!   `last_error`, exit 1) instead of `rm -f` silence; the walk continues.
//! - A missing `~/Forge` still logs `ERROR: … does not exist` and exits 1, and
//!   now also lands in `last_error`.
//! - Recorded outcome: `~/.local/state/watchers/forge-sync-conflict-cleaner.json`
//!   (interval_secs 86400), `actions` = files deleted.
//! - The tool-owned log `~/.local/share/forge-sync-conflict-cleaner.log` is
//!   capped at 5 MB with one predecessor `.1` (audit D2-24).
//! - `find`'s `-mtime +7` rounds to whole days; here "older than 7 days" is a
//!   plain 7×24 h comparison, so a file between 7 and 8 days old is deleted
//!   one run earlier than before.
//!
//! Exit: 0 ran (whether or not anything was deleted); 1 Forge missing or any
//! delete failed.

mod exec;
mod outcome;

use clap::Parser;
use exec::Exec;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const NAME: &str = "forge-sync-conflict-cleaner";
const INTERVAL_SECS: u64 = 86400;
const AGE_DAYS: u64 = 7;
const LOG_CAP: u64 = 5 * logkeep::MB;

#[derive(Parser)]
#[command(name = NAME, version, about = "Delete Syncthing sync-conflict files older than 7 days from ~/Forge")]
struct Cli {}

pub struct Paths {
    forge: PathBuf,
    log: PathBuf,
    state_dir: PathBuf,
}

impl Paths {
    fn under(home: &Path) -> Paths {
        Paths { forge: home.join("Forge"), log: home.join(".local/share/forge-sync-conflict-cleaner.log"), state_dir: outcome::state_dir(home) }
    }
}

fn log(paths: &Paths, line: &str) {
    use std::io::Write;
    if let Some(d) = paths.log.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&paths.log) {
        let _ = writeln!(f, "{} {line}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}

/// Regular files under `root` (symlinks not followed) whose name contains
/// `sync-conflict` and whose mtime is before `cutoff`. Sorted for stable logs.
pub fn stale_conflicts(root: &Path, cutoff: SystemTime) -> Vec<PathBuf> {
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            // symlink_metadata: a symlink is neither descended nor deleted.
            let Ok(md) = std::fs::symlink_metadata(&p) else { continue };
            if md.is_dir() {
                stack.push(p);
            } else if md.is_file() {
                let is_conflict = p.file_name().and_then(|n| n.to_str()).map(|n| n.contains("sync-conflict")).unwrap_or(false);
                if is_conflict && md.modified().map(|m| m < cutoff).unwrap_or(false) {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

fn notify(exec: &dyn Exec, msg: &str) {
    let _ = exec.run("notify-user", &["--tool", NAME, "forge-sync-conflict-cleaner failed", msg]);
}

/// One run. Returns the process exit code.
pub fn run(exec: &dyn Exec, paths: &Paths, now: SystemTime) -> i32 {
    let started_at = outcome::now_rfc3339();
    match logkeep::cap(&paths.log, LOG_CAP) {
        Ok(o) if o.rolled() => log(paths, &format!("log capped at 5 MB; predecessor {}", logkeep::predecessor(&paths.log).display())),
        Ok(_) => {}
        Err(e) => log(paths, &format!("could not cap log: {e}")),
    }

    if !paths.forge.is_dir() {
        let msg = format!("{} does not exist", paths.forge.display());
        log(paths, &format!("ERROR: {msg}"));
        notify(exec, &msg);
        record(paths, started_at, None, 0, Some(msg));
        return 1;
    }

    let cutoff = now - Duration::from_secs(AGE_DAYS * 86400);
    let mut count = 0u64;
    let mut failures = vec![];
    for f in stale_conflicts(&paths.forge, cutoff) {
        match std::fs::remove_file(&f) {
            Ok(()) => {
                log(paths, &format!("DELETED: {}", f.display()));
                count += 1;
            }
            Err(e) => {
                log(paths, &format!("FAILED: {}: {e}", f.display()));
                failures.push(format!("{}: {e}", f.display()));
            }
        }
    }

    if count == 0 {
        log(paths, &format!("No sync-conflict files older than {AGE_DAYS} days found"));
    } else {
        log(paths, &format!("Cleaned {count} sync-conflict file(s)"));
    }

    if failures.is_empty() {
        let action = (count > 0).then(|| format!("deleted {count} sync-conflict file(s)"));
        record(paths, started_at, action, count, None);
        0
    } else {
        let msg = format!("{} delete(s) failed: {}", failures.len(), failures.join("; "));
        notify(exec, &msg);
        record(paths, started_at, None, count, Some(msg));
        1
    }
}

fn record(paths: &Paths, started_at: String, last_action: Option<String>, actions: u64, last_error: Option<String>) {
    let o = outcome::Outcome { name: NAME, interval_secs: INTERVAL_SECS, started_at, last_action, actions, last_error };
    if let Err(e) = outcome::record(&paths.state_dir, &o) {
        log(paths, &format!("could not record outcome: {e}"));
    }
}

fn main() {
    let _ = Cli::parse();
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is not set"));
    std::process::exit(run(&exec::Real, &Paths::under(&home), SystemTime::now()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use exec::Fake;

    fn touch(p: &Path, age: Duration) {
        std::fs::write(p, b"x").unwrap();
        let f = std::fs::File::options().write(true).open(p).unwrap();
        f.set_modified(SystemTime::now() - age).unwrap();
    }
    fn days(n: u64) -> Duration {
        Duration::from_secs(n * 86400)
    }
    fn heartbeat_json(paths: &Paths) -> serde_json::Value {
        let hb = std::fs::read_to_string(paths.state_dir.join("forge-sync-conflict-cleaner.json")).unwrap();
        serde_json::from_str(&hb).unwrap()
    }

    #[test]
    fn green_control_only_old_conflict_files_are_deleted() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::under(d.path());
        let sub = paths.forge.join("Notes/deep");
        std::fs::create_dir_all(&sub).unwrap();
        let old_conflict = sub.join("note.sync-conflict-20260901-120000-ABCDEFG.md");
        let fresh_conflict = paths.forge.join("today.sync-conflict-20260922-090000-ABCDEFG.md");
        let old_plain = paths.forge.join("Notes/keep-me.md");
        touch(&old_conflict, days(10));
        touch(&fresh_conflict, days(2));
        touch(&old_plain, days(30));
        // A symlink named like a conflict must not be deleted.
        let link = paths.forge.join("link.sync-conflict-1.md");
        std::os::unix::fs::symlink(&old_plain, &link).unwrap();

        let fake = Fake::default();
        assert_eq!(run(&fake, &paths, SystemTime::now()), 0);
        assert!(!old_conflict.exists(), "old conflict deleted");
        assert!(fresh_conflict.exists(), "fresh conflict kept for review");
        assert!(old_plain.exists(), "ordinary old file untouched");
        assert!(std::fs::symlink_metadata(&link).is_ok(), "symlink untouched");
        let log = std::fs::read_to_string(&paths.log).unwrap();
        assert!(log.contains(&format!("DELETED: {}", old_conflict.display())), "{log}");
        assert!(log.contains("Cleaned 1 sync-conflict file(s)"), "{log}");
        assert!(fake.calls.borrow().is_empty(), "no banner on a clean run");
        let v = heartbeat_json(&paths);
        assert_eq!(v["interval_secs"], 86400);
        assert_eq!(v["actions"], 1);
        assert!(v["last_error"].is_null(), "{v}");
    }

    #[test]
    fn empty_forge_logs_the_nothing_found_line() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::under(d.path());
        std::fs::create_dir_all(&paths.forge).unwrap();
        assert_eq!(run(&Fake::default(), &paths, SystemTime::now()), 0);
        let log = std::fs::read_to_string(&paths.log).unwrap();
        assert!(log.contains("No sync-conflict files older than 7 days found"), "{log}");
        assert_eq!(heartbeat_json(&paths)["actions"], 0);
    }

    #[test]
    fn red_control_missing_forge_is_exit_one_and_recorded() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::under(d.path());
        let fake = Fake::default();
        assert_eq!(run(&fake, &paths, SystemTime::now()), 1);
        let log = std::fs::read_to_string(&paths.log).unwrap();
        assert!(log.contains(&format!("ERROR: {} does not exist", paths.forge.display())), "{log}");
        assert!(fake.calls.borrow().iter().any(|c| c.starts_with("notify-user --tool forge-sync-conflict-cleaner")));
        assert!(heartbeat_json(&paths)["last_error"].as_str().unwrap().ends_with("does not exist"));
    }

    #[test]
    fn red_control_a_delete_that_fails_is_exit_one_not_silence() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::under(d.path());
        let locked = paths.forge.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        let victim = locked.join("a.sync-conflict-20260101-000000-X.md");
        touch(&victim, days(20));
        let free = paths.forge.join("b.sync-conflict-20260101-000000-X.md");
        touch(&free, days(20));
        // Read-only directory: unlink fails with EACCES.
        std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o555)).unwrap();
        let fake = Fake::default();
        let code = run(&fake, &paths, SystemTime::now());
        std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        assert_eq!(code, 1);
        assert!(victim.exists());
        assert!(!free.exists(), "the walk continues past a failure");
        let log = std::fs::read_to_string(&paths.log).unwrap();
        assert!(log.contains(&format!("FAILED: {}", victim.display())), "{log}");
        assert!(log.contains("Cleaned 1 sync-conflict file(s)"), "{log}");
        let v = heartbeat_json(&paths);
        assert!(v["last_error"].as_str().unwrap().starts_with("1 delete(s) failed"), "{v}");
        assert_eq!(v["actions"], 1);
        assert!(fake.calls.borrow().iter().any(|c| c.starts_with("notify-user --tool forge-sync-conflict-cleaner")));
    }

    #[test]
    fn cutoff_is_seven_full_days() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let just_under = root.join("u.sync-conflict-1");
        let just_over = root.join("o.sync-conflict-1");
        touch(&just_under, days(7) - Duration::from_secs(60));
        touch(&just_over, days(7) + Duration::from_secs(60));
        let found = stale_conflicts(root, SystemTime::now() - days(7));
        assert_eq!(found, vec![just_over]);
    }

    #[test]
    fn log_is_capped_with_one_predecessor() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::under(d.path());
        std::fs::create_dir_all(&paths.forge).unwrap();
        std::fs::create_dir_all(paths.log.parent().unwrap()).unwrap();
        std::fs::write(&paths.log, vec![b'x'; (LOG_CAP + 1) as usize]).unwrap();
        assert_eq!(run(&Fake::default(), &paths, SystemTime::now()), 0);
        assert!(std::fs::metadata(logkeep::predecessor(&paths.log)).unwrap().len() > LOG_CAP);
        assert!(std::fs::read_to_string(&paths.log).unwrap().contains("log capped at 5 MB"));
    }
}
