//! 7-day JSON archive of captured TM3 diary data.
//!
//! Each capture writes a timestamped JSON file to ~/.local/share/tm3-appointments/.
//! Files older than 7 days are cleaned up automatically at capture time.

use crate::html::DaySchedule;
use anyhow::{Context, Result};
use chrono::Local;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize)]
struct CaptureArchive<'a> {
    captured_at: String,
    source: &'a str,
    days: &'a [DaySchedule],
}

fn archive_dir() -> PathBuf {
    dirs::home_dir()
        .expect("no home dir")
        .join(".local/share/tm3-appointments")
}

/// Save captured schedules as a JSON archive.
pub fn save(schedules: &[DaySchedule], source: &str) -> Result<PathBuf> {
    let dir = archive_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create archive dir: {}", dir.display()))?;

    let now = Local::now();
    // One file per date — re-capture overwrites previous
    let date_str = schedules.first()
        .map(|s| s.date.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| now.format("%Y-%m-%d").to_string());
    let filename = format!("capture-{date_str}.json");
    let path = dir.join(&filename);

    let archive = CaptureArchive {
        captured_at: now.to_rfc3339(),
        source,
        days: schedules,
    };

    let json = serde_json::to_string_pretty(&archive)
        .context("serialize archive")?;
    std::fs::write(&path, json)
        .with_context(|| format!("write archive: {}", path.display()))?;

    Ok(path)
}

/// Delete archive files older than 7 days.
pub fn cleanup() -> Result<usize> {
    let dir = archive_dir();
    if !dir.exists() {
        return Ok(0);
    }

    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs(7 * 24 * 3600);

    let mut removed = 0;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    if modified < cutoff {
                        std::fs::remove_file(&path)?;
                        removed += 1;
                    }
                }
            }
        }
    }

    Ok(removed)
}


/// Write a dashboard session file for the given date's appointments.
/// Called after client ID mapping so IDs are resolved.
/// A client entry for the dashboard session file.
pub struct SessionClient {
    pub id: String,
    pub start_time: String,
    pub end_time: String,
    pub rate_tag: Option<String>,
    pub status: String,  // "pending", "cancelled", etc.
}

pub fn write_dashboard_session(
    date: &chrono::NaiveDate,
    clients: &[SessionClient],
) -> Result<()> {
    let session_dir = dirs::home_dir()
        .expect("no home dir")
        .join(".local/share/clinical-dashboard");
    std::fs::create_dir_all(&session_dir)
        .with_context(|| format!("create session dir: {}", session_dir.display()))?;

    let date_str = date.format("%Y-%m-%d").to_string();
    let path = session_dir.join(format!("session-{date_str}.json"));

    // If session file already exists, merge — don't overwrite (preserves done/dna status)
    let mut existing_clients: std::collections::HashMap<String, serde_json::Value> =
        std::collections::HashMap::new();
    if path.exists() {
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(session) = serde_json::from_str::<serde_json::Value>(&data) {
                if let Some(arr) = session.get("clients").and_then(|v| v.as_array()) {
                    for c in arr {
                        if let Some(id) = c.get("id").and_then(|v| v.as_str()) {
                            existing_clients.insert(id.to_string(), c.clone());
                        }
                    }
                }
            }
        }
    }

    // Build client list: preserve existing done/dna status, add new with captured status
    let mut session_clients = Vec::new();
    for c in clients {
        if let Some(existing) = existing_clients.remove(&c.id) {
            // Keep existing entry if user already marked it done/dna
            let existing_status = existing.get("status").and_then(|v| v.as_str()).unwrap_or("");
            if existing_status == "done" || existing_status == "dna" {
                session_clients.push(existing);
                continue;
            }
        }
        let mut obj = serde_json::Map::new();
        obj.insert("id".into(), serde_json::Value::String(c.id.clone()));
        obj.insert("time".into(), serde_json::Value::String(c.start_time.clone()));
        obj.insert("end_time".into(), serde_json::Value::String(c.end_time.clone()));
        obj.insert("status".into(), serde_json::Value::String(c.status.clone()));
        if let Some(ref tag) = c.rate_tag {
            obj.insert("rate_tag".into(), serde_json::Value::String(tag.clone()));
        }
        session_clients.push(serde_json::Value::Object(obj));
    }
    // Keep any existing clients not in today's TM3 capture (walk-ins added manually)
    for (_, v) in existing_clients {
        session_clients.push(v);
    }

    let session = serde_json::json!({
        "date": date_str,
        "started_at": chrono::Local::now().to_rfc3339(),
        "clients": session_clients,
    });

    let json = serde_json::to_string_pretty(&session).context("serialize session")?;
    std::fs::write(&path, json).with_context(|| format!("write session: {}", path.display()))?;

    eprintln!("Dashboard session: {}", path.display());
    Ok(())
}
