//! Recorded outcome of a scheduled run — the WATCHERS.md heartbeat schema, written
//! atomically to ~/.local/state/watchers/<name>.json so system-health-check Check 9
//! and service-health-check read it with rules they already have: last_error →
//! problem; interval_secs > 0 and last_cycle older than max(3×interval, 15 min) →
//! the timer stopped firing. For a oneshot, started_at is this run's start.
use std::path::{Path, PathBuf};

pub struct Outcome<'a> {
    pub name: &'a str,
    pub interval_secs: u64,
    pub started_at: String,
    pub last_action: Option<String>,
    pub actions: u64,
    pub last_error: Option<String>,
}

pub fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn hostname() -> String {
    if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

pub fn state_dir(home: &Path) -> PathBuf {
    home.join(".local/state/watchers")
}

/// Writes `<state_dir>/<name>.json` via tmp + rename. A failure here is worth a
/// log line, never a reason to change the tool's exit status.
pub fn record(dir: &Path, o: &Outcome) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let json = serde_json::json!({
        "watcher": o.name,
        "version": env!("CARGO_PKG_VERSION"),
        "interval_secs": o.interval_secs,
        "started_at": o.started_at,
        "last_cycle": now_rfc3339(),
        "last_action": o.last_action,
        "actions": o.actions,
        "last_error": o.last_error,
        "host": hostname(),
    });
    let path = dir.join(format!("{}.json", o.name));
    let tmp = dir.join(format!(".{}.json.tmp", o.name));
    std::fs::write(&tmp, format!("{}\n", serde_json::to_string(&json)?))?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}
