//! Training corpus accounting — counts and lists session notes eligible
//! for voice fine-tuning.
//!
//! A note is eligible unless it carries the exclusion marker
//! (`<!-- training: exclude -->`) immediately after the session header.

use anyhow::{Context, Result};
use chrono::NaiveDate;
use regex::Regex;

use clinical_core::client;

use crate::note::TRAINING_EXCLUDE_MARKER;
use crate::session;

const VOICE_STATE_FILE: &str = "voice-state.toml";

#[derive(Debug)]
pub struct NoteRecord {
    pub client_id: String,
    pub date: NaiveDate,
    pub excluded: bool,
}

/// Walk all client files and extract every session note with its
/// inclusion status.
pub fn collect_all_notes() -> Result<Vec<NoteRecord>> {
    let clients_dir = client::clients_dir();
    let client_files = session::find_client_md_files(&clients_dir)?;

    let date_re = Regex::new(r"^### (\d{4}-\d{2}-\d{2})").unwrap();

    let mut records = Vec::new();

    for (id, path) in &client_files {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read: {}", path.display()))?;

        let lines: Vec<&str> = content.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if let Some(caps) = date_re.captures(line) {
                let date_str = caps.get(1).unwrap().as_str();
                let date = match NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                // Check if the next non-empty line is the exclusion marker
                let excluded = lines
                    .iter()
                    .skip(i + 1)
                    .take(3) // look at up to 3 lines after the header
                    .any(|l| l.trim() == TRAINING_EXCLUDE_MARKER);

                records.push(NoteRecord {
                    client_id: id.clone(),
                    date,
                    excluded,
                });
            }
        }
    }

    records.sort_by(|a, b| a.date.cmp(&b.date));
    Ok(records)
}

/// Load the last fine-tune date from voice-state.toml if present.
pub fn last_finetune_date() -> Option<NaiveDate> {
    let path = voice_state_path();
    let content = std::fs::read_to_string(&path).ok()?;
    let value: toml::Value = toml::from_str(&content).ok()?;
    value
        .get("training")
        .and_then(|t| t.get("last_finetune_date"))
        .and_then(|v| v.as_str())
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
}

fn voice_state_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("VOICE_STATE_PATH") {
        return std::path::PathBuf::from(p);
    }
    let base = if let Some(home) = dirs::home_dir() {
        home.join(".config").join("clinical-product")
    } else {
        std::path::PathBuf::from(".")
    };
    base.join(VOICE_STATE_FILE)
}

/// Run `clinical training count`.
pub fn count(all: bool) -> Result<()> {
    let records = collect_all_notes()?;

    let last_ft = if all { None } else { last_finetune_date() };

    let (since_label, eligible): (String, Vec<&NoteRecord>) = if let Some(d) = last_ft {
        (
            format!("since last fine-tune ({})", d),
            records
                .iter()
                .filter(|r| r.date > d && !r.excluded)
                .collect(),
        )
    } else {
        (
            "all time".to_string(),
            records.iter().filter(|r| !r.excluded).collect(),
        )
    };

    let excluded_count = records.iter().filter(|r| r.excluded).count();
    let total = records.len();

    println!("Training corpus status:");
    println!("  Window:              {}", since_label);
    println!("  Eligible notes:      {}", eligible.len());
    println!("  Total notes:         {}", total);
    println!("  Excluded from train: {}", excluded_count);

    if let Some(d) = last_ft {
        let future: Vec<_> = records.iter().filter(|r| r.date > d).collect();
        println!("  Total since last ft: {}", future.len());
    }

    Ok(())
}

/// Run `clinical training list`.
pub fn list(excluded_only: bool) -> Result<()> {
    let records = collect_all_notes()?;

    let filtered: Vec<&NoteRecord> = if excluded_only {
        records.iter().filter(|r| r.excluded).collect()
    } else {
        records.iter().collect()
    };

    if filtered.is_empty() {
        if excluded_only {
            println!("No notes marked as excluded from training.");
        } else {
            println!("No session notes found.");
        }
        return Ok(());
    }

    for rec in &filtered {
        let marker = if rec.excluded { "  [excluded]" } else { "" };
        println!("{}  {}{}", rec.date, rec.client_id, marker);
    }

    println!();
    println!("{} notes shown.", filtered.len());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_excluded_note() {
        let content = "# TEST01\n\n## Session Notes\n\n\
### 2026-01-15\n<!-- training: exclude -->\n\n**Risk**: None.\nBody.\n**Formulation**: ok.\n\n\
### 2026-01-22\n\n**Risk**: None.\nBody.\n**Formulation**: ok.\n";

        // We can't easily test collect_all_notes without a real client dir, but
        // we can test the exclusion detection logic inline.
        let lines: Vec<&str> = content.lines().collect();
        let date_re = Regex::new(r"^### (\d{4}-\d{2}-\d{2})").unwrap();

        let mut results = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if let Some(caps) = date_re.captures(line) {
                let date = caps.get(1).unwrap().as_str().to_string();
                let excluded = lines
                    .iter()
                    .skip(i + 1)
                    .take(3)
                    .any(|l| l.trim() == TRAINING_EXCLUDE_MARKER);
                results.push((date, excluded));
            }
        }

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "2026-01-15");
        assert!(results[0].1, "First note should be excluded");
        assert_eq!(results[1].0, "2026-01-22");
        assert!(!results[1].1, "Second note should be included");
    }
}
