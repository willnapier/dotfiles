//! Binary-level tests. Watch dir, dotter toml, log, heartbeat and lock all
//! live in a tempdir — never the real ~/.config or /tmp lock.

use anyhow::Result;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_dotter-realtime-watcher");

fn wait_for(path: &Path, needle: &str, timeout: Duration) -> String {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Ok(t) = std::fs::read_to_string(path) {
            if t.contains(needle) {
                return t;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    std::fs::read_to_string(path).unwrap_or_default()
}

/// A new `.toml` in the watched dir yields the candidate line and a heartbeat
/// with one action; a file referenced in the toml yields the "already
/// managed" line and no further action.
#[test]
fn candidate_appears_in_log_and_heartbeat() {
    let d = tempfile::tempdir().unwrap();
    // macOS FSEvents reports canonical paths (/private/var/…) while tempdir
    // hands back /var/…; compare on the canonical form.
    let root = d.path().canonicalize().unwrap();
    let watch = root.join("watch");
    std::fs::create_dir_all(&watch).unwrap();
    let toml_path = d.path().join("global.toml");
    std::fs::write(&toml_path, format!("\"m\" = \"{}\"\n", watch.join("managed.toml").display())).unwrap();
    let log = d.path().join("watcher.log");
    let state = d.path().join("state");
    let lock = d.path().join("lock");
    let mut child = Command::new(BIN)
        .arg("--dry-run")
        .args(["--debounce-ms", "200"])
        .arg("--watch")
        .arg(&watch)
        .arg("--dotter-config")
        .arg(&toml_path)
        .arg("--log")
        .arg(&log)
        .arg("--state-dir")
        .arg(&state)
        .arg("--lock")
        .arg(&lock)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let hb_path = state.join("dotter-realtime-watcher.json");
    let result = (|| -> Result<()> {
        let text = wait_for(&log, "⚡ Monitoring active", Duration::from_secs(10));
        anyhow::ensure!(text.contains("⚡ Monitoring active"), "watcher never became active:\n{text}");
        anyhow::ensure!(text.contains("🚀 Starting real-time config watcher"), "{text}");
        anyhow::ensure!(std::fs::read_to_string(&lock)?.trim() == child.id().to_string(), "lock holds our pid");
        // startup heartbeat, before any event
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&hb_path)?)?;
        anyhow::ensure!(v["actions"] == 0 && v["interval_secs"] == 0 && v["last_action"].is_null(), "startup heartbeat: {v}");
        anyhow::ensure!(v["watcher"] == "dotter-realtime-watcher" && v["started_at"].is_string(), "{v}");
        std::thread::sleep(Duration::from_millis(500)); // let the OS watcher settle

        let new = watch.join("new.toml");
        std::fs::write(&new, "a = 1\n")?;
        let needle = format!("🆕 Unmanaged config candidate: {} — run: dotter-add {}", new.display(), new.display());
        let text = wait_for(&log, &needle, Duration::from_secs(15));
        anyhow::ensure!(text.contains(&needle), "candidate line missing:\n{text}");
        anyhow::ensure!(text.contains("(dry-run) would notify: Unmanaged config: new.toml"), "{text}");
        let start = Instant::now();
        loop {
            let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&hb_path)?)?;
            if v["actions"] == 1 {
                anyhow::ensure!(v["last_action"].is_string() && v["last_error"].is_null(), "{v}");
                break;
            }
            anyhow::ensure!(start.elapsed() < Duration::from_secs(5), "heartbeat never recorded the action: {v}");
            std::thread::sleep(Duration::from_millis(100));
        }

        std::fs::write(watch.join("managed.toml"), "b = 2\n")?;
        let text = wait_for(&log, "ℹ️  File already managed", Duration::from_secs(15));
        anyhow::ensure!(text.contains("ℹ️  File already managed or not suitable for onboarding"), "managed line missing:\n{text}");
        anyhow::ensure!(text.matches("🆕 Unmanaged config candidate").count() == 1, "extra candidates:\n{text}");
        Ok(())
    })();
    let _ = child.kill();
    let _ = child.wait();
    result.unwrap();
}

#[test]
fn check_flag_evaluates_one_path_and_exits_with_status() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("thing.toml");
    std::fs::write(&f, "").unwrap();
    let toml_path = d.path().join("global.toml");
    std::fs::write(&toml_path, "").unwrap();
    let log = d.path().join("log");
    let run = |path: &Path| {
        Command::new(BIN)
            .arg("--dry-run")
            .arg("--check")
            .arg(path)
            .arg("--dotter-config")
            .arg(&toml_path)
            .arg("--log")
            .arg(&log)
            .arg("--state-dir")
            .arg(d.path().join("state"))
            .output()
            .unwrap()
    };
    assert_eq!(run(&f).status.code(), Some(0), "unmanaged toml is a candidate");
    std::fs::write(&toml_path, format!("\"t\" = \"{}\"", f.display())).unwrap();
    assert_eq!(run(&f).status.code(), Some(1), "referenced in toml = managed");
    assert_eq!(run(&d.path().join("notes.txt")).status.code(), Some(1), "not config-looking");
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains(&format!("🆕 Unmanaged config candidate: {} — run: dotter-add {}", f.display(), f.display())), "{text}");
    assert!(text.contains("ℹ️  File already managed or not suitable for onboarding"), "{text}");
}

#[test]
fn live_lock_refuses_second_instance() {
    let d = tempfile::tempdir().unwrap();
    let lock = d.path().join("lock");
    std::fs::write(&lock, std::process::id().to_string()).unwrap(); // this test process is alive
    let log = d.path().join("log");
    let out = Command::new(BIN)
        .arg("--watch")
        .arg(d.path())
        .arg("--lock")
        .arg(&lock)
        .arg("--log")
        .arg(&log)
        .arg("--state-dir")
        .arg(d.path().join("state"))
        .arg("--dotter-config")
        .arg(d.path().join("none.toml"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(std::fs::read_to_string(&log).unwrap().contains(&format!("🔒 Real-time watcher already running — pid {}", std::process::id())));
}
