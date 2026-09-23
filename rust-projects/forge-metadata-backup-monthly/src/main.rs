//! forge-metadata-backup-monthly — run the Forge metadata export and commit it.
//! Rust port (2026-09-23) of the Nushell script of the same name; the launchd
//! agent `com.user.forge-metadata-backup` (1st of the month, 02:00) runs
//! `~/.local/bin/forge-metadata-backup-monthly` unchanged, and the printed
//! lines keep the script's phrasing so the supervisor log reads the same.
//!
//! Flow: banner → Forge dir must exist → `forge-metadata-backup export <forge>`
//! → if Forge is not a git repo, warn and stop (exit 0) → if
//! `.metadata-backup.csv` is unchanged, say so and stop (exit 0) → otherwise
//! `git add` + `git commit -m "Update metadata backup - YYYY-MM-DD"`.
//!
//! What changed versus the script:
//! - Every ERROR path (missing Forge, export failure, commit failure) now raises
//!   `notify-user --tool forge-metadata-backup-monthly` and is recorded as
//!   `last_error` in `~/.local/state/watchers/forge-metadata-backup-monthly.json`.
//!   The job runs once a month; the script's `print` + `exit 1` into a log
//!   nobody reads could stay unnoticed until the next month.
//! - A recorded outcome is written on every path (interval 31 days), so
//!   system-health-check Check 9 sees a month with no run at all.
//! - `git` and the export tool are invoked directly (no `try/catch` around a
//!   `^cmd` whose non-zero exit Nushell reported as a caught error).
//!
//! Exit status: 0 = exported, and committed if there was anything to commit
//! (or Forge is not a repo); 1 = Forge missing, export failed, or commit failed.

mod exec;
mod outcome;

use clap::Parser;
use exec::Exec;
use std::path::{Path, PathBuf};

const NAME: &str = "forge-metadata-backup-monthly";
/// Monthly job: 31 days, so Check 9's max(3×interval, 15 min) allows a missed
/// month to show up after roughly a quarter — the coarsest honest window.
const INTERVAL_SECS: u64 = 31 * 86_400;
const CSV: &str = ".metadata-backup.csv";

#[derive(Parser)]
#[command(name = NAME, version, about = "Export Forge file metadata and commit it to the Forge repo (monthly launchd job)")]
struct Cli {
    /// Forge directory (default ~/Forge)
    #[arg(long)]
    forge: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// Exported and committed.
    Committed(String),
    /// Exported; nothing to commit, or Forge is not a repository.
    NothingToCommit(String),
    Failed(String),
}

impl Verdict {
    fn exit_code(&self) -> i32 {
        if matches!(self, Verdict::Failed(_)) {
            1
        } else {
            0
        }
    }
}

/// The whole run as the script did it, with every decision printed.
fn run(exec: &dyn Exec, forge: &Path, today: &str) -> Verdict {
    println!("=== Forge Metadata Backup - {} ===", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    println!("Forge directory: {}\n", forge.display());

    if !forge.is_dir() {
        let msg = format!("Forge directory not found at {}", forge.display());
        println!("ERROR: {msg}");
        return Verdict::Failed(msg);
    }

    println!("Running metadata export...");
    let forge_s = forge.to_string_lossy();
    let r = exec.run("forge-metadata-backup", &["export", &forge_s]);
    print!("{}", r.stdout);
    if !r.ok() {
        let msg = format!("Failed to export metadata (exit {}): {}", r.exit_code, r.stderr.trim());
        println!("ERROR: {msg}");
        return Verdict::Failed(msg);
    }

    println!("\nCommitting to git...");
    if !exec.run_in(forge, "git", &["rev-parse", "--git-dir"]).ok() {
        println!("WARNING: Forge directory is not a git repository");
        println!("Metadata backup created but not committed");
        return Verdict::NothingToCommit("not a git repository".into());
    }

    let status = exec.run_in(forge, "git", &["status", "--porcelain", CSV]);
    if status.out().is_empty() {
        println!("No changes to commit - metadata backup is up to date");
        return Verdict::NothingToCommit("up to date".into());
    }

    let add = exec.run_in(forge, "git", &["add", CSV]);
    if !add.ok() {
        let msg = format!("Failed to commit (git add exit {}): {}", add.exit_code, add.stderr.trim());
        println!("ERROR: {msg}");
        return Verdict::Failed(msg);
    }
    let commit_msg = format!("Update metadata backup - {today}");
    let commit = exec.run_in(forge, "git", &["commit", "-m", &commit_msg]);
    if !commit.ok() {
        let msg = format!("Failed to commit (git commit exit {}): {}", commit.exit_code, commit.stderr.trim());
        println!("ERROR: {msg}");
        return Verdict::Failed(msg);
    }
    println!("✅ Successfully committed: {commit_msg}");
    println!("\n=== Backup complete ===");
    Verdict::Committed(commit_msg)
}

/// Through `notify-user`, which records whether the banner arrived (D2-23).
fn notify(exec: &dyn Exec, msg: &str) {
    let _ = exec.run("notify-user", &["--tool", NAME, "Forge metadata backup failed", msg]);
}

/// Alert on failure and write the recorded outcome. Returns the exit code.
fn finish(exec: &dyn Exec, state_dir: &Path, started_at: String, v: &Verdict) -> i32 {
    let (last_action, actions, last_error) = match v {
        Verdict::Committed(m) => (Some(m.clone()), 1, None),
        Verdict::NothingToCommit(m) => (Some(m.clone()), 0, None),
        Verdict::Failed(m) => (None, 0, Some(m.clone())),
    };
    if let Some(e) = &last_error {
        notify(exec, e);
    }
    let o = outcome::Outcome { name: NAME, interval_secs: INTERVAL_SECS, started_at, last_action, actions, last_error };
    if let Err(e) = outcome::record(state_dir, &o) {
        eprintln!("{NAME}: could not record outcome in {}: {e}", state_dir.display());
    }
    v.exit_code()
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn main() {
    let cli = Cli::parse();
    let started_at = outcome::now_rfc3339();
    let home = home();
    let forge = cli.forge.unwrap_or_else(|| home.join("Forge"));
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let exec = exec::Real;
    let v = run(&exec, &forge, &today);
    std::process::exit(finish(&exec, &outcome::state_dir(&home), started_at, &v));
}

#[cfg(test)]
mod tests {
    use super::*;
    use exec::{CmdResult, Fake};

    fn forge_fixture() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("Forge")).unwrap();
        d
    }

    fn export_ok(f: &mut Fake, forge: &Path) {
        f.respond("forge-metadata-backup", &["export", &forge.to_string_lossy()], CmdResult::success("Exporting metadata from: x\n"));
    }

    fn heartbeat(d: &Path) -> serde_json::Value {
        let s = std::fs::read_to_string(d.join("state").join(format!("{NAME}.json"))).unwrap();
        serde_json::from_str(&s).unwrap()
    }

    #[test]
    fn green_control_exports_and_commits() {
        let d = forge_fixture();
        let forge = d.path().join("Forge");
        let mut f = Fake::default();
        export_ok(&mut f, &forge);
        f.respond("git", &["rev-parse", "--git-dir"], CmdResult::success(".git\n"))
            .respond("git", &["status", "--porcelain", CSV], CmdResult::success(" M .metadata-backup.csv\n"))
            .respond("git", &["add", CSV], CmdResult::success(""))
            .respond("git", &["commit", "-m", "Update metadata backup - 2026-09-23"], CmdResult::success("[main abc] Update\n"));
        let v = run(&f, &forge, "2026-09-23");
        assert_eq!(v, Verdict::Committed("Update metadata backup - 2026-09-23".into()));
        let code = finish(&f, &d.path().join("state"), "t0".into(), &v);
        assert_eq!(code, 0);
        let calls = f.calls.borrow();
        assert!(!calls.iter().any(|c| c.starts_with("notify-user")), "{calls:?}");
        let in_forge = format!("[{}]", forge.display());
        assert!(calls.iter().filter(|c| c.contains(" git ")).all(|c| c.starts_with(&in_forge)), "git ran in Forge: {calls:?}");
        let hb = heartbeat(d.path());
        assert_eq!(hb["actions"], 1);
        assert!(hb["last_error"].is_null(), "{hb}");
        assert_eq!(hb["interval_secs"], INTERVAL_SECS);
        assert_eq!(hb["watcher"], NAME);
    }

    #[test]
    fn red_control_export_failure_alerts_and_exits_one() {
        let d = forge_fixture();
        let forge = d.path().join("Forge");
        let mut f = Fake::default();
        f.respond("forge-metadata-backup", &["export", &forge.to_string_lossy()], CmdResult::failure(2, "boom"));
        let v = run(&f, &forge, "2026-09-23");
        assert!(matches!(&v, Verdict::Failed(m) if m.contains("Failed to export metadata") && m.contains("boom")), "{v:?}");
        let code = finish(&f, &d.path().join("state"), "t0".into(), &v);
        assert_eq!(code, 1);
        let calls = f.calls.borrow();
        assert!(calls.iter().any(|c| c.starts_with("notify-user --tool forge-metadata-backup-monthly")), "{calls:?}");
        assert!(!calls.iter().any(|c| c.contains(" git ")), "no git after a failed export: {calls:?}");
        let hb = heartbeat(d.path());
        assert!(hb["last_error"].as_str().unwrap().contains("Failed to export metadata"));
        assert_eq!(hb["actions"], 0);
    }

    #[test]
    fn missing_forge_is_a_failure() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake::default();
        let v = run(&f, &d.path().join("nope"), "2026-09-23");
        assert!(matches!(&v, Verdict::Failed(m) if m.starts_with("Forge directory not found")));
        assert_eq!(finish(&f, &d.path().join("state"), "t0".into(), &v), 1);
        assert!(f.calls.borrow().iter().any(|c| c.starts_with("notify-user")));
    }

    #[test]
    fn not_a_repo_warns_and_exits_zero() {
        let d = forge_fixture();
        let forge = d.path().join("Forge");
        let mut f = Fake::default();
        export_ok(&mut f, &forge);
        f.respond("git", &["rev-parse", "--git-dir"], CmdResult::failure(128, "not a git repository"));
        let v = run(&f, &forge, "2026-09-23");
        assert_eq!(v, Verdict::NothingToCommit("not a git repository".into()));
        assert_eq!(finish(&f, &d.path().join("state"), "t0".into(), &v), 0);
        assert!(!f.calls.borrow().iter().any(|c| c.starts_with("notify-user")));
        assert!(heartbeat(d.path())["last_error"].is_null());
    }

    #[test]
    fn up_to_date_makes_no_commit_call() {
        let d = forge_fixture();
        let forge = d.path().join("Forge");
        let mut f = Fake::default();
        export_ok(&mut f, &forge);
        f.respond("git", &["rev-parse", "--git-dir"], CmdResult::success(".git\n"))
            .respond("git", &["status", "--porcelain", CSV], CmdResult::success("\n"));
        let v = run(&f, &forge, "2026-09-23");
        assert_eq!(v, Verdict::NothingToCommit("up to date".into()));
        assert_eq!(finish(&f, &d.path().join("state"), "t0".into(), &v), 0);
        let calls = f.calls.borrow();
        assert!(!calls.iter().any(|c| c.contains("git add") || c.contains("git commit")), "{calls:?}");
        assert_eq!(calls.len(), 3, "{calls:?}");
    }

    #[test]
    fn commit_failure_is_a_failure() {
        let d = forge_fixture();
        let forge = d.path().join("Forge");
        let mut f = Fake::default();
        export_ok(&mut f, &forge);
        f.respond("git", &["rev-parse", "--git-dir"], CmdResult::success(".git\n"))
            .respond("git", &["status", "--porcelain", CSV], CmdResult::success(" M .metadata-backup.csv\n"))
            .respond("git", &["add", CSV], CmdResult::success(""))
            .respond("git", &["commit", "-m", "Update metadata backup - 2026-09-23"], CmdResult::failure(1, "hook rejected"));
        let v = run(&f, &forge, "2026-09-23");
        assert!(matches!(&v, Verdict::Failed(m) if m.contains("git commit exit 1") && m.contains("hook rejected")), "{v:?}");
        assert_eq!(finish(&f, &d.path().join("state"), "t0".into(), &v), 1);
    }
}
