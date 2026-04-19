//! Outcome measures — read-only helper used by the clinical notes toolchain.
//!
//! Files live at `~/Clinical/clients/<id>/outcomes/<measure>.yaml`.
//! This module provides only the read path and context formatting needed to
//! inject outcome data into session-note prompts.  Full record management
//! (write, add, validate) lives in the practiceforge crate.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Data types (must stay in sync with practiceforge's outcomes.rs)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutcomeEntry {
    pub date: String,
    pub score: f64,
    pub items: Option<Vec<f64>>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomeRecord {
    pub measure: String,
    pub client_id: String,
    pub entries: Vec<OutcomeEntry>,
}

impl OutcomeRecord {
    pub fn latest(&self) -> Option<&OutcomeEntry> {
        self.entries.last()
    }

    pub fn trend(&self) -> Option<f64> {
        if self.entries.len() < 2 {
            return None;
        }
        let last = self.entries[self.entries.len() - 1].score;
        let prev = self.entries[self.entries.len() - 2].score;
        Some(last - prev)
    }
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

fn outcomes_dir(client_id: &str) -> PathBuf {
    crate::client::clients_dir()
        .join(client_id)
        .join("outcomes")
}

// ---------------------------------------------------------------------------
// Severity labels
// ---------------------------------------------------------------------------

pub fn severity_label(measure: &str, score: f64) -> &'static str {
    match measure {
        "phq9" => {
            if score <= 4.0       { "Minimal" }
            else if score <= 9.0  { "Mild" }
            else if score <= 14.0 { "Moderate" }
            else if score <= 19.0 { "Mod-severe" }
            else                   { "Severe" }
        }
        "gad7" => {
            if score <= 4.0       { "Minimal" }
            else if score <= 9.0  { "Mild" }
            else if score <= 14.0 { "Moderate" }
            else                   { "Severe" }
        }
        "core10" => {
            if score <= 10.0      { "Healthy" }
            else if score <= 14.0 { "Low" }
            else if score <= 19.0 { "Mild/mod" }
            else if score <= 25.0 { "Mod/severe" }
            else                   { "Severe" }
        }
        "pcl5" => {
            if score < 33.0 { "Below threshold" }
            else            { "PTSD probable" }
        }
        "wemwbs" => {
            if score <= 40.0      { "Low" }
            else if score <= 59.0 { "Moderate" }
            else                   { "High" }
        }
        "isi" => {
            if score <= 7.0       { "No clinically sig" }
            else if score <= 14.0 { "Subthreshold" }
            else if score <= 21.0 { "Moderate" }
            else                   { "Severe" }
        }
        _ => "",
    }
}

fn display_name(measure: &str) -> String {
    match measure {
        "phq9"   => "PHQ-9".to_string(),
        "gad7"   => "GAD-7".to_string(),
        "core10" => "CORE-10".to_string(),
        "pcl5"   => "PCL-5".to_string(),
        "wemwbs" => "WEMWBS".to_string(),
        "isi"    => "ISI".to_string(),
        _        => measure.to_uppercase(),
    }
}

fn higher_is_better(measure: &str) -> bool {
    measure == "wemwbs"
}

fn format_score(score: f64) -> String {
    if score.fract() == 0.0 {
        format!("{}", score as i64)
    } else {
        format!("{:.1}", score)
    }
}

// ---------------------------------------------------------------------------
// Load all records for a client
// ---------------------------------------------------------------------------

pub fn all_measures_for_client(client_id: &str) -> Vec<OutcomeRecord> {
    let dir = outcomes_dir(client_id);
    if !dir.exists() {
        return vec![];
    }

    let read_dir = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(_) => return vec![],
    };

    let mut records = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        // Validate the file stem is non-empty; the record's own `measure` field
        // is used as the canonical name after deserialisation.
        let _measure_check = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => continue,
        };
        let yaml = match std::fs::read_to_string(&path) {
            Ok(y) => y,
            Err(_) => continue,
        };
        let record: OutcomeRecord = match serde_yaml::from_str(&yaml) {
            Ok(r) => r,
            Err(_) => continue,
        };
        records.push(record);
    }

    records.sort_by(|a, b| a.measure.cmp(&b.measure));
    records
}

// ---------------------------------------------------------------------------
// Context block for LLM prompts
// ---------------------------------------------------------------------------

/// Build a markdown table of all outcome measures for a client.
///
/// Returns an empty string when no outcomes are recorded.  Insert the result
/// directly into the session-note prompt context.
pub fn outcomes_context_block(client_id: &str) -> String {
    let records = all_measures_for_client(client_id);
    let records: Vec<&OutcomeRecord> =
        records.iter().filter(|r| !r.entries.is_empty()).collect();

    if records.is_empty() {
        return String::new();
    }

    let mut out = String::from("## Outcome Measures\n");
    out.push_str("| Measure | Latest | Date | Severity | Trend |\n");
    out.push_str("|---------|--------|------|----------|-------|\n");

    for record in &records {
        let latest = match record.latest() {
            Some(e) => e,
            None    => continue,
        };

        let name     = display_name(&record.measure);
        let score    = format_score(latest.score);
        let date     = &latest.date;
        let severity = severity_label(&record.measure, latest.score);

        let trend_str = match record.trend() {
            None => String::from("–"),
            Some(delta) => {
                let abs = format_score(delta.abs());
                let better = if higher_is_better(&record.measure) {
                    delta > 0.0
                } else {
                    delta < 0.0
                };
                if delta == 0.0 {
                    String::from("→ 0 (stable)")
                } else if better {
                    format!("↑ {} (improving)", abs)
                } else {
                    format!("↓ {} (worsening)", abs)
                }
            }
        };

        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            name, score, date, severity, trend_str
        ));
    }

    out
}
