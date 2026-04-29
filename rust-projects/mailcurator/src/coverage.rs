//! Coverage reporting for extractor field-population rates.
//!
//! For each policy with extractors, walk the corresponding jsonl file and
//! report how often each field is populated. When a vendor module is set,
//! highlight `required_fields` separately — those drive the health score
//! and feed drift detection (Session 3).
//!
//! Usage: `mailcurator coverage` (all policies) or `mailcurator coverage
//! --policy amazon-orders` (one policy).

use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};

use crate::extractors;
use crate::policy::Policy;
use crate::store;

/// Per-policy coverage report.
pub struct Report {
    pub policy_name: String,
    pub category: String,
    pub records: usize,
    /// field name → count of records where the field is populated (non-empty)
    pub field_population: BTreeMap<String, usize>,
    /// Required fields per the vendor module, if any. Empty otherwise.
    pub required_fields: Vec<String>,
    /// Records where ALL required_fields are populated.
    pub fully_covered: usize,
}

impl Report {
    pub fn coverage_pct(&self, field: &str) -> f64 {
        if self.records == 0 {
            return 0.0;
        }
        100.0 * (*self.field_population.get(field).unwrap_or(&0) as f64) / (self.records as f64)
    }

    pub fn health_pct(&self) -> Option<f64> {
        if self.required_fields.is_empty() || self.records == 0 {
            return None;
        }
        Some(100.0 * (self.fully_covered as f64) / (self.records as f64))
    }
}

pub fn report_all(policies: &[Policy], policy_filter: Option<&str>) -> Result<Vec<Report>> {
    let mut reports = Vec::new();
    for pol in policies {
        if pol.extractors.is_empty() {
            continue;
        }
        if let Some(name) = policy_filter {
            if pol.name != name {
                continue;
            }
        }
        for ex in &pol.extractors {
            let report = build_report(pol, &ex.category)?;
            reports.push(report);
        }
    }
    Ok(reports)
}

fn build_report(pol: &Policy, category: &str) -> Result<Report> {
    let path = store::category_path(category)?;
    let required_fields: Vec<String> = pol
        .vendor_module
        .as_deref()
        .and_then(extractors::dispatch)
        .map(|m| m.required_fields().iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();

    let mut field_population: BTreeMap<String, usize> = BTreeMap::new();
    let mut records = 0usize;
    let mut fully_covered = 0usize;

    if path.exists() {
        let f = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
        for line in BufReader::new(f).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let Ok(obj) = serde_json::from_str::<Value>(&line) else {
                continue; // tolerate corrupt lines
            };
            // Only count rows attributed to THIS policy. The same jsonl may
            // be shared by multiple policies (e.g. deliveries.jsonl from
            // royal-mail + amazon-shipping). Filter on the `policy` field;
            // tolerate older rows without it (count them in for category).
            if let Some(p) = obj.get("policy").and_then(|v| v.as_str()) {
                if p != pol.name {
                    continue;
                }
            }
            records += 1;
            let mut all_required = !required_fields.is_empty();
            if let Some(map) = obj.as_object() {
                for (k, v) in map {
                    if is_populated(v) {
                        *field_population.entry(k.clone()).or_insert(0) += 1;
                    }
                }
                if all_required {
                    for r in &required_fields {
                        if !map.get(r).map(is_populated).unwrap_or(false) {
                            all_required = false;
                            break;
                        }
                    }
                    if all_required {
                        fully_covered += 1;
                    }
                }
            }
        }
    }

    Ok(Report {
        policy_name: pol.name.clone(),
        category: category.to_string(),
        records,
        field_population,
        required_fields,
        fully_covered,
    })
}

fn is_populated(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        _ => true,
    }
}

/// Bookkeeping fields written by the framework, not by extractor logic. We
/// don't want them dominating the field-coverage display.
const BOOKKEEPING_FIELDS: &[&str] =
    &["extracted_at", "message_id", "policy", "ts", "source"];

pub fn print_reports(reports: &[Report]) {
    if reports.is_empty() {
        println!("no policies with extractors found");
        return;
    }
    for r in reports {
        let header = match r.health_pct() {
            Some(h) => format!(
                "=== {} (jsonl: {}, records: {}, health: {:.1}% required-coverage) ===",
                r.policy_name, r.category, r.records, h
            ),
            None => format!(
                "=== {} (jsonl: {}, records: {}, no vendor module) ===",
                r.policy_name, r.category, r.records
            ),
        };
        println!("{header}");
        if r.records == 0 {
            println!("  (no records yet — run `mailcurator run` first)");
            println!();
            continue;
        }
        // Show required fields first, marked.
        let mut shown = std::collections::HashSet::new();
        for field in &r.required_fields {
            shown.insert(field.clone());
            println!(
                "  ★ {:<20} {:>4}/{:<4}  {:>5.1}%",
                field,
                r.field_population.get(field).copied().unwrap_or(0),
                r.records,
                r.coverage_pct(field)
            );
        }
        // Then other fields, skipping bookkeeping.
        let mut other: Vec<(&String, &usize)> = r
            .field_population
            .iter()
            .filter(|(k, _)| !shown.contains(*k) && !BOOKKEEPING_FIELDS.contains(&k.as_str()))
            .collect();
        other.sort_by(|a, b| b.1.cmp(a.1));
        for (k, _) in other {
            println!(
                "    {:<20} {:>4}/{:<4}  {:>5.1}%",
                k,
                r.field_population.get(k).copied().unwrap_or(0),
                r.records,
                r.coverage_pct(k)
            );
        }
        println!();
    }
}
