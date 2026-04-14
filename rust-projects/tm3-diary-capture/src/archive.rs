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
    let filename = format!("capture-{}.json", now.format("%Y-%m-%dT%H%M%S"));
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
