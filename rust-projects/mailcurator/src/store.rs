// JSONL append-only store for extracted structured data and audit logs.
//
// Deletion logging is wired in v1 (every trash operation writes a record).
// Extractor helpers (append_record, etc.) are scaffolded for v2+.
//
// Data lives under ~/.local/share/mailcurator/:
//   - deletions.jsonl     — one line per trashed message (audit)
//   - invoices.jsonl      — v2+ extractor output
//   - deliveries.jsonl    — v2+ extractor output
//   - extracted.jsonl     — v2+ generic catch-all

use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Return the store directory (~/.local/share/mailcurator), creating it if missing.
/// Uses XDG-style ~/.local/share explicitly (not dirs::data_local_dir, which
/// returns macOS's ~/Library/Application Support).
pub fn store_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("couldn't resolve $HOME")?;
    let dir = home.join(".local").join("share").join("mailcurator");
    create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// Append a single record as one JSON line to <store_dir>/<category>.jsonl.
#[allow(dead_code)] // v2+ extractors will use this
pub fn append_record<T: Serialize>(category: &str, record: &T) -> Result<()> {
    let dir = store_dir()?;
    let file = dir.join(format!("{category}.jsonl"));
    append_record_at(&file, record)
}

/// Append to a specific file path.
fn append_record_at<T: Serialize>(path: &Path, record: &T) -> Result<()> {
    let line = serde_json::to_string(record)
        .context("serializing record to JSON")?;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    writeln!(f, "{line}")
        .with_context(|| format!("appending to {}", path.display()))?;
    Ok(())
}

/// One line in the deletions log — written before each trash operation so
/// the audit trail captures what was actually destroyed. Enables recovery
/// from Gmail Trash (30-day window) and policy-tuning when a real message
/// was caught wrongly.
#[derive(Serialize)]
struct DeletionRecord<'a> {
    ts: String,
    policy: &'a str,
    message_id: String,
    subject: String,
    from: String,
    date: String,
}

/// Log each message that matches `query` as a deletion record under
/// `deletions.jsonl`, attributed to `policy_name`. Called before the
/// actual notmuch trash tag is applied.
pub fn log_deletions(policy_name: &str, query: &str) -> Result<()> {
    let messages = list_messages(query)?;
    let dir = store_dir()?;
    let path = dir.join("deletions.jsonl");
    let ts = Utc::now().to_rfc3339();
    for m in messages {
        let rec = DeletionRecord {
            ts: ts.clone(),
            policy: policy_name,
            message_id: m.message_id,
            subject: m.subject,
            from: m.from,
            date: m.date,
        };
        append_record_at(&path, &rec)?;
    }
    Ok(())
}

/// Minimal message summary — what we need for the preview subcommand,
/// the deletion log, and the unmatched survey.
#[derive(Debug, Clone)]
pub struct MessageSummary {
    pub message_id: String,
    pub subject: String,
    pub from: String,
    pub date: String,
}

/// Return message summaries matching the query (uses `notmuch search
/// --format=json --output=summary` and parses the JSON). This returns
/// thread-level summaries; for single-message threads (the common case
/// with automated mail) each summary represents one message.
pub fn list_messages(query: &str) -> Result<Vec<MessageSummary>> {
    let output = Command::new("notmuch")
        .args(["search", "--format=json", "--output=summary", query])
        .output()
        .with_context(|| format!("spawning `notmuch search {query}`"))?;
    if !output.status.success() {
        anyhow::bail!(
            "notmuch search failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // notmuch returns an array of thread-summary objects
    let raw: Vec<NotmuchSummary> = serde_json::from_slice(&output.stdout)
        .context("parsing notmuch search JSON")?;
    Ok(raw.into_iter().map(|s| s.into()).collect())
}

#[derive(serde::Deserialize)]
struct NotmuchSummary {
    thread: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    authors: String,
    #[serde(default)]
    date_relative: String,
    #[serde(default)]
    timestamp: i64,
}

impl From<NotmuchSummary> for MessageSummary {
    fn from(s: NotmuchSummary) -> Self {
        Self {
            message_id: s.thread,
            subject: s.subject,
            from: s.authors,
            date: if s.timestamp > 0 {
                s.date_relative
            } else {
                String::new()
            },
        }
    }
}
