//! `mailcurator bookings` — query interface for Airbnb bookings (and
//! eventually any booking-shaped category).
//!
//! Most-useful subcommand: `upcoming`, which surfaces bookings whose
//! check-in date is in the future.

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Utc};
use regex::Regex;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::sync::OnceLock;

use crate::store;

#[derive(Debug, Clone)]
struct Booking {
    received: Option<DateTime<Utc>>,
    subject: String,
    property: Option<String>,
    checkin: Option<String>,
    checkout: Option<String>,
    host: Option<String>,
    total: Option<String>,
    booking_ref: Option<String>,
}

fn load_bookings() -> Result<Vec<Booking>> {
    let path = store::category_path("bookings")?;
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
        out.push(Booking {
            received,
            subject,
            property: opt_string(&obj, "property"),
            checkin: opt_string(&obj, "checkin"),
            checkout: opt_string(&obj, "checkout"),
            host: opt_string(&obj, "host"),
            total: opt_string(&obj, "total"),
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

/// Best-effort parse of the `checkin` field into a `DateTime<Utc>` for
/// future-vs-past filtering. Handles compact "24 Apr" format (assumes
/// current year) and longer forms like "April 24, 2026" or "Saturday,
/// April 24, 2026". Returns None if no recognisable date emerges.
fn parse_checkin(s: &str) -> Option<DateTime<Utc>> {
    static DAY_MONTH_RE: OnceLock<Regex> = OnceLock::new();
    let day_month = DAY_MONTH_RE.get_or_init(|| {
        Regex::new(r"(\d{1,2})\s+([A-Za-z]+?)(?:\s+(\d{4}))?").unwrap()
    });
    let caps = day_month.captures(s)?;
    let day: u32 = caps.get(1)?.as_str().parse().ok()?;
    let month_str = caps.get(2)?.as_str();
    let year: i32 = caps
        .get(3)
        .and_then(|m| m.as_str().parse().ok())
        .unwrap_or_else(|| Utc::now().year());
    let month = match &month_str.to_lowercase()[..3.min(month_str.len())] {
        "jan" => 1, "feb" => 2, "mar" => 3, "apr" => 4,
        "may" => 5, "jun" => 6, "jul" => 7, "aug" => 8,
        "sep" => 9, "oct" => 10, "nov" => 11, "dec" => 12,
        _ => return None,
    };
    chrono::NaiveDate::from_ymd_opt(year, month, day)?
        .and_hms_opt(0, 0, 0)
        .map(|nd| nd.and_utc())
}

pub fn upcoming(limit: usize) -> Result<()> {
    let now = Utc::now();
    let all = load_bookings()?;
    let mut upcoming: Vec<(DateTime<Utc>, &Booking)> = all
        .iter()
        .filter_map(|b| b.checkin.as_deref().and_then(parse_checkin).map(|d| (d, b)))
        .filter(|(d, _)| *d >= now - chrono::Duration::days(1))
        .collect();
    upcoming.sort_by_key(|(d, _)| *d);
    upcoming.truncate(limit);
    if upcoming.is_empty() {
        println!("no upcoming bookings (store has {} total)", all.len());
        return Ok(());
    }
    println!(
        "{:<12}  {:<25}  {:<25}  {:>6}  {}",
        "checkin", "property", "host", "total", "subject"
    );
    println!("{}", "─".repeat(96));
    for (d, b) in &upcoming {
        let property = b.property.as_deref().unwrap_or("—");
        let host = b.host.as_deref().unwrap_or("—");
        let total = b.total.as_deref().unwrap_or("—");
        let subj = truncate(&b.subject, 35);
        println!(
            "{:<12}  {:<25}  {:<25}  {:>6}  {subj}",
            d.format("%Y-%m-%d"),
            truncate(property, 25),
            truncate(host, 25),
            total
        );
    }
    Ok(())
}

pub fn list(year: Option<i32>, limit: usize) -> Result<()> {
    let all = load_bookings()?;
    let filtered: Vec<&Booking> = all
        .iter()
        .filter(|b| match (year, b.received) {
            (Some(y), Some(d)) => d.year() == y,
            (Some(_), None) => false,
            (None, _) => true,
        })
        .take(limit)
        .collect();
    print_bookings(&filtered, all.len());
    Ok(())
}

pub fn find(query: &str, limit: usize) -> Result<()> {
    let q = query.to_lowercase();
    let all = load_bookings()?;
    let filtered: Vec<&Booking> = all
        .iter()
        .filter(|b| {
            b.subject.to_lowercase().contains(&q)
                || b.property.as_deref().is_some_and(|s| s.to_lowercase().contains(&q))
                || b.host.as_deref().is_some_and(|s| s.to_lowercase().contains(&q))
        })
        .take(limit)
        .collect();
    print_bookings(&filtered, all.len());
    Ok(())
}

fn print_bookings(items: &[&Booking], total_in_store: usize) {
    if items.is_empty() {
        println!("no bookings matching filter (store has {total_in_store} total)");
        return;
    }
    println!(
        "{:<12}  {:<25}  {:<10}  {:<10}  {}",
        "received", "property", "checkin", "checkout", "host"
    );
    println!("{}", "─".repeat(85));
    for b in items {
        let recv = b
            .received
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "?".to_string());
        let property = b.property.as_deref().unwrap_or("—");
        let checkin = b.checkin.as_deref().unwrap_or("—");
        let checkout = b.checkout.as_deref().unwrap_or("—");
        let host = b.host.as_deref().unwrap_or("—");
        println!(
            "{recv:<12}  {:<25}  {:<10}  {:<10}  {host}",
            truncate(property, 25),
            truncate(checkin, 10),
            truncate(checkout, 10)
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
