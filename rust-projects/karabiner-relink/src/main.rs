//! karabiner-relink — self-healing sync for `~/.config/karabiner/karabiner.json`.
//! Rust port (2026-09-23) of the bash script of the same name; run by
//! `com.williamnapier.karabiner-relink` (launchd WatchPaths on the deployed
//! file plus a 60 s StartInterval, Mac only), whose ExecStart path is unchanged.
//!
//! Karabiner-Elements writes its config via atomic rename (write tmp, mv to
//! target), which replaces the Dotter symlink with a real file. Without
//! intervention, GUI-side edits diverge silently from the Dotter-managed
//! source of truth in `~/dotfiles/karabiner/karabiner.json`. Each run:
//!
//! 1. deployed path already a symlink → nothing to do (idempotent; avoids
//!    ping-pong between our own write and the kernel notify);
//! 2. deployed path missing → `noop: deployed file missing`;
//! 3. otherwise: if the bytes differ from the dotfile, copy them into the
//!    dotfile (`capture: …`); replace the real file with a symlink
//!    (`relink: …`); if the dotfile changed, `git add` + `git commit` in
//!    `~/dotfiles` and a `git push` bounded to 30 s (the script's
//!    `timeout 30 git push`; here a spawned child killed at the deadline);
//!    `done`.
//!
//! Semantic differences from the script:
//! - A failed capture copy, file removal or symlink creation is now
//!   `last_error` + exit 1 (the script had `set -u` only and would carry on
//!   or die silently). The capture is not attempted before the copy is
//!   known to be needed, so a failed copy leaves the dotfile untouched.
//! - A failed commit when content changed is `last_error`; exit stays 0
//!   because the relink itself — the job — succeeded. A push failure is a
//!   log line + `last_error` as before, exit 0.
//! - The recorded outcome (`~/.local/state/watchers/karabiner-relink.json`,
//!   interval 60) is written on EVERY run including the silent symlink case:
//!   the minute tick is the liveness signal; Check 9 sees a stopped agent.
//! - The tool's own log `~/Library/Logs/karabiner-relink.log` is capped at
//!   5 MB by `logkeep` (one predecessor `.log.1`). The plist points
//!   StandardOutPath/StandardErrorPath at the same file — that handle is the
//!   supervisor's; the tool only writes through it on unexpected errors.
//!
//! Exit status: 0 = symlink in place (or nothing to do); 1 = the deployed
//! file could not be captured or relinked.

mod exec;
mod outcome;

use clap::Parser;
use exec::Exec;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "karabiner-relink", version, about = "Restore the Dotter symlink for karabiner.json after Karabiner-Elements replaces it, capturing GUI-side edits")]
struct Cli {}

const NAME: &str = "karabiner-relink";
const INTERVAL_SECS: u64 = 60;
const PUSH_TIMEOUT: Duration = Duration::from_secs(30);
const DOTFILE_REL: &str = "karabiner/karabiner.json";

pub struct Paths {
    pub home: PathBuf,
}

impl Paths {
    fn deployed(&self) -> PathBuf {
        self.home.join(".config/karabiner/karabiner.json")
    }
    fn dotfiles(&self) -> PathBuf {
        self.home.join("dotfiles")
    }
    fn dotfile(&self) -> PathBuf {
        self.dotfiles().join(DOTFILE_REL)
    }
    fn log(&self) -> PathBuf {
        self.home.join("Library/Logs/karabiner-relink.log")
    }
}

fn append_log(path: &Path, line: &str) {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {line}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Already a symlink.
    AlreadyLinked,
    /// Nothing at the deployed path.
    Missing,
    /// Relinked; `captured` says whether the dotfile was updated from GUI-side content.
    Relinked { captured: bool },
}

/// Steps 1–3 without git: the file-system half. Errors are the hard failures.
pub fn relink(paths: &Paths, log: &Path) -> Result<Step, String> {
    let deployed = paths.deployed();
    let dotfile = paths.dotfile();
    match std::fs::symlink_metadata(&deployed) {
        Ok(m) if m.file_type().is_symlink() => return Ok(Step::AlreadyLinked),
        Ok(m) if m.is_file() => {}
        Ok(_) | Err(_) => {
            append_log(log, "noop: deployed file missing");
            return Ok(Step::Missing);
        }
    }
    let gui = std::fs::read(&deployed).map_err(|e| format!("read {}: {e}", deployed.display()))?;
    let current = std::fs::read(&dotfile).unwrap_or_default();
    let captured = gui != current;
    if captured {
        append_log(log, "capture: deployed file differs from dotfile, syncing");
        std::fs::write(&dotfile, &gui).map_err(|e| format!("copy into {}: {e}", dotfile.display()))?;
    }
    append_log(log, &format!("relink: replacing real file with symlink → {}", dotfile.display()));
    std::fs::remove_file(&deployed).map_err(|e| format!("remove {}: {e}", deployed.display()))?;
    std::os::unix::fs::symlink(&dotfile, &deployed).map_err(|e| format!("symlink {} → {}: {e}", deployed.display(), dotfile.display()))?;
    Ok(Step::Relinked { captured })
}

/// Auto-commit and bounded push of the captured dotfile. Returns the soft
/// error (commit or push failure), if any.
pub fn commit_and_push(exec: &dyn Exec, paths: &Paths, log: &Path) -> Option<String> {
    let repo = paths.dotfiles();
    let add = exec.run_in(&repo, "git", &["add", DOTFILE_REL]);
    if !add.ok() {
        append_log(log, &format!("git add failed: {}", add.stderr.trim()));
    }
    let commit = exec.run_in(&repo, "git", &["commit", "-m", "karabiner: auto-sync GUI-side changes via karabiner-relink"]);
    if !commit.ok() {
        append_log(log, "commit: nothing to commit");
        let reason = commit.stderr.trim().lines().next().or_else(|| commit.stdout.trim().lines().next()).unwrap_or("");
        return Some(format!("commit failed: {reason}"));
    }
    append_log(log, "commit: pushed to local repo");
    let push = exec.run_in_bounded(&repo, "git", &["push"], PUSH_TIMEOUT);
    if push.ok() {
        None
    } else {
        append_log(log, "push: timed out or failed (network?)");
        Some(format!("push failed (exit {}): {}", push.exit_code, push.stderr.trim().lines().last().unwrap_or("")))
    }
}

/// The whole run. Returns the exit code.
pub fn run(exec: &dyn Exec, paths: &Paths) -> i32 {
    let started = outcome::now_rfc3339();
    let log = paths.log();
    if let Err(e) = logkeep::cap(&log, 5 * logkeep::MB) {
        eprintln!("{NAME}: could not cap {}: {e}", log.display());
    }
    let (code, actions, last_action, last_error) = match relink(paths, &log) {
        Ok(Step::AlreadyLinked) => (0, 0, Some("noop: symlink".to_string()), None),
        Ok(Step::Missing) => (0, 0, Some("noop: deployed file missing".to_string()), None),
        Ok(Step::Relinked { captured }) => {
            let soft = if captured { commit_and_push(exec, paths, &log) } else { None };
            append_log(&log, "done");
            let action = if captured { "captured GUI edits and relinked" } else { "relinked" };
            (0, 1, Some(action.to_string()), soft)
        }
        Err(e) => {
            append_log(&log, &format!("ERROR: {e}"));
            (1, 0, None, Some(e))
        }
    };
    let o = outcome::Outcome { name: NAME, interval_secs: INTERVAL_SECS, started_at: started, last_action, actions, last_error };
    if let Err(e) = outcome::record(&outcome::state_dir(&paths.home), &o) {
        eprintln!("{NAME}: heartbeat write failed: {e}");
    }
    code
}

fn main() {
    let _ = Cli::parse();
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/Users/williamnapier"));
    std::process::exit(run(&exec::Real, &Paths { home }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use exec::{CmdResult, Fake};

    const COMMIT_MSG: &str = "karabiner: auto-sync GUI-side changes via karabiner-relink";

    /// Home with a dotfile of known content and an empty ~/.config/karabiner.
    fn fixture() -> (tempfile::TempDir, Paths) {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().to_path_buf();
        std::fs::create_dir_all(home.join("dotfiles/karabiner")).unwrap();
        std::fs::write(home.join("dotfiles/karabiner/karabiner.json"), "{\"profiles\":[]}\n").unwrap();
        std::fs::create_dir_all(home.join(".config/karabiner")).unwrap();
        (d, Paths { home })
    }

    fn git_ok() -> Fake {
        let mut f = Fake::default();
        f.respond("git", &["add", DOTFILE_REL], CmdResult::success(""));
        f.respond("git", &["commit", "-m", COMMIT_MSG], CmdResult::success("[main abc] karabiner\n"));
        f.respond("git", &["push"], CmdResult::success(""));
        f
    }

    fn log_text(p: &Paths) -> String {
        std::fs::read_to_string(p.log()).unwrap_or_default()
    }

    fn heartbeat(p: &Paths) -> String {
        std::fs::read_to_string(outcome::state_dir(&p.home).join("karabiner-relink.json")).unwrap()
    }

    fn assert_linked(p: &Paths) {
        assert_eq!(std::fs::read_link(p.deployed()).unwrap(), p.dotfile(), "deployed path is a symlink to the dotfile");
    }

    #[test]
    fn already_a_symlink_is_a_silent_noop_with_a_heartbeat() {
        let (_d, p) = fixture();
        std::os::unix::fs::symlink(p.dotfile(), p.deployed()).unwrap();
        let f = Fake::default();
        assert_eq!(run(&f, &p), 0);
        assert!(!p.log().exists(), "no log line for the symlink case");
        assert!(f.calls.borrow().is_empty(), "no git");
        let hb = heartbeat(&p);
        assert!(hb.contains("\"last_action\":\"noop: symlink\"") && hb.contains("\"interval_secs\":60") && hb.contains("\"last_error\":null"), "{hb}");
        assert_linked(&p);
    }

    #[test]
    fn missing_deployed_file_logs_noop() {
        let (_d, p) = fixture();
        let f = Fake::default();
        assert_eq!(run(&f, &p), 0);
        assert!(log_text(&p).contains("noop: deployed file missing"));
        assert!(f.calls.borrow().is_empty());
    }

    #[test]
    fn identical_regular_file_is_relinked_without_git_green_control() {
        let (_d, p) = fixture();
        std::fs::write(p.deployed(), "{\"profiles\":[]}\n").unwrap();
        let f = git_ok();
        assert_eq!(run(&f, &p), 0);
        let log = log_text(&p);
        assert!(!log.contains("capture:"), "{log}");
        assert!(log.contains("relink: replacing real file with symlink → ") && log.contains("done"), "{log}");
        assert!(f.calls.borrow().is_empty(), "identical content → no commit: {:?}", f.calls.borrow());
        assert_linked(&p);
        let hb = heartbeat(&p);
        assert!(hb.contains("\"last_error\":null") && hb.contains("\"actions\":1"), "{hb}");
    }

    #[test]
    fn differing_file_is_captured_committed_and_pushed() {
        let (_d, p) = fixture();
        std::fs::write(p.deployed(), "{\"profiles\":[{\"name\":\"gui edit\"}]}\n").unwrap();
        let f = git_ok();
        assert_eq!(run(&f, &p), 0);
        assert_eq!(std::fs::read_to_string(p.dotfile()).unwrap(), "{\"profiles\":[{\"name\":\"gui edit\"}]}\n", "GUI content captured");
        assert_linked(&p);
        let log = log_text(&p);
        for phrase in ["capture: deployed file differs from dotfile, syncing", "relink: replacing real file with symlink →", "commit: pushed to local repo", "done"] {
            assert!(log.contains(phrase), "missing {phrase:?} in {log}");
        }
        let calls = f.calls.borrow();
        let repo = p.dotfiles().display().to_string();
        assert_eq!(
            *calls,
            vec![
                format!("[in {repo}] git add {DOTFILE_REL}"),
                format!("[in {repo}] git commit -m {COMMIT_MSG}"),
                format!("[in {repo} ≤30s] git push"),
            ]
        );
        assert!(heartbeat(&p).contains("\"last_error\":null"));
    }

    #[test]
    fn commit_failure_is_recorded_but_exit_stays_zero() {
        let (_d, p) = fixture();
        std::fs::write(p.deployed(), "changed\n").unwrap();
        let mut f = Fake::default();
        f.respond("git", &["add", DOTFILE_REL], CmdResult::success(""));
        f.respond("git", &["commit", "-m", COMMIT_MSG], CmdResult::failure(1, "fatal: not a git repository"));
        assert_eq!(run(&f, &p), 0, "the relink itself succeeded");
        assert_linked(&p);
        let log = log_text(&p);
        assert!(log.contains("commit: nothing to commit") && log.contains("done"), "{log}");
        assert!(!f.calls.borrow().iter().any(|c| c.ends_with("git push")), "no push after a failed commit");
        let hb = heartbeat(&p);
        assert!(hb.contains("\"last_error\":\"commit failed: fatal: not a git repository\""), "{hb}");
    }

    #[test]
    fn push_timeout_is_a_log_line_and_last_error() {
        let (_d, p) = fixture();
        std::fs::write(p.deployed(), "changed\n").unwrap();
        let mut f = git_ok();
        f.respond("git", &["push"], CmdResult::failure(124, "killed after 30s"));
        assert_eq!(run(&f, &p), 0);
        assert!(log_text(&p).contains("push: timed out or failed (network?)"));
        assert!(heartbeat(&p).contains("\"last_error\":\"push failed (exit 124): killed after 30s\""));
    }

    #[test]
    fn unrelinkable_file_is_exit_one_red_control() {
        let (_d, p) = fixture();
        // The dotfile's directory is gone: the capture copy cannot land.
        std::fs::write(p.deployed(), "changed\n").unwrap();
        std::fs::remove_dir_all(p.home.join("dotfiles")).unwrap();
        let f = git_ok();
        assert_eq!(run(&f, &p), 1);
        assert!(std::fs::symlink_metadata(p.deployed()).unwrap().is_file(), "deployed file left in place, not lost");
        assert!(log_text(&p).contains("ERROR: copy into "));
        assert!(f.calls.borrow().is_empty());
        let hb = heartbeat(&p);
        assert!(hb.contains("\"last_error\":\"copy into ") && hb.contains("\"actions\":0"), "{hb}");
    }

    #[test]
    fn real_bounded_run_kills_at_the_deadline() {
        let d = tempfile::tempdir().unwrap();
        let r = exec::Real.run_in_bounded(d.path(), "sleep", &["5"], Duration::from_millis(300));
        assert_eq!(r.exit_code, 124, "{r:?}");
        let ok = exec::Real.run_in_bounded(d.path(), "true", &[], Duration::from_secs(5));
        assert!(ok.ok());
    }
}
