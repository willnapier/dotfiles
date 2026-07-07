//! JSONL audit log per §7.
//!
//! One line per request: `{ts, ip, ua, path, status, bytes, verified=false}`.
//! Retention: 14-day rolling, purged at startup and once per day.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

const RETENTION_DAYS: i64 = 14;
const PURGE_INTERVAL_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Serialize, Deserialize)]
pub struct AuditEntry {
    pub ts: String,
    pub ip: String,
    pub ua: String,
    pub path: String,
    pub status: u16,
    pub bytes: usize,
    pub verified: bool,
}

#[derive(Clone)]
pub struct AuditLog {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl AuditLog {
    pub fn new(path: PathBuf) -> Self {
        AuditLog {
            path,
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// Append a single JSONL entry. Each entry is built with `verified: false`;
    /// the ack workflow (separate CLI) flips it later.
    pub async fn append(&self, entry: &AuditEntry) -> Result<()> {
        let line = serde_json::to_string(entry)?;
        let _guard = self.lock.lock().await;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("opening audit log {}", self.path.display()))?;
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")?;
        Ok(())
    }

    /// Read-filter-rewrite: drop lines whose `ts` is older than `RETENTION_DAYS`.
    /// Malformed lines (or lines with unparsable timestamps) are kept conservatively —
    /// it's better to keep a possibly-stale line than to silently lose audit data.
    pub async fn purge_old(&self) -> Result<()> {
        let cutoff = Utc::now() - Duration::days(RETENTION_DAYS);
        let _guard = self.lock.lock().await;
        purge_file(&self.path, cutoff)
    }

    /// Spawn the daily purge task. Runs once at startup (via the caller before
    /// invoking this), then once every 24h.
    pub fn spawn_purge_task(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(PURGE_INTERVAL_SECS));
            // First tick fires immediately — skip it because the caller has already
            // run the startup purge synchronously.
            interval.tick().await;
            loop {
                interval.tick().await;
                if let Err(e) = self.purge_old().await {
                    eprintln!("audit purge failed: {e:#}");
                }
            }
        });
    }
}

fn purge_file(path: &Path, cutoff: DateTime<Utc>) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading audit log {}", path.display()))?;
    let mut kept: Vec<&str> = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if line_is_recent(line, cutoff) {
            kept.push(line);
        }
    }
    let mut tmp = path.to_path_buf();
    tmp.set_extension("log.tmp");
    {
        let mut f = std::fs::File::create(&tmp)
            .with_context(|| format!("creating tmp file {}", tmp.display()))?;
        for line in &kept {
            f.write_all(line.as_bytes())?;
            f.write_all(b"\n")?;
        }
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Returns true if the line's `ts` field parses and is >= cutoff,
/// OR if the line is unparseable (conservatively kept).
fn line_is_recent(line: &str, cutoff: DateTime<Utc>) -> bool {
    let v: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return true, // keep unparseable lines
    };
    let ts_str = match v.get("ts").and_then(|t| t.as_str()) {
        Some(s) => s,
        None => return true,
    };
    match DateTime::parse_from_rfc3339(ts_str) {
        Ok(dt) => dt.with_timezone(&Utc) >= cutoff,
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purge_drops_old_keeps_recent() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        // ts 20 days ago — drop
        let old = AuditEntry {
            ts: (Utc::now() - Duration::days(20)).to_rfc3339(),
            ip: "1.1.1.1".into(),
            ua: "x".into(),
            path: "/x".into(),
            status: 404,
            bytes: 0,
            verified: false,
        };
        // ts 1 day ago — keep
        let new = AuditEntry {
            ts: (Utc::now() - Duration::days(1)).to_rfc3339(),
            ip: "2.2.2.2".into(),
            ua: "y".into(),
            path: "/y".into(),
            status: 200,
            bytes: 100,
            verified: false,
        };
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "{}", serde_json::to_string(&old).unwrap()).unwrap();
        writeln!(f, "{}", serde_json::to_string(&new).unwrap()).unwrap();
        drop(f);

        purge_file(&path, Utc::now() - Duration::days(RETENTION_DAYS)).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(!after.contains("1.1.1.1"), "old entry should have been purged");
        assert!(after.contains("2.2.2.2"), "recent entry should be kept");
    }

    #[test]
    fn purge_keeps_unparseable_lines() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        std::fs::write(&path, "garbage not json\n").unwrap();
        purge_file(&path, Utc::now() - Duration::days(RETENTION_DAYS)).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("garbage"));
    }
}
