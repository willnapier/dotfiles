//! Read-only local evidence projection. Never invokes notmuch, an LLM, or a writer.
//! Versioned JSON is consumed by MailForge; raw messages/subjects are not returned.
use crate::policy::Policy;
use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader},
    path::Path,
};

#[derive(Serialize)]
pub struct EvidenceReport {
    pub version: u32,
    pub total: usize,
    pub exceptions: usize,
    pub malformed: usize,
    pub duplicates: usize,
    pub filtered_total: usize,
    pub offset: usize,
    pub latest_write: Option<String>,
    pub rows: Vec<EvidenceRow>,
    pub health: Vec<Health>,
}

#[derive(Serialize)]
pub struct EvidenceRow {
    pub vendor: String,
    pub amount: String,
    pub currency: String,
    pub date: String,
    pub policy: String,
    pub account: String,
    pub message_id: String,
    pub issues: Vec<String>,
}

#[derive(Serialize)]
pub struct Health {
    pub policy: String,
    pub records: usize,
    pub complete: usize,
}

fn string(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn stamp(v: &Value) -> Option<DateTime<chrono::FixedOffset>> {
    DateTime::parse_from_rfc3339(&string(v, "extracted_at")).ok()
}

fn populated(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::String(s) => !s.trim().is_empty(),
        _ => true,
    }
}

fn date(v: &Value) -> String {
    for key in ["payment_date", "statement_date", "received", "due_date"] {
        let s = string(v, key);
        if let Ok(d) = NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
            return d.to_string();
        }
        if let Ok(d) = DateTime::parse_from_rfc2822(&s) {
            return d.date_naive().to_string();
        }
        if let Ok(d) = DateTime::parse_from_rfc3339(&s) {
            return d.date_naive().to_string();
        }
    }
    String::new()
}

fn row(v: &Value) -> EvidenceRow {
    let vendor = string(v, "vendor");
    let vendor = if vendor.eq_ignore_ascii_case("paypal") {
        string(v, "counterparty")
    } else {
        vendor
    };
    let amount = ["amount_gbp", "amount"]
        .into_iter()
        .map(|k| string(v, k))
        .find(|s| !s.is_empty())
        .unwrap_or_default();
    let currency = if !string(v, "amount_gbp").is_empty() {
        "GBP".into()
    } else {
        string(v, "currency")
    };
    let date = date(v);
    let account = string(v, "account");
    let message_id = string(v, "message_id");
    let mut issues = Vec::new();
    if vendor.is_empty() {
        issues.push("Missing supplier".into());
    }
    // Do not manufacture a numeric amount from a malformed vendor capture.
    let numeric = amount.replace([',', '£', '$', '€'], "");
    if numeric
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
        .is_none()
    {
        issues.push("Missing or invalid amount".into());
    }
    if currency.is_empty() {
        issues.push("Currency unconfirmed".into());
    }
    if date.is_empty() {
        issues.push("Missing or invalid date".into());
    }
    if message_id.is_empty() || !matches!(account.as_str(), "personal" | "cohs") {
        issues.push("Source account unverified (legacy record)".into());
    }
    if v.get("_provenance")
        .and_then(Value::as_object)
        .is_some_and(|m| m.values().any(|s| s == "llm"))
    {
        issues.push("LLM-derived fields: verify against source".into());
    }
    EvidenceRow {
        vendor,
        amount,
        currency,
        date,
        policy: string(v, "policy"),
        account,
        message_id,
        issues,
    }
}

/// Reads only known ledger basenames; no arbitrary path is accepted by the UI.
/// Directory errors are not silently reported as an empty/healthy ledger.
pub fn load(
    dir: &Path,
    policies: &[Policy],
    offset: usize,
    limit: usize,
    exceptions_only: bool,
) -> Result<EvidenceReport> {
    let mut categories = vec!["bills".to_string()];
    categories.extend(
        policies
            .iter()
            .filter(|p| p.vendor_module.is_some())
            .flat_map(|p| p.extractors.iter().map(|e| e.category.clone())),
    );
    categories.sort();
    categories.dedup();
    let mut report = EvidenceReport {
        version: 1,
        total: 0,
        exceptions: 0,
        malformed: 0,
        duplicates: 0,
        filtered_total: 0,
        offset,
        latest_write: None,
        rows: Vec::new(),
        health: Vec::new(),
    };
    let mut paths = fs::read_dir(dir)
        .context("evidence directory unavailable")?
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort_by_key(|e| e.file_name());
    let mut entries: BTreeMap<(String, String, String, String), Value> = BTreeMap::new();
    let mut serial = 0usize;
    let mut bytes = 0u64;
    for entry in paths {
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(category) = categories.iter().find(|c| {
            name == format!("{c}.jsonl")
                || name
                    .strip_prefix(&format!("{c}."))
                    .and_then(|s| s.strip_suffix(".jsonl"))
                    .is_some_and(|s| !s.is_empty())
        }) else {
            continue;
        };
        let meta = entry.metadata()?;
        bytes += meta.len();
        anyhow::ensure!(
            bytes <= 200_000_000,
            "evidence ledger too large; compact before viewing"
        );
        if category == "bills" {
            if let Ok(time) = meta.modified() {
                let time = DateTime::<Utc>::from(time).to_rfc3339();
                if report.latest_write.as_ref().is_none_or(|old| &time > old) {
                    report.latest_write = Some(time);
                }
            }
        }
        for line in BufReader::new(fs::File::open(entry.path())?).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let v: Value = match serde_json::from_str(&line) {
                Ok(v @ Value::Object(_)) => v,
                _ => {
                    report.malformed += 1;
                    continue;
                }
            };
            serial += 1;
            let id = string(&v, "message_id");
            let id = if id.is_empty() {
                format!("missing-{serial}")
            } else {
                id
            };
            let key = (
                category.clone(),
                string(&v, "account"),
                string(&v, "policy"),
                id,
            );
            if let Some(old) = entries.get(&key) {
                report.duplicates += 1;
                let rank = |v: &Value| {
                    (
                        stamp(v),
                        v.as_object()
                            .unwrap()
                            .values()
                            .filter(|v| populated(v))
                            .count(),
                    )
                };
                if rank(&v) < rank(old) {
                    continue;
                }
            }
            entries.insert(key, v);
        }
    }
    for pol in policies {
        let Some(module) = pol
            .vendor_module
            .as_deref()
            .and_then(crate::extractors::dispatch)
        else {
            continue;
        };
        let required = module.required_fields();
        if required.is_empty() {
            continue;
        }
        let records: Vec<_> = entries
            .iter()
            .filter(|((_, _, p, _), _)| p == &pol.name)
            .map(|(_, v)| v)
            .collect();
        report.health.push(Health {
            policy: pol.name.clone(),
            records: records.len(),
            complete: records
                .iter()
                .filter(|v| required.iter().all(|k| v.get(*k).is_some_and(populated)))
                .count(),
        });
    }
    let mut rows: Vec<_> = entries
        .iter()
        .filter(|((c, _, _, _), _)| c == "bills")
        .map(|(_, v)| row(v))
        .collect();
    rows.sort_by(|a, b| {
        b.date
            .cmp(&a.date)
            .then_with(|| a.vendor.cmp(&b.vendor))
            .then_with(|| a.message_id.cmp(&b.message_id))
            .then_with(|| a.policy.cmp(&b.policy))
            .then_with(|| a.account.cmp(&b.account))
    });
    report.total = rows.len();
    report.exceptions = rows.iter().filter(|r| !r.issues.is_empty()).count();
    if exceptions_only {
        rows.retain(|r| !r.issues.is_empty());
    }
    report.filtered_total = rows.len();
    report.rows = rows.into_iter().skip(offset).take(limit.min(200)).collect();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn read_only_union_and_exception_controls() {
        let dir = tempfile::tempdir().unwrap();
        let good = serde_json::json!({"message_id":"fixture@example.org", "account":"personal", "policy":"fixture", "vendor":"Fixture", "amount":"12.34", "currency":"GBP", "received":"2026-09-19", "extracted_at":"2026-09-19T00:00:00Z"});
        let mut old = good.clone();
        old["amount"] = Value::String("1.00".into());
        old["extracted_at"] = Value::String("2026-01-01T00:00:00Z".into());
        fs::write(dir.path().join("bills.jsonl"), format!("{old}\nnot json\n")).unwrap();
        fs::write(
            dir.path().join("bills.host.jsonl"),
            format!("{good}\n{{\"message_id\":\"incomplete\",\"amount\":\"NaN\"}}\n"),
        )
        .unwrap();
        fs::write(dir.path().join("bills..jsonl"), "{}\n").unwrap();
        let r = load(dir.path(), &[], 0, 50, false).unwrap();
        assert_eq!(
            (r.total, r.exceptions, r.malformed, r.duplicates),
            (2, 1, 1, 1)
        );
        assert_eq!(r.rows[0].amount, "12.34");
        assert!(r.rows[0].issues.is_empty());
        assert!(
            r.rows[1]
                .issues
                .iter()
                .any(|s| s.contains("invalid amount"))
        );
        let r = load(dir.path(), &[], 0, 1, true).unwrap();
        assert_eq!(r.filtered_total, 1);
        assert_eq!(r.rows.len(), 1);
        assert!(
            load(dir.path(), &[], 99, 50, false)
                .unwrap()
                .rows
                .is_empty()
        );
        assert!(load(&dir.path().join("missing"), &[], 0, 50, false).is_err());
        assert_eq!(
            fs::read_to_string(dir.path().join("bills.jsonl")).unwrap(),
            format!("{old}\nnot json\n")
        );
    }
    #[test]
    fn no_records_is_unknown_not_healthy() {
        let dir = tempfile::tempdir().unwrap();
        let policies: crate::config::Config = toml::from_str("[[policy]]\nname='fixture'\nfrom='vendor@example.org'\nvendor_module='amazon_orders'\n[[policy.extractor]]\ncategory='orders'\n").unwrap();
        let r = load(dir.path(), &policies.policies, 0, 50, false).unwrap();
        assert_eq!(r.health.len(), 1);
        assert_eq!(r.health[0].records, 0);
        assert_eq!(r.total, 0);
    }
}
