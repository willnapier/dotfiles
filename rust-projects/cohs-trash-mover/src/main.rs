//! cohs-trash-mover — reconcile notmuch trash tags in the COHS-only index with
//! its Maildir folders. Rust port (2026-09-23) of the bash script of the same
//! name; the launchd agent `com.williamnapier.cohs-trash-mover` (every 5 min)
//! and the `cohs-trash-mover.timer` on nimbini run `~/.local/bin/cohs-trash-mover`
//! unchanged.
//!
//! Background: meli's `D` applies notmuch tag operations regardless of backend.
//! For Gmail that is enough (gmail-push-tags propagates tags as labels); for
//! COHS mail synced by mbsync from M365 IMAP a tag is invisible to the server
//! because mbsync mirrors Maildir FOLDERS. So: every message tagged
//! `cohs and trash` whose file is still in `~/Mail/cohs/INBOX/cur/` is moved to
//! `~/Mail/cohs/Deleted Items/cur/`; mbsync's next tick replicates the move and
//! Outlook shows it in Deleted Items. Idempotent; `notmuch new` afterwards so
//! the index sees the new paths. `NOTMUCH_CONFIG` is forced to the COHS-only
//! config, as the script did (the Mac plist hands in the canonical config; the
//! query must run against the COHS index).
//!
//! What changed versus the script:
//! - A failing `notmuch search` (non-zero exit, or notmuch not on PATH) is a
//!   failure — exit 1, `last_error` recorded, an alert through `notify-user`.
//!   The script's `|| true` turned it into "nothing to move" forever.
//! - A file that could not be moved, or a failing `notmuch new`, is reported
//!   per item and makes the exit non-zero; the script's `set -e` would have
//!   aborted mid-loop with no record.
//! - Every run writes `~/.local/state/watchers/cohs-trash-mover.json`
//!   (interval_secs 300), so a timer that stops firing shows up in
//!   system-health-check Check 9 instead of in nobody's log.
//! - The stderr line `moved N cohs message(s) to Deleted Items` is unchanged.
//!
//! Exit status: 0 = ran, every candidate handled; 1 = notmuch failed, a move
//! failed, or the Deleted Items directory could not be created.

mod exec;
mod outcome;

use clap::Parser;
use exec::Exec;
use std::path::{Path, PathBuf};

const NAME: &str = "cohs-trash-mover";
/// The launchd StartInterval and the systemd OnCalendar=*:0/5 cadence.
const INTERVAL_SECS: u64 = 300;
const QUERY: &str = "tag:cohs and tag:trash and folder:\"INBOX\"";

#[derive(Parser)]
#[command(name = NAME, version, about = "Reconcile trash tags in the COHS-only index with its Maildir folders.")]
struct Cli {
    /// List what would be moved; move nothing, do not reindex.
    #[arg(long)]
    dry_run: bool,
}

struct Paths {
    inbox_cur: PathBuf,
    deleted_cur: PathBuf,
}

fn paths(home: &Path) -> Paths {
    Paths { inbox_cur: home.join("Mail/cohs/INBOX/cur"), deleted_cur: home.join("Mail/cohs/Deleted Items/cur") }
}

/// The result of one run, before any side channel (log line, alert, heartbeat).
#[derive(Debug, Default, PartialEq, Eq)]
struct Run {
    moved: u64,
    /// Candidates the search named that no longer existed on disk.
    skipped: u64,
    /// Per-file move failures, each with the path and the OS error.
    failures: Vec<String>,
    /// A failure that stopped the run doing its job at all.
    error: Option<String>,
}

impl Run {
    fn ok(&self) -> bool {
        self.error.is_none() && self.failures.is_empty()
    }
    fn exit_code(&self) -> i32 {
        if self.ok() {
            0
        } else {
            1
        }
    }
    fn problem(&self) -> Option<String> {
        if let Some(e) = &self.error {
            return Some(e.clone());
        }
        if !self.failures.is_empty() {
            return Some(format!("{} move failure(s): {}", self.failures.len(), self.failures.join("; ")));
        }
        None
    }
}

/// Which of the search's file paths are ours to move: only files under
/// INBOX/cur. Anything already under Deleted Items, or outside the COHS
/// tree, stays where it is (the query should exclude them; belt and braces).
fn candidates(search_output: &str, p: &Paths) -> Vec<PathBuf> {
    search_output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .filter(|f| f.starts_with(&p.inbox_cur))
        .filter(|f| !f.components().any(|c| c.as_os_str() == "Deleted Items"))
        .collect()
}

/// Rename into Deleted Items; if the rename fails (a different volume, say)
/// fall back to copy + remove so the message is never left in both places.
fn move_one(src: &Path, dst_dir: &Path) -> std::io::Result<()> {
    let Some(base) = src.file_name() else {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no file name"));
    };
    let dst = dst_dir.join(base);
    if std::fs::rename(src, &dst).is_ok() {
        return Ok(());
    }
    std::fs::copy(src, &dst)?;
    std::fs::remove_file(src)
}

fn run(exec: &dyn Exec, p: &Paths, dry_run: bool) -> Run {
    let mut run = Run::default();
    let search = exec.run("notmuch", &["search", "--output=files", QUERY]);
    if !search.ok() {
        run.error = Some(format!("notmuch search exited {}: {}", search.exit_code, search.stderr.trim()));
        return run;
    }
    let files = candidates(&search.stdout, p);
    if files.is_empty() {
        return run;
    }
    if !dry_run {
        if let Err(e) = std::fs::create_dir_all(&p.deleted_cur) {
            run.error = Some(format!("cannot create {}: {e}", p.deleted_cur.display()));
            return run;
        }
    }
    for f in &files {
        if !f.is_file() {
            run.skipped += 1;
            continue;
        }
        if dry_run {
            println!("would move {}", f.display());
            run.moved += 1;
            continue;
        }
        match move_one(f, &p.deleted_cur) {
            Ok(()) => run.moved += 1,
            Err(e) => run.failures.push(format!("{}: {e}", f.display())),
        }
    }
    if run.moved > 0 && !dry_run {
        // Reindex so notmuch sees the new file paths. Quiet on success.
        let new = exec.run("notmuch", &["new"]);
        if !new.ok() {
            run.error = Some(format!("notmuch new exited {} after moving {} file(s): {}", new.exit_code, run.moved, new.stderr.trim()));
        }
    }
    run
}

fn notify(exec: &dyn Exec, body: &str) {
    // notify-user records whether the alert arrived (D2-23); its exit is not ours.
    let _ = exec.run("notify-user", &["--tool", NAME, "cohs-trash-mover failed", body]);
}

/// Side channels after a run: the stderr log line the script wrote, per-file
/// failures, the alert, the recorded outcome. Returns the exit code.
fn finish(exec: &dyn Exec, state_dir: &Path, started_at: String, run: &Run, dry_run: bool) -> i32 {
    let stamp = || chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    if run.moved > 0 {
        let verb = if dry_run { "would move" } else { "moved" };
        eprintln!("{} {verb} {} cohs message(s) to Deleted Items", stamp(), run.moved);
    }
    for f in &run.failures {
        eprintln!("{} move failed: {f}", stamp());
    }
    if let Some(e) = &run.error {
        eprintln!("{} {NAME}: {e}", stamp());
    }
    let problem = run.problem();
    if let Some(p) = &problem {
        notify(exec, p);
    }
    let o = outcome::Outcome {
        name: NAME,
        interval_secs: INTERVAL_SECS,
        started_at,
        last_action: if run.moved > 0 { Some(format!("moved {} to Deleted Items", run.moved)) } else { None },
        actions: run.moved,
        last_error: problem,
    };
    if !dry_run {
        if let Err(e) = outcome::record(state_dir, &o) {
            eprintln!("{} {NAME}: could not record outcome: {e}", stamp());
        }
    }
    run.exit_code()
}

fn main() {
    let cli = Cli::parse();
    let started_at = outcome::now_rfc3339();
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));
    // The query must run against the COHS-only index whatever the supervisor
    // handed in (the Mac plist passes the canonical config).
    std::env::set_var("NOTMUCH_CONFIG", home.join("Mail/.notmuch-cohs-config"));
    let p = paths(&home);
    let r = run(&exec::Real, &p, cli.dry_run);
    std::process::exit(finish(&exec::Real, &outcome::state_dir(&home), started_at, &r, cli.dry_run));
}

#[cfg(test)]
mod tests {
    use super::*;
    use exec::{CmdResult, Fake};

    const SEARCH_ARGS: &[&str] = &["search", "--output=files", QUERY];

    /// A synthetic COHS Maildir under a temp home: two INBOX files, one
    /// already in Deleted Items. Nothing here is mail — one byte each.
    fn fixture() -> (tempfile::TempDir, Paths) {
        let d = tempfile::tempdir().unwrap();
        let p = paths(d.path());
        std::fs::create_dir_all(&p.inbox_cur).unwrap();
        std::fs::create_dir_all(&p.deleted_cur).unwrap();
        std::fs::write(p.inbox_cur.join("a:2,S"), b"x").unwrap();
        std::fs::write(p.inbox_cur.join("b:2,S"), b"x").unwrap();
        std::fs::write(p.deleted_cur.join("z:2,ST"), b"x").unwrap();
        (d, p)
    }

    fn search_listing(p: &Paths, outside: &Path) -> String {
        format!(
            "{}\n{}\n{}\n{}\n",
            p.inbox_cur.join("a:2,S").display(),
            p.deleted_cur.join("z:2,ST").display(),
            outside.display(),
            p.inbox_cur.join("gone:2,S").display(),
        )
    }

    #[test]
    fn candidates_are_only_inbox_cur_files() {
        let (d, p) = fixture();
        let outside = d.path().join("Mail/gmail-rs/INBOX/cur/o:2,S");
        let c = candidates(&search_listing(&p, &outside), &p);
        assert_eq!(c, vec![p.inbox_cur.join("a:2,S"), p.inbox_cur.join("gone:2,S")]);
        assert!(candidates("", &p).is_empty());
        assert!(candidates("\n  \n", &p).is_empty());
    }

    #[test]
    fn moves_inbox_files_skips_missing_and_reindexes() {
        let (d, p) = fixture();
        let outside = d.path().join("Mail/gmail-rs/INBOX/cur/o:2,S");
        let mut f = Fake::default();
        f.respond("notmuch", SEARCH_ARGS, CmdResult::success(&search_listing(&p, &outside)));
        f.respond("notmuch", &["new"], CmdResult::success(""));
        let r = run(&f, &p, false);
        assert_eq!(r, Run { moved: 1, skipped: 1, failures: vec![], error: None });
        assert!(!p.inbox_cur.join("a:2,S").exists(), "moved out of INBOX");
        assert!(p.deleted_cur.join("a:2,S").exists(), "arrived in Deleted Items");
        assert!(p.inbox_cur.join("b:2,S").exists(), "untagged file untouched");
        assert!(p.deleted_cur.join("z:2,ST").exists(), "already-deleted file untouched");
        assert_eq!(f.calls.borrow().last().unwrap(), "notmuch new");
        assert_eq!(r.exit_code(), 0);
    }

    #[test]
    fn dry_run_moves_nothing_and_does_not_reindex() {
        let (d, p) = fixture();
        let outside = d.path().join("elsewhere");
        let mut f = Fake::default();
        f.respond("notmuch", SEARCH_ARGS, CmdResult::success(&search_listing(&p, &outside)));
        let r = run(&f, &p, true);
        assert_eq!(r.moved, 1);
        assert!(p.inbox_cur.join("a:2,S").exists());
        assert_eq!(f.calls.borrow().len(), 1, "no notmuch new: {:?}", f.calls.borrow());
    }

    #[test]
    fn green_control_nothing_to_move_is_a_clean_exit_zero() {
        let (d, p) = fixture();
        let mut f = Fake::default();
        f.respond("notmuch", SEARCH_ARGS, CmdResult::success(""));
        let r = run(&f, &p, false);
        assert_eq!(r, Run::default());
        let state = d.path().join("state");
        let code = finish(&f, &state, outcome::now_rfc3339(), &r, false);
        assert_eq!(code, 0);
        assert!(!f.calls.borrow().iter().any(|c| c.starts_with("notify-user")), "{:?}", f.calls.borrow());
        let hb: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(state.join("cohs-trash-mover.json")).unwrap()).unwrap();
        assert_eq!(hb["watcher"], "cohs-trash-mover");
        assert_eq!(hb["interval_secs"], 300);
        assert!(hb["last_error"].is_null(), "{hb}");
        assert_eq!(hb["actions"], 0);
    }

    #[test]
    fn red_control_failed_search_exits_one_alerts_and_records_the_error() {
        let (d, p) = fixture();
        let mut f = Fake::default();
        f.respond("notmuch", SEARCH_ARGS, CmdResult::failure(1, "notmuch: database locked"));
        let r = run(&f, &p, false);
        assert_eq!(r.error.as_deref(), Some("notmuch search exited 1: notmuch: database locked"));
        assert!(p.inbox_cur.join("a:2,S").exists(), "nothing moved on a failed search");
        let state = d.path().join("state");
        let code = finish(&f, &state, outcome::now_rfc3339(), &r, false);
        assert_eq!(code, 1);
        let calls = f.calls.borrow();
        assert!(calls.iter().any(|c| c.starts_with("notify-user --tool cohs-trash-mover ")), "{calls:?}");
        let hb: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(state.join("cohs-trash-mover.json")).unwrap()).unwrap();
        assert_eq!(hb["last_error"], "notmuch search exited 1: notmuch: database locked");
    }

    #[test]
    fn notmuch_missing_from_path_is_the_same_red() {
        let (_d, p) = fixture();
        let f = Fake::default(); // unscripted → 127
        let r = run(&f, &p, false);
        assert!(r.error.as_deref().unwrap().starts_with("notmuch search exited 127"), "{r:?}");
        assert_eq!(r.exit_code(), 1);
    }

    #[test]
    fn failed_reindex_after_a_move_is_reported() {
        let (d, p) = fixture();
        let outside = d.path().join("elsewhere");
        let mut f = Fake::default();
        f.respond("notmuch", SEARCH_ARGS, CmdResult::success(&search_listing(&p, &outside)));
        f.respond("notmuch", &["new"], CmdResult::failure(1, "Xapian lock"));
        let r = run(&f, &p, false);
        assert_eq!(r.moved, 1, "the move itself happened");
        assert_eq!(r.error.as_deref(), Some("notmuch new exited 1 after moving 1 file(s): Xapian lock"));
        assert_eq!(r.exit_code(), 1);
    }

    #[test]
    fn a_move_failure_is_recorded_per_file_and_fails_the_run() {
        let (d, p) = fixture();
        // Make Deleted Items unwritable-into by putting a directory where the
        // destination file would go: rename and copy both fail.
        std::fs::create_dir(p.deleted_cur.join("a:2,S")).unwrap();
        let outside = d.path().join("elsewhere");
        let mut f = Fake::default();
        f.respond("notmuch", SEARCH_ARGS, CmdResult::success(&search_listing(&p, &outside)));
        let r = run(&f, &p, false);
        assert_eq!(r.moved, 0);
        assert_eq!(r.failures.len(), 1, "{r:?}");
        assert!(r.failures[0].contains("a:2,S"));
        assert!(p.inbox_cur.join("a:2,S").exists(), "source still there");
        assert_eq!(r.exit_code(), 1);
        assert!(r.problem().unwrap().starts_with("1 move failure(s): "));
        assert_eq!(f.calls.borrow().len(), 1, "no reindex when nothing moved");
    }
}
