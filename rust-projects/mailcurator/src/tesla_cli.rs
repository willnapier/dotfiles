//! `mailcurator tesla` — query interface for Tesla email records.
//!
//! Tesla's data is heterogeneous: auth codes, service appointments,
//! supercharger receipts, software releases, marketing. The `kind` field
//! (set by the deterministic classifier) is the primary index — most
//! useful CLI verbs are kind-filtered (`mailcurator tesla service`,
//! `mailcurator tesla supercharger`, etc.).

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Utc};
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};

use crate::store;

#[derive(Debug, Clone)]
struct TeslaRow {
    received: Option<DateTime<Utc>>,
    subject: String,
    kind: Option<String>,
    amount: Option<String>,
    service_date: Option<String>,
    location: Option<String>,
    kwh: Option<String>,
}

fn load_tesla() -> Result<Vec<TeslaRow>> {
    let path = store::category_path("tesla")?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let f = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let mut out = Vec::new();
    for line in BufReader::new(f).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(obj) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let received = obj
            .get("received")
            .and_then(|v| v.as_str())
            .and_then(parse_rfc2822);
        let subject = obj
            .get("subject")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        out.push(TeslaRow {
            received,
            subject,
            kind: opt_string(&obj, "kind"),
            amount: opt_string(&obj, "amount"),
            service_date: opt_string(&obj, "service_date"),
            location: opt_string(&obj, "location"),
            kwh: opt_string(&obj, "kwh"),
        });
    }
    out.sort_by(|a, b| b.received.cmp(&a.received));
    Ok(out)
}

fn opt_string(obj: &Value, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

fn parse_rfc2822(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

pub fn list(year: Option<i32>, kind: Option<&str>, limit: usize) -> Result<()> {
    let all = load_tesla()?;
    let filtered: Vec<&TeslaRow> = all
        .iter()
        .filter(|r| match (year, r.received) {
            (Some(y), Some(d)) => d.year() == y,
            (Some(_), None) => false,
            (None, _) => true,
        })
        .filter(|r| match kind {
            Some(k) => r.kind.as_deref() == Some(k),
            None => true,
        })
        .take(limit)
        .collect();
    print_rows(&filtered, all.len());
    Ok(())
}

pub fn summary() -> Result<()> {
    use std::collections::BTreeMap;
    let all = load_tesla()?;
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    for r in &all {
        let k = r.kind.clone().unwrap_or_else(|| "(unknown)".into());
        *by_kind.entry(k).or_insert(0) += 1;
    }
    println!("Tesla emails by kind ({} total):", all.len());
    for (k, n) in &by_kind {
        println!("  {k:<15} {n:>4}");
    }
    println!();
    let total_paid: f64 = all
        .iter()
        .filter_map(|r| r.amount.as_deref())
        .filter_map(|s| s.parse::<f64>().ok())
        .sum();
    let with_amount = all.iter().filter(|r| r.amount.is_some()).count();
    println!("Total amount captured: £{total_paid:.2} across {with_amount} records");
    Ok(())
}

pub fn find(query: &str, limit: usize) -> Result<()> {
    let q = query.to_lowercase();
    let all = load_tesla()?;
    let filtered: Vec<&TeslaRow> = all
        .iter()
        .filter(|r| {
            r.subject.to_lowercase().contains(&q)
                || r.location.as_deref().is_some_and(|s| s.to_lowercase().contains(&q))
                || r.kind.as_deref().is_some_and(|s| s.to_lowercase() == q)
        })
        .take(limit)
        .collect();
    print_rows(&filtered, all.len());
    Ok(())
}

fn print_rows(items: &[&TeslaRow], total_in_store: usize) {
    if items.is_empty() {
        println!("no tesla rows matching filter (store has {total_in_store} total)");
        return;
    }
    println!(
        "{:<12}  {:<13}  {:>7}  {:<20}  {}",
        "received", "kind", "amount £", "location", "subject"
    );
    println!("{}", "─".repeat(90));
    for r in items {
        let recv = r
            .received
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "?".to_string());
        let kind = r.kind.as_deref().unwrap_or("—");
        let amount = r.amount.as_deref().unwrap_or("—");
        let location = r.location.as_deref().unwrap_or("—");
        let subj = truncate(&r.subject, 35);
        println!(
            "{recv:<12}  {kind:<13}  {amount:>7}  {:<20}  {subj}",
            truncate(location, 20)
        );
    }
    println!();
    println!("{} matching / {} in store", items.len(), total_in_store);
}

fn truncate(s: &str, n: usize) -> String {
    let count = s.chars().count();
    if count <= n {
        s.to_string()
    } else {
        let head: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{head}…")
    }
}
