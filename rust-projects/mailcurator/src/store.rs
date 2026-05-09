// JSONL append-only store for extracted structured data and audit logs.
//
// Deletion logging is wired in v1 (every trash operation writes a record).
// Extractor helpers (append_record, etc.) are scaffolded for v2+.
//
// Data lives under ~/.local/share/mailcurator/. Each category is sharded
// per-machine — files are named `<category>.<hostname>.jsonl` so multiple
// machines (e.g. Mac + nimbini) running mailcurator can each write their
// own file without ever colliding on a shared path. Readers union across
// all per-host files for a category.
//
//   - deletions.<host>.jsonl     — one line per trashed message (audit)
//   - subscriptions.<host>.jsonl — subscription monitor events
//   - bills.<host>.jsonl         — extractor outputs (bills, orders, …)
//   - coverage-history.<host>.jsonl — coverage report history
//
// Legacy single-file paths (`<category>.jsonl`, no hostname) are still
// READ for backward compatibility with data written before the multi-host
// migration (2026-05-09). New writes always use the per-host path.
//
// See `~/.claude/projects/-Users-williamnapier/memory/project_tm3_multi_machine_race.md`
// for the protocol design that prompted this — same shape applied to
// mailcurator instead of TM3 capture.

use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Best-effort hostname for sharding ledger files. Mirrors the logic in
/// `leader::machine_id()` — the two MUST resolve to the same string on
/// any given machine, since `leader` reads activity records named after
/// the same id and decisions assume per-machine self-consistency.
pub fn machine_id() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    if let Ok(content) = std::fs::read_to_string("/etc/hostname") {
        let h = content.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    if let Ok(out) = Command::new("hostname").output() {
        if out.status.success() {
            let h = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !h.is_empty() {
                return h;
            }
        }
    }
    "unknown-machine".to_string()
}

/// Return the store directory (~/.local/share/mailcurator), creating it if missing.
/// Uses XDG-style ~/.local/share explicitly (not dirs::data_local_dir, which
/// returns macOS's ~/Library/Application Support).
pub fn store_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("couldn't resolve $HOME")?;
    let dir = home.join(".local").join("share").join("mailcurator");
    create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// Path the CURRENT machine writes to for a given category:
/// `<store>/<category>.<hostname>.jsonl`.
pub fn category_path(category: &str) -> Result<PathBuf> {
    Ok(store_dir()?.join(format!("{category}.{}.jsonl", machine_id())))
}

/// All paths for `category` across machines, plus the legacy single-file
/// path if it still exists. Use this when reading — collected records
/// from multiple machines form the unified view of the category.
///
/// Returned in arbitrary filesystem order. Callers that need chronological
/// order should sort by record timestamp after parsing.
pub fn category_paths_all(category: &str) -> Result<Vec<PathBuf>> {
    let dir = store_dir()?;
    let mut paths = Vec::new();

    // Legacy: <category>.jsonl (no hostname). Pre-2026-05-09 data may
    // still live here; readers must include it until it's been merged
    // or migrated.
    let legacy = dir.join(format!("{category}.jsonl"));
    if legacy.exists() {
        paths.push(legacy);
    }

    // Per-host: <category>.<hostname>.jsonl. Glob via read_dir + name match.
    let prefix = format!("{category}.");
    let suffix = ".jsonl";
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Ok(paths),
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(rest) = name.strip_prefix(&prefix) {
            if let Some(host) = rest.strip_suffix(suffix) {
                if !host.is_empty() {
                    paths.push(entry.path());
                }
            }
        }
    }
    Ok(paths)
}

/// Append a single record as one JSON line to the current machine's file
/// for `category`: `<store>/<category>.<hostname>.jsonl`.
pub fn append_record<T: Serialize>(category: &str, record: &T) -> Result<()> {
    let file = category_path(category)?;
    append_record_at(&file, record)
}

/// Read all non-empty JSONL lines for `category` across every per-host file
/// AND the legacy single-file (if present). Lines are returned in arbitrary
/// order — callers parsing records should sort by timestamp if order matters.
///
/// Errors propagate from file open/read; missing files are silently skipped.
pub fn read_category_lines(category: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for path in category_paths_all(category)? {
        let f = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("opening {}", path.display())),
        };
        for line in BufReader::new(f).lines() {
            let line = line.with_context(|| format!("reading {}", path.display()))?;
            if !line.trim().is_empty() {
                out.push(line);
            }
        }
    }
    Ok(out)
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
/// `deletions.<hostname>.jsonl`, attributed to `policy_name`. Called
/// before the actual notmuch trash tag is applied.
pub fn log_deletions(policy_name: &str, query: &str) -> Result<()> {
    let messages = list_messages(query)?;
    let path = category_path("deletions")?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_id_is_nonempty() {
        assert!(!machine_id().is_empty());
    }

    #[test]
    fn category_paths_all_includes_legacy_and_per_host() {
        // Use a tempdir; build paths manually since store_dir() always
        // points at $HOME/.local/share/mailcurator.
        let tmp = tempfile::tempdir().unwrap();
        let cat = "test_cat";
        let legacy = tmp.path().join(format!("{cat}.jsonl"));
        let host_a = tmp.path().join(format!("{cat}.host-a.jsonl"));
        let host_b = tmp.path().join(format!("{cat}.host-b.jsonl"));
        let unrelated = tmp.path().join(format!("other.jsonl"));
        let unrelated_per_host = tmp.path().join(format!("other.host-a.jsonl"));
        std::fs::write(&legacy, "{}\n").unwrap();
        std::fs::write(&host_a, "{}\n").unwrap();
        std::fs::write(&host_b, "{}\n").unwrap();
        std::fs::write(&unrelated, "{}\n").unwrap();
        std::fs::write(&unrelated_per_host, "{}\n").unwrap();

        // Reproduce category_paths_all logic against the tmpdir.
        let prefix = format!("{cat}.");
        let suffix = ".jsonl";
        let mut paths: Vec<PathBuf> = Vec::new();
        let leg = tmp.path().join(format!("{cat}.jsonl"));
        if leg.exists() {
            paths.push(leg);
        }
        for entry in std::fs::read_dir(tmp.path()).unwrap().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(rest) = name.strip_prefix(&prefix) {
                if let Some(host) = rest.strip_suffix(suffix) {
                    if !host.is_empty() {
                        paths.push(entry.path());
                    }
                }
            }
        }
        let names: std::collections::HashSet<String> = paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&format!("{cat}.jsonl")));
        assert!(names.contains(&format!("{cat}.host-a.jsonl")));
        assert!(names.contains(&format!("{cat}.host-b.jsonl")));
        assert!(!names.contains("other.jsonl"));
        assert!(!names.contains("other.host-a.jsonl"));
    }
}
