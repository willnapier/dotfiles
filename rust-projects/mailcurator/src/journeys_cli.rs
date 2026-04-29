//! `mailcurator journeys` — query interface for trainline journey records.
//!
//! Sister of `orders_cli`. The jsonl is the authoritative store for
//! journey history; this CLI makes it queryable so you reach for it
//! before grepping email.

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Utc};
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};

use crate::store;

#[derive(Debug, Clone)]
struct Journey {
    received: Option<DateTime<Utc>>,
    subject: String,
    destination: Option<String>,
    journey_date: Option<String>,
    journey_time: Option<String>,
    fare: Option<String>,
    booking_ref: Option<String>,
}

fn load_journeys() -> Result<Vec<Journey>> {
    let path = store::category_path("journeys")?;
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
        out.push(Journey {
            received,
            subject,
            destination: opt_string(&obj, "destination"),
            journey_date: opt_string(&obj, "journey_date"),
            journey_time: opt_string(&obj, "journey_time"),
            fare: opt_string(&obj, "fare"),
            booking_ref: opt_string(&obj, "booking_ref"),
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

pub fn list(year: Option<i32>, limit: usize) -> Result<()> {
    let all = load_journeys()?;
    let filtered: Vec<&Journey> = all
        .iter()
        .filter(|j| match (year, j.received) {
            (Some(y), Some(d)) => d.year() == y,
            (Some(_), None) => false,
            (None, _) => true,
        })
        .take(limit)
        .collect();
    print_journeys(&filtered, all.len());
    Ok(())
}

pub fn find(query: &str, limit: usize) -> Result<()> {
    let q = query.to_lowercase();
    let all = load_journeys()?;
    let filtered: Vec<&Journey> = all
        .iter()
        .filter(|j| {
            j.subject.to_lowercase().contains(&q)
                || j.destination.as_deref().is_some_and(|s| s.to_lowercase().contains(&q))
                || j.journey_date.as_deref().is_some_and(|s| s.to_lowercase().contains(&q))
        })
        .take(limit)
        .collect();
    print_journeys(&filtered, all.len());
    Ok(())
}

pub fn recent(days: i64, limit: usize) -> Result<()> {
    let cutoff = Utc::now() - chrono::Duration::days(days);
    let all = load_journeys()?;
    let filtered: Vec<&Journey> = all
        .iter()
        .filter(|j| j.received.map(|d| d > cutoff).unwrap_or(false))
        .take(limit)
        .collect();
    print_journeys(&filtered, all.len());
    Ok(())
}

pub fn total(year: Option<i32>) -> Result<()> {
    let all = load_journeys()?;
    let mut sum = 0.0_f64;
    let mut counted = 0usize;
    let mut missing_fare = 0usize;
    for j in &all {
        if let Some(y) = year {
            match j.received {
                Some(d) if d.year() == y => {}
                _ => continue,
            }
        }
        match j.fare.as_deref().and_then(|t| t.parse::<f64>().ok()) {
            Some(v) => {
                sum += v;
                counted += 1;
            }
            None => missing_fare += 1,
        }
    }
    let label = year.map(|y| format!("year {y}")).unwrap_or_else(|| "all years".to_string());
    println!("journeys {label}: counted={counted}  missing-fare={missing_fare}");
    println!("sum (£): {sum:.2}");
    Ok(())
}

fn print_journeys(items: &[&Journey], total_in_store: usize) {
    if items.is_empty() {
        println!("no journeys matching filter (store has {total_in_store} total)");
        return;
    }
    println!(
        "{:<10}  {:<22}  {:>5}  {:>6}  {}",
        "received", "destination", "time", "fare £", "subject"
    );
    println!("{}", "─".repeat(90));
    for j in items {
        let date_s = j
            .received
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "?".to_string());
        let dest = j.destination.as_deref().unwrap_or("—");
        let time = j.journey_time.as_deref().unwrap_or("—");
        let fare = j.fare.as_deref().unwrap_or("—");
        let subj = truncate(&j.subject, 40);
        println!("{date_s:<10}  {:<22}  {time:>5}  {fare:>6}  {subj}", truncate(dest, 22));
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
