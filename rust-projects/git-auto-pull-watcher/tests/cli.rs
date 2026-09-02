//! End-to-end: the real binary in `--once` mode against a bare remote in a
//! tempdir, with a fake `dotter` on a private PATH for the deploy path.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_git-auto-pull-watcher");

fn sh(dir: &Path, args: &[&str]) {
    let o = Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(o.status.success(), "git {:?} in {}: {}", args, dir.display(), String::from_utf8_lossy(&o.stderr));
}

/// Bare remote + clone `a`; a second clone `b` has pushed `pushed` commits on top of the seed.
fn fixture(root: &Path, pushed: usize) -> PathBuf {
    let remote = root.join("remote.git");
    std::fs::create_dir_all(&remote).unwrap();
    sh(&remote, &["init", "--bare", "-q", "-b", "main"]);
    let seed = root.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    sh(&seed, &["init", "-q", "-b", "main"]);
    sh(&seed, &["config", "user.email", "t@t"]);
    sh(&seed, &["config", "user.name", "t"]);
    std::fs::write(seed.join("file"), "0\n").unwrap();
    sh(&seed, &["add", "-A"]);
    sh(&seed, &["commit", "-q", "-m", "seed"]);
    sh(&seed, &["remote", "add", "origin", remote.to_str().unwrap()]);
    sh(&seed, &["push", "-q", "origin", "main"]);
    let a = root.join("a");
    sh(root, &["clone", "-q", remote.to_str().unwrap(), a.to_str().unwrap()]);
    for i in 1..=pushed {
        std::fs::write(seed.join("file"), format!("{i}\n")).unwrap();
        sh(&seed, &["commit", "-q", "-am", &format!("push {i}")]);
        sh(&seed, &["push", "-q", "origin", "main"]);
    }
    a
}

/// A `dotter` on a private PATH that records its invocation and exits `code`.
fn fake_dotter(root: &Path, code: i32) -> PathBuf {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let marker = root.join("dotter-ran");
    let script = format!("#!/bin/sh\necho \"$PWD $*\" > '{}'\necho 'simulated failure' >&2\nexit {code}\n", marker.display());
    let path = bin.join("dotter");
    std::fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

fn run_once(root: &Path, repo: &Path, extra: &[&str], path_prefix: Option<&Path>) -> (i32, String, String) {
    let mut cmd = Command::new(BIN);
    cmd.arg("--once").arg("--repo").arg(repo).arg("--log").arg(root.join("log")).arg("--state-dir").arg(root.join("state")).args(extra);
    if let Some(p) = path_prefix {
        let old = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{}:{old}", p.display()));
    }
    let o = cmd.output().unwrap();
    let log = std::fs::read_to_string(root.join("log")).unwrap_or_default();
    let hb = std::fs::read_to_string(root.join("state/git-auto-pull-watcher.json")).unwrap_or_default();
    (o.status.code().unwrap_or(-1), log, hb)
}

#[test]
fn once_with_nothing_behind_is_clean() {
    let d = tempfile::tempdir().unwrap();
    let a = fixture(d.path(), 0);
    let (code, log, hb) = run_once(d.path(), &a, &[], None);
    assert_eq!(code, 0);
    assert!(log.is_empty(), "nothing to say when up to date: {log}");
    assert!(hb.contains("\"watcher\":\"git-auto-pull-watcher\""), "{hb}");
    assert!(hb.contains("\"interval_secs\":120,"), "{hb}");
    assert!(hb.contains("\"actions\":0,\"last_error\":null"), "{hb}");
    assert!(hb.contains("\"last_action\":null"), "{hb}");
    assert!(!hb.contains("\"last_cycle\":null"), "{hb}");
}

#[test]
fn once_pulls_and_runs_dotter_for_a_deploy_repo() {
    let d = tempfile::tempdir().unwrap();
    let a = fixture(d.path(), 3);
    let bin = fake_dotter(d.path(), 0);
    let (code, log, hb) = run_once(d.path(), &a, &["--deploy", a.to_str().unwrap()], Some(&bin));
    assert_eq!(code, 0, "{log}");
    assert_eq!(std::fs::read_to_string(a.join("file")).unwrap(), "3\n");
    assert!(log.contains("📥 [a] Remote changes detected: 3 commits behind"), "{log}");
    assert!(log.contains("✅ [a] Successfully pulled changes"), "{log}");
    assert_eq!(log.matches("✅ [a] Dotter deploy successful - configs updated").count(), 1, "{log}");
    let ran = std::fs::read_to_string(d.path().join("dotter-ran")).unwrap();
    assert!(ran.trim().ends_with("deploy"), "{ran}");
    assert!(hb.contains("\"actions\":1,\"last_error\":null"), "{hb}");
}

#[test]
fn once_without_deploy_flag_leaves_dotter_alone() {
    let d = tempfile::tempdir().unwrap();
    let a = fixture(d.path(), 1);
    let bin = fake_dotter(d.path(), 0);
    let (code, log, _) = run_once(d.path(), &a, &[], Some(&bin));
    assert_eq!(code, 0, "{log}");
    assert!(log.contains("✅ [a] Successfully pulled changes"), "{log}");
    assert!(!log.contains("Dotter"), "{log}");
    assert!(!d.path().join("dotter-ran").exists());
}

#[test]
fn once_exits_one_when_deploy_fails() {
    let d = tempfile::tempdir().unwrap();
    let a = fixture(d.path(), 1);
    let bin = fake_dotter(d.path(), 3);
    let (code, log, hb) = run_once(d.path(), &a, &["--deploy", a.to_str().unwrap()], Some(&bin));
    assert_eq!(code, 1, "{log}");
    assert!(log.contains("✅ [a] Successfully pulled changes"), "{log}");
    assert!(log.contains("❌ [a] Dotter deploy failed: simulated failure"), "{log}");
    assert!(hb.contains("\"actions\":1,\"last_error\":\"[a] simulated failure\""), "{hb}");
}

#[test]
fn dry_run_touches_nothing() {
    let d = tempfile::tempdir().unwrap();
    let a = fixture(d.path(), 2);
    let bin = fake_dotter(d.path(), 0);
    let (code, log, _) = run_once(d.path(), &a, &["--dry-run", "--deploy", a.to_str().unwrap()], Some(&bin));
    assert_eq!(code, 0, "{log}");
    assert_eq!(std::fs::read_to_string(a.join("file")).unwrap(), "0\n");
    assert!(log.contains("(dry-run) [a] would pull 2 commits and run dotter deploy"), "{log}");
    assert!(!d.path().join("dotter-ran").exists());
}
