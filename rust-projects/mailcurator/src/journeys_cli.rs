//! `mailcurator journeys` — query interface for trainline journey records.
//!
//! Sister of `orders_cli`. The jsonl is the authoritative store for
//! journey history; this CLI makes it queryable so you reach for it
//! before grepping email.

use anyhow::Result;
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde_json::Value;

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
    let lines = store::read_category_lines("journeys")?;
    let mut out = Vec::new();
    for line in lines {
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

/// Identity key for grouping. Journey emails about one trip arrive
/// over weeks (booking → reminder → delay-repay), so we can't use a
/// simple date key. Use (destination, journey_time) plus the ISO-week
/// of the received date — that groups the day-before reminder + same-
/// hour disruption alert together, while keeping the post-trip
/// delay-repay (typically arriving the next week) as a separate row
/// because it carries different info (the fare).
fn identity_key(j: &Journey) -> Option<String> {
    let d = j.destination.as_deref()?.trim();
    if d.is_empty() {
        return None;
    }
    let t = j.journey_time.as_deref().unwrap_or("");
    let recv = j.received?;
    let iso = recv.iso_week();
    Some(format!("{}|{}|{}-W{:02}", d.to_lowercase(), t, iso.year(), iso.week()))
}

fn dedupe_journeys(rows: Vec<Journey>) -> Vec<(Journey, usize)> {
    use std::collections::HashMap;
    let mut groups: HashMap<String, Vec<Journey>> = HashMap::new();
    let mut orphans: Vec<Journey> = Vec::new();
    for j in rows {
        match identity_key(&j) {
            Some(k) => groups.entry(k).or_default().push(j),
            None => orphans.push(j),
        }
    }
    let mut out: Vec<(Journey, usize)> = Vec::new();
    for (_, mut group) in groups {
        let count = group.len();
        // Pick the canonical row: most populated fields, then most recent.
        group.sort_by(|a, b| {
            let pa = populated_count(a);
            let pb = populated_count(b);
            pb.cmp(&pa).then_with(|| b.received.cmp(&a.received))
        });
        let canonical = group.into_iter().next().unwrap();
        out.push((canonical, count));
    }
    for o in orphans {
        out.push((o, 1));
    }
    out.sort_by(|a, b| b.0.received.cmp(&a.0.received));
    out
}

fn populated_count(j: &Journey) -> usize {
    [
        j.destination.is_some(),
        j.journey_date.is_some(),
        j.journey_time.is_some(),
        j.fare.is_some(),
        j.booking_ref.is_some(),
    ]
    .iter()
    .filter(|x| **x)
    .count()
}

/// Filter row that has zero useful data (no destination, no time, no
/// fare) — these are typically "Thanks for booking" emails where every
/// extracted field is null and the row is just noise.
fn has_useful_data(j: &Journey) -> bool {
    j.destination.is_some() || j.journey_time.is_some() || j.fare.is_some()
}

pub fn list(year: Option<i32>, limit: usize) -> Result<()> {
    let all = load_journeys()?;
    let total_emails = all.len();
    let useful: Vec<Journey> = all.into_iter().filter(has_useful_data).collect();
    let deduped = dedupe_journeys(useful);
    let unique_count = deduped.len();
    let filtered: Vec<(&Journey, usize)> = deduped
        .iter()
        .filter(|(j, _)| match (year, j.received) {
            (Some(y), Some(d)) => d.year() == y,
            (Some(_), None) => false,
            (None, _) => true,
        })
        .take(limit)
        .map(|(j, n)| (j, *n))
        .collect();
    print_journeys(&filtered, total_emails, unique_count);
    Ok(())
}

pub fn find(query: &str, limit: usize) -> Result<()> {
    let q = query.to_lowercase();
    let all = load_journeys()?;
    let total_emails = all.len();
    let useful: Vec<Journey> = all.into_iter().filter(has_useful_data).collect();
    let deduped = dedupe_journeys(useful);
    let unique_count = deduped.len();
    let filtered: Vec<(&Journey, usize)> = deduped
        .iter()
        .filter(|(j, _)| {
            j.subject.to_lowercase().contains(&q)
                || j.destination.as_deref().is_some_and(|s| s.to_lowercase().contains(&q))
                || j.journey_date.as_deref().is_some_and(|s| s.to_lowercase().contains(&q))
        })
        .take(limit)
        .map(|(j, n)| (j, *n))
        .collect();
    print_journeys(&filtered, total_emails, unique_count);
    Ok(())
}

pub fn recent(days: i64, limit: usize) -> Result<()> {
    let cutoff = Utc::now() - chrono::Duration::days(days);
    let all = load_journeys()?;
    let total_emails = all.len();
    let useful: Vec<Journey> = all.into_iter().filter(has_useful_data).collect();
    let deduped = dedupe_journeys(useful);
    let unique_count = deduped.len();
    let filtered: Vec<(&Journey, usize)> = deduped
        .iter()
        .filter(|(j, _)| j.received.map(|d| d > cutoff).unwrap_or(false))
        .take(limit)
        .map(|(j, n)| (j, *n))
        .collect();
    print_journeys(&filtered, total_emails, unique_count);
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

fn print_journeys(items: &[(&Journey, usize)], total_emails: usize, unique_count: usize) {
    if items.is_empty() {
        println!(
            "no journeys matching filter ({total_emails} email records, {unique_count} unique trips)"
        );
        return;
    }
    println!(
        "{:<10}  {:<22}  {:>5}  {:>6}  {:>5}  {}",
        "received", "destination", "time", "fare £", "msgs", "subject"
    );
    println!("{}", "─".repeat(96));
    for (j, n) in items {
        let date_s = j
            .received
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "?".to_string());
        let dest = j.destination.as_deref().unwrap_or("—");
        let time = j.journey_time.as_deref().unwrap_or("—");
        let fare = j.fare.as_deref().unwrap_or("—");
        let subj = truncate(&j.subject, 40);
        println!(
            "{date_s:<10}  {:<22}  {time:>5}  {fare:>6}  {n:>5}  {subj}",
            truncate(dest, 22)
        );
    }
    println!();
    println!(
        "{} unique trip(s) shown / {unique_count} unique / {total_emails} email records",
        items.len()
    );
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
