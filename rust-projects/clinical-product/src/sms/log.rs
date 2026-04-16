//! SMS delivery logging — JSONL files per date.
//!
//! Log format: one JSON object per line at
//! `~/.local/share/clinical-product/sms-log/YYYY-MM-DD.jsonl`

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

/// A single SMS delivery log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmsLogEntry {
    pub timestamp: String,
    pub client_id: String,
    pub client_name: String,
    pub phone: String,
    pub message_sid: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub appointment_date: String,
    pub appointment_time: String,
}

/// Append a log entry to the JSONL file for the appointment date.
pub fn log_send(log_dir: &Path, entry: &SmsLogEntry) -> Result<()> {
    std::fs::create_dir_all(log_dir)?;

    let filename = format!("{}.jsonl", entry.appointment_date);
    let path = log_dir.join(filename);

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("Failed to open log file: {}", path.display()))?;

    let json = serde_json::to_string(entry)
        .context("Failed to serialize log entry")?;

    writeln!(file, "{}", json)
        .with_context(|| format!("Failed to write to log file: {}", path.display()))?;

    Ok(())
}

/// Read all log entries for a given date.
pub fn get_log(log_dir: &Path, date: &str) -> Result<Vec<SmsLogEntry>> {
    let filename = format!("{}.jsonl", date);
    let path = log_dir.join(filename);

    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read log file: {}", path.display()))?;

    let mut entries = Vec::new();
    for (line_num, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<SmsLogEntry>(line) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                eprintln!(
                    "Warning: failed to parse line {} of {}: {}",
                    line_num + 1,
                    path.display(),
                    e
                );
            }
        }
    }

    Ok(entries)
}
