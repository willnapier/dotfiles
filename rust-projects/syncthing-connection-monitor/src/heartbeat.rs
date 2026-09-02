//! Heartbeat artifact — the watcher convention adopted 2026-09-02.
//!
//! Once at startup and after every cycle/event the watcher atomically writes
//! `~/.local/state/watchers/<crate-name>.json` (same-dir tmp file + rename) so
//! a supervisor can tell "alive and cycling" from "process exists".
//! `interval_secs` is the poll interval; 0 means event-driven, and the health
//! check skips staleness for it. The module is vendored verbatim into each
//! watcher crate (no workspace).

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Heartbeat {
    path: PathBuf,
    watcher: &'static str,
    version: &'static str,
    interval_secs: u64,
    started_at: String,
    host: String,
    actions: u64,
    last_action: Option<String>,
    last_error: Option<String>,
}

pub fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn hostname() -> String {
    Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|h| !h.is_empty())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_string()))
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

impl Heartbeat {
    /// `interval_secs`: poll interval, or 0 for an event-driven watcher.
    pub fn new(state_dir: &Path, watcher: &'static str, version: &'static str, interval_secs: u64) -> Self {
        Heartbeat {
            path: state_dir.join(format!("{watcher}.json")),
            watcher,
            version,
            interval_secs,
            started_at: now_rfc3339(),
            host: hostname(),
            actions: 0,
            last_action: None,
            last_error: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// An action = a restart (syncthing monitor) / a candidate reported (dotter watcher).
    pub fn record_action(&mut self) {
        self.actions += 1;
        self.last_action = Some(now_rfc3339());
    }

    /// `last_error` is the error of the most recent cycle; `None` once a cycle succeeds.
    pub fn set_error(&mut self, err: Option<String>) {
        self.last_error = err;
    }

    /// Stamp `last_cycle` and write atomically.
    pub fn write(&self) -> Result<()> {
        let doc = serde_json::json!({
            "watcher": self.watcher,
            "version": self.version,
            "interval_secs": self.interval_secs,
            "started_at": self.started_at,
            "last_cycle": now_rfc3339(),
            "last_action": self.last_action,
            "actions": self.actions,
            "last_error": self.last_error,
            "host": self.host,
        });
        let dir = self.path.parent().context("heartbeat path has no parent")?;
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let tmp = dir.join(format!(".{}.{}.tmp", self.watcher, std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(&doc)?).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path).with_context(|| format!("renaming into {}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_is_written_atomically_with_all_fields() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("nested/watchers");
        let mut hb = Heartbeat::new(&dir, "test-watcher", "9.9.9", 300);
        hb.write().unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(hb.path()).unwrap()).unwrap();
        assert_eq!(v["watcher"], "test-watcher");
        assert_eq!(v["version"], "9.9.9");
        assert_eq!(v["interval_secs"], 300);
        assert_eq!(v["actions"], 0);
        assert!(v["last_action"].is_null());
        assert!(v["last_error"].is_null());
        assert!(v["started_at"].as_str().unwrap().contains('T'));
        assert!(v["last_cycle"].as_str().unwrap().contains('T'));
        assert!(!v["host"].as_str().unwrap().is_empty());

        hb.record_action();
        hb.set_error(Some("boom".into()));
        hb.write().unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(hb.path()).unwrap()).unwrap();
        assert_eq!(v["actions"], 1);
        assert!(v["last_action"].is_string());
        assert_eq!(v["last_error"], "boom");
        let leftovers = std::fs::read_dir(&dir).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".tmp")).count();
        assert_eq!(leftovers, 0, "tmp file left behind");
    }
}
