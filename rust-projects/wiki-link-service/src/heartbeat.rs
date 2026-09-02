//! Heartbeat file — the watcher convention:
//! `~/.local/state/watchers/wiki-link-service-<sub>.json`, rewritten
//! atomically at startup and after every handled event.
//!
//! `{"watcher":…,"version":…,"started_at":RFC3339,"last_cycle":RFC3339,
//!   "last_action":RFC3339|null,"actions":N,"last_error":string|null,
//!   "host":hostname,"interval_secs":0}`
//!
//! "action" = a file rewritten. `interval_secs` is 0: event-driven, no cadence.

use chrono::{DateTime, Local, SecondsFormat};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Heartbeat {
    path: PathBuf,
    watcher: String,
    started_at: DateTime<Local>,
    last_action: Option<DateTime<Local>>,
    actions: u64,
    last_error: Option<String>,
    host: String,
}

pub fn default_state_dir() -> PathBuf {
    crate::wiki::home().join(".local/state/watchers")
}

fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn rfc3339(t: &DateTime<Local>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, false)
}

impl Heartbeat {
    /// `state_dir/wiki-link-service-<sub>.json`; writes the startup record.
    pub fn new(state_dir: &Path, sub: &str) -> Self {
        let hb = Heartbeat {
            path: state_dir.join(format!("wiki-link-service-{sub}.json")),
            watcher: format!("wiki-link-service-{sub}"),
            started_at: Local::now(),
            last_action: None,
            actions: 0,
            last_error: None,
            host: hostname(),
        };
        hb.write();
        hb
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Record one handled event: `actions` files rewritten, optional error.
    pub fn cycle(&mut self, actions: usize, error: Option<String>) {
        if actions > 0 {
            self.actions += actions as u64;
            self.last_action = Some(Local::now());
        }
        if error.is_some() {
            self.last_error = error;
        }
        self.write();
    }

    pub fn render(&self, now: &DateTime<Local>) -> String {
        let opt = |t: &Option<DateTime<Local>>| t.as_ref().map(|t| json_str(&rfc3339(t))).unwrap_or_else(|| "null".into());
        let err = self.last_error.as_deref().map(json_str).unwrap_or_else(|| "null".into());
        format!(
            "{{\"watcher\":{},\"version\":{},\"started_at\":{},\"last_cycle\":{},\"last_action\":{},\"actions\":{},\"last_error\":{},\"host\":{},\"interval_secs\":0}}\n",
            json_str(&self.watcher),
            json_str(env!("CARGO_PKG_VERSION")),
            json_str(&rfc3339(&self.started_at)),
            json_str(&rfc3339(now)),
            opt(&self.last_action),
            self.actions,
            err,
            json_str(&self.host),
        )
    }

    /// Atomic: write `<path>.tmp` then rename over `<path>`. Failures are
    /// swallowed — a heartbeat must never take the watcher down.
    fn write(&self) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = self.path.with_extension("json.tmp");
        if std::fs::write(&tmp, self.render(&Local::now())).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_json_shape_and_atomic_write() {
        let d = tempfile::tempdir().unwrap();
        let mut hb = Heartbeat::new(d.path(), "backlinks");
        let s = std::fs::read_to_string(hb.path()).unwrap();
        assert!(s.starts_with("{\"watcher\":\"wiki-link-service-backlinks\",\"version\":\""));
        assert!(s.contains("\"last_action\":null,\"actions\":0,\"last_error\":null,\"host\":\""));
        assert!(s.ends_with("\"interval_secs\":0}\n"));
        hb.cycle(2, Some("boom \"quoted\"".into()));
        let s = std::fs::read_to_string(hb.path()).unwrap();
        assert!(s.contains("\"actions\":2,\"last_error\":\"boom \\\"quoted\\\"\""));
        assert!(!s.contains("\"last_action\":null"));
        assert!(!d.path().join("wiki-link-service-backlinks.json.tmp").exists());
    }
}
