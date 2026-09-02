//! Binary-level tests. Everything points at a tempdir and a port nobody
//! listens on — never the live Syncthing, never /tmp locks.

use std::net::TcpListener;
use std::process::Command;

/// Reserve a port then release it so nothing listens there.
fn dead_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

/// `--once --dry-run` against an unreachable API: exit 1, no restart, a
/// heartbeat written with the cycle's error and `interval_secs` = tick.
#[test]
fn once_dry_run_writes_heartbeat_and_exits_1_when_unreachable() {
    let d = tempfile::tempdir().unwrap();
    let cfg = d.path().join("config.xml");
    std::fs::write(&cfg, format!("<configuration><gui><address>127.0.0.1:{}</address><apikey>testkey12345</apikey></gui></configuration>", dead_port())).unwrap();
    let log = d.path().join("log");
    let state = d.path().join("state");
    let out = Command::new(env!("CARGO_BIN_EXE_syncthing-connection-monitor"))
        .args(["--once", "--dry-run", "--tick", "77", "--config"])
        .arg(&cfg)
        .arg("--log")
        .arg(&log)
        .arg("--state-dir")
        .arg(&state)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stdout));
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("INFO: 🔍 Starting Syncthing connection monitor"), "{text}");
    assert!(text.contains("INFO: 📊 API Key: testkey1..."), "{text}");
    assert!(text.contains("ERROR: Failed to check connections - Syncthing may be down"), "{text}");
    assert!(text.contains("(dry-run) would restart"), "{text}");
    assert!(!text.contains("restart requested"), "{text}");
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(state.join("syncthing-connection-monitor.json")).unwrap()).unwrap();
    assert_eq!(v["watcher"], "syncthing-connection-monitor");
    assert_eq!(v["interval_secs"], 77);
    assert_eq!(v["actions"], 0);
    assert!(v["last_action"].is_null());
    assert!(v["started_at"].is_string() && v["last_cycle"].is_string());
    assert!(v["last_error"].as_str().unwrap().contains("unreachable"), "{v}");
}

#[test]
fn missing_api_key_exits_1_with_the_oracle_error_line() {
    let d = tempfile::tempdir().unwrap();
    let cfg = d.path().join("config.xml");
    std::fs::write(&cfg, "<configuration><gui><address>127.0.0.1:1</address></gui></configuration>").unwrap();
    let log = d.path().join("log");
    let out = Command::new(env!("CARGO_BIN_EXE_syncthing-connection-monitor"))
        .args(["--once", "--config"])
        .arg(&cfg)
        .arg("--log")
        .arg(&log)
        .arg("--state-dir")
        .arg(d.path().join("state"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(std::fs::read_to_string(&log).unwrap().contains("ERROR: Failed to get Syncthing API key"));
}
