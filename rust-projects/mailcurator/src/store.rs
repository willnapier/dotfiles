// JSONL append-only store for extracted structured data.
//
// Infrastructure for v2+ extractors. Not wired into v1 lifecycle operations.
// Kept here so the data-shape decisions are visible from day one, and new
// extractors just call `append_record(...)` without introducing schema.

#![allow(dead_code)] // v1 doesn't yet use these; v2 extractors will

use anyhow::{Context, Result};
use serde::Serialize;
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Return the store directory (~/.local/share/mailcurator), creating it if missing.
pub fn store_dir() -> Result<PathBuf> {
    let base = dirs::data_local_dir().context("no XDG_DATA_HOME equivalent")?;
    let dir = base.join("mailcurator");
    create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// Append a single record as one JSON line to <store_dir>/<category>.jsonl.
/// Category examples: "invoices", "deliveries", "extracted".
pub fn append_record<T: Serialize>(category: &str, record: &T) -> Result<()> {
    let dir = store_dir()?;
    let file = dir.join(format!("{category}.jsonl"));
    append_record_at(&file, record)
}

/// Append to a specific file path. Useful for tests.
pub fn append_record_at<T: Serialize>(path: &Path, record: &T) -> Result<()> {
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
