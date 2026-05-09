//! Coverage reporting for extractor field-population rates.
//!
//! For each policy with extractors, walk the corresponding jsonl file and
//! report how often each field is populated. When a vendor module is set,
//! highlight `required_fields` separately — those drive the health score
//! and feed drift detection (Session 3).
//!
//! Usage: `mailcurator coverage` (all policies) or `mailcurator coverage
//! --policy amazon-orders` (one policy).

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

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
    /// Vendor module name (if any), separate from required_fields so the
    /// "no vendor module" header can be distinguished from "vendor module
    /// with empty required_fields" (e.g. tesla).
    pub vendor_module: Option<String>,
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
    let required_fields: Vec<String> = pol
        .vendor_module
        .as_deref()
        .and_then(extractors::dispatch)
        .map(|m| m.required_fields().iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();

    let mut field_population: BTreeMap<String, usize> = BTreeMap::new();
    let mut records = 0usize;
    let mut fully_covered = 0usize;

    let lines = store::read_category_lines(category)?;
    for line in lines {
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

    Ok(Report {
        policy_name: pol.name.clone(),
        category: category.to_string(),
        records,
        field_population,
        required_fields,
        fully_covered,
        vendor_module: pol.vendor_module.clone(),
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

/// One row in coverage-history.jsonl. Captured once per `coverage` run so
/// `coverage --drift` can compare current rates to the most recent prior
/// snapshot and flag vendor template-change regressions.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Snapshot {
    pub ts: String, // RFC3339
    pub policy: String,
    pub category: String,
    pub records: usize,
    pub fully_covered: usize,
    pub health_pct: Option<f64>,
    pub field_pct: BTreeMap<String, f64>,
}

impl Snapshot {
    fn from_report(r: &Report) -> Self {
        let mut field_pct = BTreeMap::new();
        for (k, v) in &r.field_population {
            if r.records > 0 {
                field_pct.insert(k.clone(), 100.0 * (*v as f64) / (r.records as f64));
            }
        }
        Self {
            ts: Utc::now().to_rfc3339(),
            policy: r.policy_name.clone(),
            category: r.category.clone(),
            records: r.records,
            fully_covered: r.fully_covered,
            health_pct: r.health_pct(),
            field_pct,
        }
    }
}

/// Append one row per report to this machine's coverage-history file
/// (`coverage-history.<hostname>.jsonl`). Reads in `read_history` union
/// across all per-host files when comparing for drift.
pub fn snapshot(reports: &[Report]) -> Result<()> {
    for r in reports {
        let s = Snapshot::from_report(r);
        store::append_record("coverage-history", &s)?;
    }
    Ok(())
}

/// Read the entire snapshot history across every per-host file.
fn read_history() -> Result<Vec<Snapshot>> {
    let lines = store::read_category_lines("coverage-history")?;
    let mut out = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(s) = serde_json::from_str::<Snapshot>(&line) {
            out.push(s);
        }
    }
    Ok(out)
}

/// Compare current reports against the most recent prior snapshot per
/// policy (last snapshot strictly older than the current run's timestamp).
/// Reports policies whose `health_pct` dropped by `threshold_pp` percentage
/// points or more, or where any required field's coverage dropped by that
/// margin. Exit code intent: caller propagates 1 if any drift detected, 0
/// otherwise — useful for cron + alerting.
pub fn drift(reports: &[Report], threshold_pp: f64) -> Result<DriftReport> {
    let history = read_history()?;
    let now = Utc::now();
    let mut findings = Vec::new();

    for current_report in reports {
        let current = Snapshot::from_report(current_report);
        // Find latest historical snapshot for this policy that's strictly older.
        let prior = history
            .iter()
            .filter(|s| s.policy == current.policy)
            .filter(|s| {
                DateTime::parse_from_rfc3339(&s.ts)
                    .ok()
                    .map(|t| t.with_timezone(&Utc) < now)
                    .unwrap_or(false)
            })
            .max_by_key(|s| s.ts.clone()); // RFC3339 sort is lexicographic = chronological

        let Some(prior) = prior else {
            continue; // first snapshot for this policy
        };

        // Health drop?
        if let (Some(h_now), Some(h_prev)) = (current.health_pct, prior.health_pct) {
            let drop = h_prev - h_now;
            if drop >= threshold_pp {
                findings.push(Finding {
                    policy: current.policy.clone(),
                    field: "<health>".into(),
                    prev: h_prev,
                    current: h_now,
                    drop,
                    prev_ts: prior.ts.clone(),
                });
            }
        }

        // Per-field drops on required fields.
        for f in &current_report.required_fields {
            let now_v = current.field_pct.get(f).copied().unwrap_or(0.0);
            let prev_v = prior.field_pct.get(f).copied();
            if let Some(prev_v) = prev_v {
                let drop = prev_v - now_v;
                if drop >= threshold_pp {
                    findings.push(Finding {
                        policy: current.policy.clone(),
                        field: f.clone(),
                        prev: prev_v,
                        current: now_v,
                        drop,
                        prev_ts: prior.ts.clone(),
                    });
                }
            }
        }
    }

    Ok(DriftReport { findings })
}

#[derive(Debug)]
pub struct DriftReport {
    pub findings: Vec<Finding>,
}

#[derive(Debug)]
pub struct Finding {
    pub policy: String,
    pub field: String,
    pub prev: f64,
    pub current: f64,
    pub drop: f64,
    pub prev_ts: String,
}

pub fn print_drift(d: &DriftReport) {
    if d.findings.is_empty() {
        println!("no drift detected");
        return;
    }
    println!("DRIFT DETECTED in {} field/policy combination(s):", d.findings.len());
    println!();
    println!(
        "{:<25}  {:<15}  {:>9}  {:>9}  {:>6}  {}",
        "policy", "field", "previous", "current", "drop", "prev snapshot"
    );
    for f in &d.findings {
        println!(
            "{:<25}  {:<15}  {:>8.1}%  {:>8.1}%  {:>5.1}%  {}",
            f.policy, f.field, f.prev, f.current, f.drop, f.prev_ts
        );
    }
    println!();
    println!("Likely causes:");
    println!("  - Vendor changed their email template (CSS classes, table structure)");
    println!("  - New email type from this sender that the extractor doesn't yet handle");
    println!("Next: `mailcurator preview <policy>` to sample messages for the failing field.");
}

pub fn print_reports(reports: &[Report]) {
    if reports.is_empty() {
        println!("no policies with extractors found");
        return;
    }
    for r in reports {
        // Distinguish "vendor module declared no required_fields" from
        // "module IS declared but records=0 so health is undefined yet".
        let suffix = match &r.vendor_module {
            None => "no vendor module".to_string(),
            Some(m) if r.required_fields.is_empty() => {
                format!("vendor module: {m} (no required_fields declared)")
            }
            Some(m) if r.records == 0 => {
                format!("vendor module: {m} (no records yet — run `mailcurator run` first)")
            }
            Some(_) => {
                let h = r.health_pct().unwrap_or(0.0);
                format!("health: {h:.1}% required-coverage")
            }
        };
        let header = format!(
            "=== {} (jsonl: {}, records: {}, {}) ===",
            r.policy_name, r.category, r.records, suffix
        );
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
