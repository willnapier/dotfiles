//! sync-forge-to-dropbox — one-way rsync of `~/Forge` and `~/Admin` into the
//! Dropbox folder so 1Writer on iOS can read them. Rust port (2026-09-23) of
//! the bash script of the same name; the launchd plist
//! `com.user.forge-dropbox-sync` still runs `~/.local/bin/sync-forge-to-dropbox`
//! every 30 minutes, unchanged. Mac only (the Dropbox path).
//!
//! Each run: cap the log, then two rsync calls with exactly the script's
//! options and exclude lists (`--inplace --no-perms --no-owner --no-group
//! --omit-dir-times -a --delete`, relaxed for the Dropbox file provider), rsync
//! output appended to the log, then the same summary lines the script wrote:
//! `[YYYY-mm-dd HH:MM:SS] Starting Forge+Admin sync to Dropbox...`,
//! `Forge sync OK` / `Forge sync FAILED (exit N)`, likewise for Admin, `Sync complete.`
//! Lines go to stdout and the log, as the script's `tee -a` did.
//!
//! Semantic differences from the script:
//! - Exit status means something. The script had `set -e` but tested each rsync
//!   in an `if`, so every run exited 0 — a Dropbox folder launchd was not allowed
//!   to touch (the 2026-07-27 → 09-03 "Operation not permitted" outage) was
//!   invisible. Now a FAILED rsync → exit 1 and a banner through
//!   `notify-user --tool sync-forge-to-dropbox`; both rsyncs still run.
//! - rsync exit 24 (some source files vanished during transfer — normal for a
//!   tree Syncthing and Helix write into) is treated as OK, with a note in the log.
//! - A missing Dropbox folder (`~/Library/CloudStorage/Dropbox`) is a failure
//!   (exit 1, notified) rather than an rsync into a freshly created stray dir.
//! - Log cap: the script moved the log to `.old` above 1 MB; now logkeep caps at
//!   1 MB with the predecessor at `.1` (audit D2-24).
//! - Recorded outcome: `~/.local/state/watchers/sync-forge-to-dropbox.json`
//!   (interval_secs 1800); `actions` = trees synced OK, `last_error` on any failure.
//!
//! Exit: 0 both trees synced (or vanished-files only); 1 any tree failed or the
//! Dropbox folder is missing.

mod exec;
mod outcome;

use clap::Parser;
use exec::Exec;
use std::path::{Path, PathBuf};

const NAME: &str = "sync-forge-to-dropbox";
const INTERVAL_SECS: u64 = 1800;
const LOG_CAP: u64 = logkeep::MB;
/// rsync: "Partial transfer due to vanished source files" — not a failure here.
const RSYNC_VANISHED: i32 = 24;

const RSYNC_OPTS: &[&str] = &["--inplace", "--no-perms", "--no-owner", "--no-group", "--omit-dir-times", "-a", "--delete"];
const FORGE_EXCLUDES: &[&str] = &[
    ".stversions/",
    ".stignore",
    ".stfolder/",
    ".syncthing-*",
    ".frecency-cache/",
    ".obsidian/",
    ".corruption-quarantine-*/",
    ".html/",
    ".DS_Store",
    "*.tmp",
    "*.temp",
    "*.mp4",
    "*.mov",
];
const ADMIN_EXCLUDES: &[&str] = &[".stversions/", ".stignore", ".stfolder/", ".syncthing-*", ".DS_Store", "*.tmp"];

#[derive(Parser)]
#[command(name = NAME, version, about = "One-way sync of Forge and Admin to Dropbox for 1Writer mobile access")]
struct Cli {}

pub struct Paths {
    home: PathBuf,
    dropbox: PathBuf,
    log: PathBuf,
    state_dir: PathBuf,
}

impl Paths {
    fn under(home: &Path) -> Paths {
        Paths {
            home: home.to_path_buf(),
            dropbox: home.join("Library/CloudStorage/Dropbox"),
            log: home.join(".local/logs/forge-dropbox-sync.log"),
            state_dir: outcome::state_dir(home),
        }
    }
}

/// The argv for one tree, exactly as the script built it (options, then
/// excludes in order, then `src/ dst/`).
pub fn rsync_args(src: &Path, dst: &Path, excludes: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = RSYNC_OPTS.iter().map(|s| s.to_string()).collect();
    for e in excludes {
        v.push(format!("--exclude={e}"));
    }
    v.push(format!("{}/", src.display()));
    v.push(format!("{}/", dst.display()));
    v
}

fn stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// `log()` as the script had it: `[stamp] line` to stdout and the log.
fn log(paths: &Paths, line: &str) {
    let full = format!("[{}] {line}", stamp());
    println!("{full}");
    append_raw(paths, &format!("{full}\n"));
}

fn append_raw(paths: &Paths, text: &str) {
    use std::io::Write;
    if let Some(d) = paths.log.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&paths.log) {
        let _ = f.write_all(text.as_bytes());
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum TreeResult {
    Ok,
    /// Exit 24: vanished source files, still counted as synced.
    OkVanished,
    Failed(i32),
}

pub fn classify(exit_code: i32) -> TreeResult {
    match exit_code {
        0 => TreeResult::Ok,
        RSYNC_VANISHED => TreeResult::OkVanished,
        n => TreeResult::Failed(n),
    }
}

fn sync_tree(exec: &dyn Exec, paths: &Paths, label: &str, src: &Path, dst: &Path, excludes: &[&str]) -> TreeResult {
    let args = rsync_args(src, dst, excludes);
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let r = exec.run("rsync", &argv);
    // The script appended rsync's stdout+stderr to the log.
    append_raw(paths, &r.stdout);
    append_raw(paths, &r.stderr);
    let res = classify(r.exit_code);
    match &res {
        TreeResult::Ok => log(paths, &format!("{label} sync OK")),
        TreeResult::OkVanished => log(paths, &format!("{label} sync OK (rsync 24: some source files vanished during transfer)")),
        TreeResult::Failed(n) => log(paths, &format!("{label} sync FAILED (exit {n})")),
    }
    res
}

fn notify(exec: &dyn Exec, msg: &str) {
    let _ = exec.run("notify-user", &["--tool", NAME, "Forge→Dropbox sync failed", msg]);
}

/// One run. Returns the process exit code.
pub fn run(exec: &dyn Exec, paths: &Paths) -> i32 {
    let started_at = outcome::now_rfc3339();
    match logkeep::cap(&paths.log, LOG_CAP) {
        Ok(o) if o.rolled() => log(paths, &format!("log capped at 1 MB; predecessor {}", logkeep::predecessor(&paths.log).display())),
        Ok(_) => {}
        Err(e) => log(paths, &format!("could not cap log: {e}")),
    }

    log(paths, "Starting Forge+Admin sync to Dropbox...");

    if !paths.dropbox.is_dir() {
        let msg = format!("Dropbox folder missing: {}", paths.dropbox.display());
        log(paths, &format!("FAILED: {msg}"));
        notify(exec, &msg);
        record(paths, started_at, None, 0, Some(msg));
        return 1;
    }

    let forge = sync_tree(exec, paths, "Forge", &paths.home.join("Forge"), &paths.dropbox.join("Forge"), FORGE_EXCLUDES);
    let admin = sync_tree(exec, paths, "Admin", &paths.home.join("Admin"), &paths.dropbox.join("Admin"), ADMIN_EXCLUDES);

    log(paths, "Sync complete.");

    let mut failures = vec![];
    if let TreeResult::Failed(n) = forge {
        failures.push(format!("Forge rsync exit {n}"));
    }
    if let TreeResult::Failed(n) = admin {
        failures.push(format!("Admin rsync exit {n}"));
    }
    let synced = [&forge, &admin].iter().filter(|r| !matches!(r, TreeResult::Failed(_))).count() as u64;

    if failures.is_empty() {
        record(paths, started_at, Some("Forge and Admin synced".into()), synced, None);
        0
    } else {
        let msg = failures.join("; ");
        notify(exec, &msg);
        record(paths, started_at, None, synced, Some(msg));
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
    std::process::exit(run(&exec::Real, &Paths::under(&home)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use exec::{CmdResult, Fake};

    /// Home with Forge, Admin and the Dropbox folder present.
    fn fixture() -> (tempfile::TempDir, Paths) {
        let d = tempfile::tempdir().unwrap();
        for p in ["Forge", "Admin", "Library/CloudStorage/Dropbox"] {
            std::fs::create_dir_all(d.path().join(p)).unwrap();
        }
        let paths = Paths::under(d.path());
        (d, paths)
    }

    fn forge_argv(paths: &Paths) -> Vec<String> {
        rsync_args(&paths.home.join("Forge"), &paths.dropbox.join("Forge"), FORGE_EXCLUDES)
    }
    fn admin_argv(paths: &Paths) -> Vec<String> {
        rsync_args(&paths.home.join("Admin"), &paths.dropbox.join("Admin"), ADMIN_EXCLUDES)
    }
    fn respond(fake: &mut Fake, argv: &[String], r: CmdResult) {
        let a: Vec<&str> = argv.iter().map(String::as_str).collect();
        fake.respond("rsync", &a, r);
    }
    fn heartbeat_json(paths: &Paths) -> serde_json::Value {
        let hb = std::fs::read_to_string(paths.state_dir.join("sync-forge-to-dropbox.json")).unwrap();
        serde_json::from_str(&hb).unwrap()
    }

    #[test]
    fn forge_argv_matches_the_script_exactly() {
        let (_d, paths) = fixture();
        let argv = forge_argv(&paths);
        let home = paths.home.display().to_string();
        let expected: Vec<String> = [
            "--inplace",
            "--no-perms",
            "--no-owner",
            "--no-group",
            "--omit-dir-times",
            "-a",
            "--delete",
            "--exclude=.stversions/",
            "--exclude=.stignore",
            "--exclude=.stfolder/",
            "--exclude=.syncthing-*",
            "--exclude=.frecency-cache/",
            "--exclude=.obsidian/",
            "--exclude=.corruption-quarantine-*/",
            "--exclude=.html/",
            "--exclude=.DS_Store",
            "--exclude=*.tmp",
            "--exclude=*.temp",
            "--exclude=*.mp4",
            "--exclude=*.mov",
        ]
        .iter()
        .map(|s| s.to_string())
        .chain([format!("{home}/Forge/"), format!("{home}/Library/CloudStorage/Dropbox/Forge/")])
        .collect();
        assert_eq!(argv, expected);
        assert_eq!(admin_argv(&paths).len(), RSYNC_OPTS.len() + ADMIN_EXCLUDES.len() + 2);
    }

    #[test]
    fn green_control_both_trees_ok() {
        let (_d, paths) = fixture();
        let mut fake = Fake::default();
        respond(&mut fake, &forge_argv(&paths), CmdResult::success("sending incremental file list\n"));
        respond(&mut fake, &admin_argv(&paths), CmdResult::success(""));
        assert_eq!(run(&fake, &paths), 0);
        let log = std::fs::read_to_string(&paths.log).unwrap();
        assert!(log.contains("] Starting Forge+Admin sync to Dropbox..."), "{log}");
        assert!(log.contains("sending incremental file list"), "rsync output appended: {log}");
        assert!(log.contains("] Forge sync OK\n"), "{log}");
        assert!(log.contains("] Admin sync OK\n"), "{log}");
        assert!(log.contains("] Sync complete."), "{log}");
        assert!(!log.contains("FAILED"), "{log}");
        let calls = fake.calls.borrow();
        assert_eq!(calls.len(), 2, "two rsyncs, no banner: {calls:?}");
        assert_eq!(calls[0], Fake::key("rsync", &forge_argv(&paths).iter().map(String::as_str).collect::<Vec<_>>()), "exact Forge argv");
        let v = heartbeat_json(&paths);
        assert_eq!(v["interval_secs"], 1800);
        assert!(v["last_error"].is_null(), "{v}");
        assert_eq!(v["actions"], 2);
    }

    #[test]
    fn red_control_forge_failure_is_notified_and_exit_one() {
        let (_d, paths) = fixture();
        let mut fake = Fake::default();
        respond(&mut fake, &forge_argv(&paths), CmdResult::failure(23, "rsync: opendir failed: Operation not permitted (1)"));
        respond(&mut fake, &admin_argv(&paths), CmdResult::success(""));
        assert_eq!(run(&fake, &paths), 1);
        let log = std::fs::read_to_string(&paths.log).unwrap();
        assert!(log.contains("Operation not permitted"), "rsync stderr appended: {log}");
        assert!(log.contains("] Forge sync FAILED (exit 23)\n"), "{log}");
        assert!(log.contains("] Admin sync OK\n"), "the second tree still runs: {log}");
        assert!(log.contains("] Sync complete."), "{log}");
        let calls = fake.calls.borrow();
        assert!(calls.iter().any(|c| c.starts_with("notify-user --tool sync-forge-to-dropbox Forge→Dropbox sync failed Forge rsync exit 23")), "{calls:?}");
        let v = heartbeat_json(&paths);
        assert_eq!(v["last_error"], "Forge rsync exit 23");
        assert_eq!(v["actions"], 1);
    }

    #[test]
    fn vanished_files_exit_24_is_ok_with_a_note() {
        let (_d, paths) = fixture();
        let mut fake = Fake::default();
        respond(&mut fake, &forge_argv(&paths), CmdResult::failure(24, "file has vanished"));
        respond(&mut fake, &admin_argv(&paths), CmdResult::success(""));
        assert_eq!(run(&fake, &paths), 0);
        let log = std::fs::read_to_string(&paths.log).unwrap();
        assert!(log.contains("] Forge sync OK (rsync 24"), "{log}");
        assert!(!log.contains("FAILED"), "{log}");
        assert!(!fake.calls.borrow().iter().any(|c| c.starts_with("notify-user")));
        assert!(heartbeat_json(&paths)["last_error"].is_null());
        assert_eq!(classify(24), TreeResult::OkVanished);
        assert_eq!(classify(0), TreeResult::Ok);
        assert_eq!(classify(23), TreeResult::Failed(23));
    }

    #[test]
    fn missing_dropbox_folder_is_a_failure_not_a_sync_into_nowhere() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("Forge")).unwrap();
        let paths = Paths::under(d.path());
        let fake = Fake::default();
        assert_eq!(run(&fake, &paths), 1);
        let calls = fake.calls.borrow();
        assert!(!calls.iter().any(|c| c.starts_with("rsync")), "no rsync ran: {calls:?}");
        assert!(calls.iter().any(|c| c.starts_with("notify-user --tool sync-forge-to-dropbox")), "{calls:?}");
        let log = std::fs::read_to_string(&paths.log).unwrap();
        assert!(log.contains("FAILED: Dropbox folder missing"), "{log}");
        assert!(heartbeat_json(&paths)["last_error"].as_str().unwrap().starts_with("Dropbox folder missing"));
    }

    #[test]
    fn log_is_capped_at_one_megabyte_with_predecessor_dot_one() {
        let (_d, paths) = fixture();
        std::fs::create_dir_all(paths.log.parent().unwrap()).unwrap();
        std::fs::write(&paths.log, vec![b'x'; (LOG_CAP + 1) as usize]).unwrap();
        let mut fake = Fake::default();
        respond(&mut fake, &forge_argv(&paths), CmdResult::success(""));
        respond(&mut fake, &admin_argv(&paths), CmdResult::success(""));
        assert_eq!(run(&fake, &paths), 0);
        assert!(std::fs::metadata(logkeep::predecessor(&paths.log)).unwrap().len() > LOG_CAP);
        assert!(!paths.log.with_extension("log.old").exists());
        assert!(std::fs::read_to_string(&paths.log).unwrap().contains("log capped at 1 MB"));
    }
}
