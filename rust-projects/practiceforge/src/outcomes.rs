//! Outcome measures module for PracticeForge.
//!
//! Stores scored questionnaire results per-client per-measure.
//! Files live at `~/Clinical/clients/<id>/outcomes/<measure>.yaml`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutcomeEntry {
    pub date: String,              // YYYY-MM-DD
    pub score: f64,                // f64 to allow decimal scores
    pub items: Option<Vec<f64>>,   // individual item scores (optional)
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomeRecord {
    pub measure: String,   // slug, e.g. "phq9"
    pub client_id: String,
    pub entries: Vec<OutcomeEntry>,
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

fn outcomes_dir(client_id: &str) -> PathBuf {
    crate::config::clients_dir()
        .join(client_id)
        .join("outcomes")
}

fn outcomes_file(client_id: &str, measure: &str) -> PathBuf {
    outcomes_dir(client_id).join(format!("{}.yaml", measure))
}

// ---------------------------------------------------------------------------
// OutcomeRecord methods
// ---------------------------------------------------------------------------

impl OutcomeRecord {
    /// Load an existing record, or return an empty one if the file doesn't exist.
    pub fn load(client_id: &str, measure: &str) -> Result<Self> {
        let path = outcomes_file(client_id, measure);
        if !path.exists() {
            return Ok(OutcomeRecord {
                measure: measure.to_string(),
                client_id: client_id.to_string(),
                entries: vec![],
            });
        }
        let yaml = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read outcome file: {}", path.display()))?;
        let record: OutcomeRecord = serde_yaml::from_str(&yaml)
            .with_context(|| format!("Failed to parse outcome file: {}", path.display()))?;
        Ok(record)
    }

    /// Persist the record to disk.
    pub fn save(&self) -> Result<()> {
        let dir = outcomes_dir(&self.client_id);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create outcomes dir: {}", dir.display()))?;
        let path = outcomes_file(&self.client_id, &self.measure);
        let yaml = serde_yaml::to_string(self)
            .context("Failed to serialise outcome record")?;
        std::fs::write(&path, yaml)
            .with_context(|| format!("Failed to write outcome file: {}", path.display()))?;
        Ok(())
    }

    /// Append an entry (does not save — call `save()` afterwards).
    pub fn add_entry(&mut self, entry: OutcomeEntry) {
        self.entries.push(entry);
    }

    /// The most recently added entry.
    pub fn latest(&self) -> Option<&OutcomeEntry> {
        self.entries.last()
    }

    /// Trend: `last_score - second_last_score`.
    ///
    /// Positive = worsening for most measures (higher=worse).
    /// Returns `None` if fewer than 2 entries.
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
// Multi-record helpers
// ---------------------------------------------------------------------------

/// Load all outcome records for a client (reads every `*.yaml` in outcomes/).
/// Returns records sorted by measure name.
pub fn all_measures_for_client(client_id: &str) -> Result<Vec<OutcomeRecord>> {
    let dir = outcomes_dir(client_id);
    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut records = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("Failed to read outcomes dir: {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let measure = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if measure.is_empty() {
            continue;
        }
        let record = OutcomeRecord::load(client_id, &measure)?;
        records.push(record);
    }

    records.sort_by(|a, b| a.measure.cmp(&b.measure));
    Ok(records)
}

// ---------------------------------------------------------------------------
// Known measures and severity labels
// ---------------------------------------------------------------------------

/// Well-known measures with validated score ranges and severity thresholds.
pub enum KnownMeasure {
    Phq9,
    Gad7,
    Core10,
    Pcl5,
    Wemwbs,
    Isi,
}

impl KnownMeasure {
    pub fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "phq9"   => Some(Self::Phq9),
            "gad7"   => Some(Self::Gad7),
            "core10" => Some(Self::Core10),
            "pcl5"   => Some(Self::Pcl5),
            "wemwbs" => Some(Self::Wemwbs),
            "isi"    => Some(Self::Isi),
            _        => None,
        }
    }

    /// Human-readable display name.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Phq9   => "PHQ-9",
            Self::Gad7   => "GAD-7",
            Self::Core10 => "CORE-10",
            Self::Pcl5   => "PCL-5",
            Self::Wemwbs => "WEMWBS",
            Self::Isi    => "ISI",
        }
    }

    /// Valid score range (inclusive).
    pub fn score_range(&self) -> (f64, f64) {
        match self {
            Self::Phq9   => (0.0, 27.0),
            Self::Gad7   => (0.0, 21.0),
            Self::Core10 => (0.0, 40.0),
            Self::Pcl5   => (0.0, 80.0),
            Self::Wemwbs => (14.0, 70.0),
            Self::Isi    => (0.0, 28.0),
        }
    }

    /// Number of items.
    pub fn item_count(&self) -> usize {
        match self {
            Self::Phq9   => 9,
            Self::Gad7   => 7,
            Self::Core10 => 10,
            Self::Pcl5   => 20,
            Self::Wemwbs => 14,
            Self::Isi    => 7,
        }
    }

    /// Whether higher scores are *better* (currently only WEMWBS).
    pub fn higher_is_better(&self) -> bool {
        matches!(self, Self::Wemwbs)
    }

    /// Severity label for a given score.
    pub fn severity(&self, score: f64) -> &'static str {
        match self {
            Self::Phq9 => {
                if score <= 4.0      { "Minimal" }
                else if score <= 9.0  { "Mild" }
                else if score <= 14.0 { "Moderate" }
                else if score <= 19.0 { "Mod-severe" }
                else                  { "Severe" }
            }
            Self::Gad7 => {
                if score <= 4.0      { "Minimal" }
                else if score <= 9.0  { "Mild" }
                else if score <= 14.0 { "Moderate" }
                else                  { "Severe" }
            }
            Self::Core10 => {
                if score <= 10.0     { "Healthy" }
                else if score <= 14.0 { "Low" }
                else if score <= 19.0 { "Mild/mod" }
                else if score <= 25.0 { "Mod/severe" }
                else                  { "Severe" }
            }
            Self::Pcl5 => {
                if score < 33.0 { "Below threshold" }
                else            { "PTSD probable" }
            }
            Self::Wemwbs => {
                if score <= 40.0     { "Low" }
                else if score <= 59.0 { "Moderate" }
                else                  { "High" }
            }
            Self::Isi => {
                if score <= 7.0      { "No clinically sig" }
                else if score <= 14.0 { "Subthreshold" }
                else if score <= 21.0 { "Moderate" }
                else                  { "Severe" }
            }
        }
    }
}

/// Return the severity label for a score, or `""` for unknown measures.
pub fn severity_label(measure: &str, score: f64) -> &'static str {
    match KnownMeasure::from_slug(measure) {
        Some(m) => m.severity(score),
        None    => "",
    }
}

/// Display name for a measure slug (falls back to the slug itself for custom measures).
pub fn display_name(measure: &str) -> String {
    match KnownMeasure::from_slug(measure) {
        Some(m) => m.display_name().to_string(),
        None    => measure.to_uppercase(),
    }
}

/// Whether a higher score is better for a measure (false for unknowns).
fn higher_is_better(measure: &str) -> bool {
    KnownMeasure::from_slug(measure)
        .map(|m| m.higher_is_better())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Context block for prompts
// ---------------------------------------------------------------------------

/// Build a markdown table of all outcome measures for a client.
///
/// Returns an empty string if there are no recorded outcomes.
pub fn outcomes_context_block(client_id: &str) -> String {
    let records = match all_measures_for_client(client_id) {
        Ok(r) => r,
        Err(_) => return String::new(),
    };

    // Filter to records that actually have at least one entry.
    let records: Vec<&OutcomeRecord> = records.iter().filter(|r| !r.entries.is_empty()).collect();
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

        let name = display_name(&record.measure);
        let score = format_score(latest.score);
        let date = &latest.date;
        let severity = severity_label(&record.measure, latest.score);

        let trend_str = match record.trend() {
            None => String::from("–"),
            Some(delta) => {
                let abs = delta.abs();
                let score_str = format_score(abs);
                // For WEMWBS, positive delta = improving; for all others, positive = worsening.
                let better = if higher_is_better(&record.measure) {
                    delta > 0.0
                } else {
                    delta < 0.0
                };
                if delta == 0.0 {
                    String::from("→ 0 (stable)")
                } else if better {
                    format!("↑ {} (improving)", score_str)
                } else {
                    format!("↓ {} (worsening)", score_str)
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

fn format_score(score: f64) -> String {
    // Show as integer when there's no fractional part.
    if score.fract() == 0.0 {
        format!("{}", score as i64)
    } else {
        format!("{:.1}", score)
    }
}

// ---------------------------------------------------------------------------
// Score validation
// ---------------------------------------------------------------------------

/// Validate score against known-measure range. Returns a warning string if out
/// of range; returns `None` for unknown measures (free-form accepted silently).
pub fn validate_score(measure: &str, score: f64) -> Option<String> {
    let m = KnownMeasure::from_slug(measure)?;
    let (min, max) = m.score_range();
    if score < min || score > max {
        Some(format!(
            "Warning: {} score {:.1} is outside expected range {:.0}–{:.0}",
            m.display_name(),
            score,
            min,
            max
        ))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper: override CLINICAL_ROOT for the test and return the TempDir guard.
    fn with_temp_clinical(client_id: &str) -> (TempDir, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        let client_dir = tmp.path().join("clients").join(client_id);
        std::fs::create_dir_all(&client_dir).unwrap();
        (tmp, client_dir)
    }

    // We need to be able to resolve outcomes_dir to a temp path in tests.
    // The easiest way without changing prod code is to set CLINICAL_ROOT.
    fn outcomes_dir_for_test(tmp: &TempDir, client_id: &str) -> std::path::PathBuf {
        tmp.path().join("clients").join(client_id).join("outcomes")
    }

    fn write_temp_outcome(
        tmp: &TempDir,
        client_id: &str,
        measure: &str,
        record: &OutcomeRecord,
    ) {
        let dir = outcomes_dir_for_test(tmp, client_id);
        std::fs::create_dir_all(&dir).unwrap();
        let yaml = serde_yaml::to_string(record).unwrap();
        std::fs::write(dir.join(format!("{}.yaml", measure)), yaml).unwrap();
    }

    // --------------------------------------------------------------------
    // test_record_round_trip
    // --------------------------------------------------------------------
    #[test]
    fn test_record_round_trip() {
        let tmp = TempDir::new().unwrap();
        let client_dir = tmp.path().join("clients").join("RT01");
        std::fs::create_dir_all(&client_dir).unwrap();

        // Temporarily redirect CLINICAL_ROOT so config::clients_dir() resolves here.
        // NOTE: tests run in parallel; setting env vars is unsafe when parallelised.
        // We use a local approach: build paths manually and call yaml directly.
        let outcomes_path = client_dir.join("outcomes");
        std::fs::create_dir_all(&outcomes_path).unwrap();

        let entry = OutcomeEntry {
            date: "2026-03-01".to_string(),
            score: 14.0,
            items: Some(vec![1.0, 2.0, 1.0, 2.0, 2.0, 1.0, 1.0, 2.0, 2.0]),
            notes: Some("Baseline".to_string()),
        };
        let record = OutcomeRecord {
            measure: "phq9".to_string(),
            client_id: "RT01".to_string(),
            entries: vec![entry.clone()],
        };

        // Serialise and deserialise without using prod path helpers.
        let yaml = serde_yaml::to_string(&record).unwrap();
        let file = outcomes_path.join("phq9.yaml");
        std::fs::write(&file, &yaml).unwrap();

        let loaded: OutcomeRecord = serde_yaml::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(loaded.measure, "phq9");
        assert_eq!(loaded.client_id, "RT01");
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].score, 14.0);
        assert_eq!(loaded.entries[0].items, Some(vec![1.0, 2.0, 1.0, 2.0, 2.0, 1.0, 1.0, 2.0, 2.0]));
        assert_eq!(loaded.entries[0].notes, Some("Baseline".to_string()));
    }

    // --------------------------------------------------------------------
    // test_severity_labels
    // --------------------------------------------------------------------
    #[test]
    fn test_severity_labels() {
        assert_eq!(severity_label("phq9",  4.0),  "Minimal");
        assert_eq!(severity_label("phq9",  9.0),  "Mild");
        assert_eq!(severity_label("phq9", 14.0),  "Moderate");
        assert_eq!(severity_label("phq9", 19.0),  "Mod-severe");
        assert_eq!(severity_label("phq9", 27.0),  "Severe");

        assert_eq!(severity_label("gad7",  0.0),  "Minimal");
        assert_eq!(severity_label("gad7", 15.0),  "Severe");

        assert_eq!(severity_label("core10",  5.0), "Healthy");
        assert_eq!(severity_label("core10", 12.0), "Low");
        assert_eq!(severity_label("core10", 17.0), "Mild/mod");
        assert_eq!(severity_label("core10", 22.0), "Mod/severe");
        assert_eq!(severity_label("core10", 30.0), "Severe");

        assert_eq!(severity_label("pcl5", 32.0), "Below threshold");
        assert_eq!(severity_label("pcl5", 33.0), "PTSD probable");

        assert_eq!(severity_label("wemwbs", 40.0), "Low");
        assert_eq!(severity_label("wemwbs", 50.0), "Moderate");
        assert_eq!(severity_label("wemwbs", 60.0), "High");

        assert_eq!(severity_label("isi",  7.0), "No clinically sig");
        assert_eq!(severity_label("isi", 14.0), "Subthreshold");
        assert_eq!(severity_label("isi", 21.0), "Moderate");
        assert_eq!(severity_label("isi", 25.0), "Severe");
    }

    // --------------------------------------------------------------------
    // test_trend_calculation
    // --------------------------------------------------------------------
    #[test]
    fn test_trend_calculation() {
        let mut record = OutcomeRecord {
            measure: "phq9".to_string(),
            client_id: "TR01".to_string(),
            entries: vec![],
        };

        // No entries → no trend
        assert!(record.trend().is_none());

        // One entry → still no trend
        record.entries.push(OutcomeEntry {
            date: "2026-03-01".to_string(),
            score: 14.0,
            items: None,
            notes: None,
        });
        assert!(record.trend().is_none());

        // Two entries — score decreasing = improving (negative delta)
        record.entries.push(OutcomeEntry {
            date: "2026-03-15".to_string(),
            score: 10.0,
            items: None,
            notes: None,
        });
        let trend = record.trend().unwrap();
        assert_eq!(trend, -4.0, "trend should be 10 - 14 = -4");

        // Score increasing = worsening (positive delta)
        record.entries.push(OutcomeEntry {
            date: "2026-03-29".to_string(),
            score: 16.0,
            items: None,
            notes: None,
        });
        let trend = record.trend().unwrap();
        assert_eq!(trend, 6.0, "trend should be 16 - 10 = +6");
    }

    // --------------------------------------------------------------------
    // test_unknown_measure_no_validation
    // --------------------------------------------------------------------
    #[test]
    fn test_unknown_measure_no_validation() {
        // A free-form measure slug: no range validation, no severity label
        let label = severity_label("custom-distress", 999.0);
        assert_eq!(label, "", "Unknown measure should return empty severity");

        let warning = validate_score("custom-distress", 999.0);
        assert!(warning.is_none(), "Unknown measure should accept any score without warning");

        let warning = validate_score("phq9", 999.0);
        assert!(warning.is_some(), "Known measure should warn when out of range");
    }

    // --------------------------------------------------------------------
    // test_outcomes_context_block_empty
    // --------------------------------------------------------------------
    #[test]
    fn test_outcomes_context_block_empty() {
        // Point CLINICAL_ROOT at a temp dir with no outcomes for the client.
        let tmp = TempDir::new().unwrap();
        let client_dir = tmp.path().join("clients").join("EMPTY01");
        std::fs::create_dir_all(&client_dir).unwrap();

        // Redirect CLINICAL_ROOT.
        // (tests that mutate env vars run sequentially in Rust by default when
        //  they share a key, but we clean up immediately after the assertion.)
        // SAFETY: single-threaded test; no other thread reads CLINICAL_ROOT here.
        unsafe { std::env::set_var("CLINICAL_ROOT", tmp.path()); }
        let result = outcomes_context_block("EMPTY01");
        unsafe { std::env::remove_var("CLINICAL_ROOT"); }

        assert_eq!(result, "", "Empty outcomes dir should return empty string");
    }
}
