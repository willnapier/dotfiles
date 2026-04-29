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
    checkin_time: Option<String>,
    checkout: Option<String>,
    checkout_time: Option<String>,
    host: Option<String>,
    guests: Option<String>,
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
            checkin_time: opt_string(&obj, "checkin_time"),
            checkout: opt_string(&obj, "checkout"),
            checkout_time: opt_string(&obj, "checkout_time"),
            host: opt_string(&obj, "host"),
            guests: opt_string(&obj, "guests"),
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

/// Identity key for deduping. Prefer the explicit booking_ref when
/// present; fall back to (property_normalised, checkin_normalised) so
/// different date formats for the same stay collapse to the same key.
/// Without normalisation, "Friday 24 April 2026", "Fri 24 Apr", and
/// "24 Apr" all looked like distinct bookings.
fn identity_key(b: &Booking) -> Option<String> {
    // Prefer (property, parsed-date) over booking_ref because the same
    // physical stay may have ref extracted on some emails (LLM) and not
    // others (deterministic only) — keying on ref first split a single
    // stay across multiple groups. Property + normalised date is stable
    // across all emails about a booking.
    match (&b.property, &b.checkin) {
        (Some(p), Some(c)) if !p.is_empty() && !c.is_empty() => {
            let prop = normalise_property(p);
            let date = parse_checkin(c)
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| c.to_lowercase());
            return Some(format!("pc:{prop}|{date}"));
        }
        _ => {}
    }
    if let Some(r) = &b.booking_ref {
        if !r.is_empty() {
            return Some(format!("ref:{r}"));
        }
    }
    None
}

/// Lowercase + collapse whitespace + strip emoji-ish chars so
/// "**ELEGANT STAY** CENT…" and "Elegant Stay Central Manchester"
/// hash to the same key.
fn normalise_property(s: &str) -> String {
    let lower = s.to_lowercase();
    let cleaned: String = lower
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Collapse rows that refer to the same booking. Two-pass:
///   1. Group by primary key (property+checkin preferred, ref as fallback).
///      Track which refs appear within property-keyed groups.
///   2. Merge any ref-keyed group into its property-keyed parent if a
///      member of the property-keyed group has the same ref. Handles
///      the case where some emails have ref extracted (LLM) and others
///      don't, but they're all the same physical stay.
///
/// Singletons (no identifying fields at all) pass through with count=1.
fn dedupe_by_booking(rows: Vec<Booking>) -> Vec<(Booking, usize)> {
    use std::collections::HashMap;
    let mut groups: HashMap<String, Vec<Booking>> = HashMap::new();
    let mut singletons: Vec<Booking> = Vec::new();
    // ref → property-keyed group it belongs to (populated from records
    // whose primary key is property+checkin AND who have a booking_ref).
    let mut ref_to_pc_group: HashMap<String, String> = HashMap::new();

    for b in rows {
        match identity_key(&b) {
            Some(k) => {
                if k.starts_with("pc:") {
                    if let Some(r) = b.booking_ref.as_deref() {
                        if !r.is_empty() {
                            ref_to_pc_group.insert(r.to_string(), k.clone());
                        }
                    }
                }
                groups.entry(k).or_default().push(b);
            }
            None => singletons.push(b),
        }
    }

    // Second pass: merge ref-keyed groups into matching property-keyed groups.
    let ref_keys: Vec<String> = groups
        .keys()
        .filter(|k| k.starts_with("ref:"))
        .cloned()
        .collect();
    for ref_key in ref_keys {
        let r = ref_key.strip_prefix("ref:").unwrap();
        if let Some(target) = ref_to_pc_group.get(r).cloned() {
            if target != ref_key {
                if let Some(members) = groups.remove(&ref_key) {
                    groups.entry(target).or_default().extend(members);
                }
            }
        }
    }

    let mut out: Vec<(Booking, usize)> = Vec::new();
    for (_, mut group) in groups {
        let count = group.len();
        // Pick canonical: maximise populated fields, tiebreak by recency.
        group.sort_by(|a, b| {
            let pa = populated_count(a);
            let pb = populated_count(b);
            pb.cmp(&pa).then_with(|| b.received.cmp(&a.received))
        });
        let canonical = group.into_iter().next().unwrap();
        out.push((canonical, count));
    }
    for s in singletons {
        out.push((s, 1));
    }
    out.sort_by(|a, b| b.0.received.cmp(&a.0.received));
    out
}

fn populated_count(b: &Booking) -> usize {
    [
        b.property.is_some(),
        b.checkin.is_some(),
        b.checkin_time.is_some(),
        b.checkout.is_some(),
        b.checkout_time.is_some(),
        b.host.is_some(),
        b.guests.is_some(),
        b.total.is_some(),
        b.booking_ref.is_some(),
    ]
    .iter()
    .filter(|x| **x)
    .count()
}

fn parse_rfc2822(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Best-effort parse of the `checkin` field into a `DateTime<Utc>` for
/// future-vs-past filtering AND identity-key normalisation. Handles
/// compact "24 Apr" (assumes current year), "Fri 24 Apr 2026", "Friday
/// 24 April 2026", and "April 24, 2026" forms. Returns None if no
/// recognisable date emerges.
fn parse_checkin(s: &str) -> Option<DateTime<Utc>> {
    static DAY_MONTH_RE: OnceLock<Regex> = OnceLock::new();
    let day_month = DAY_MONTH_RE.get_or_init(|| {
        // Greedy `[A-Za-z]+` so the month captures fully; the non-greedy
        // form was matching just one letter, breaking month-name lookup.
        Regex::new(r"\b(\d{1,2})\s+([A-Za-z]+)(?:[,\s]+(\d{4}))?").unwrap()
    });
    static MONTH_DAY_RE: OnceLock<Regex> = OnceLock::new();
    let month_day = MONTH_DAY_RE.get_or_init(|| {
        // "April 24, 2026" — month-first form.
        Regex::new(r"\b([A-Za-z]+)\s+(\d{1,2})(?:[,\s]+(\d{4}))?").unwrap()
    });

    // Try day-first first since it's more common in the data.
    let (day, month_str, year_opt) = if let Some(caps) = day_month.captures(s) {
        (
            caps.get(1)?.as_str().parse::<u32>().ok()?,
            caps.get(2)?.as_str().to_string(),
            caps.get(3).and_then(|m| m.as_str().parse::<i32>().ok()),
        )
    } else if let Some(caps) = month_day.captures(s) {
        (
            caps.get(2)?.as_str().parse::<u32>().ok()?,
            caps.get(1)?.as_str().to_string(),
            caps.get(3).and_then(|m| m.as_str().parse::<i32>().ok()),
        )
    } else {
        return None;
    };

    let year = year_opt.unwrap_or_else(|| Utc::now().year());
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
    let total = all.len();
    let deduped = dedupe_by_booking(all);
    let mut upcoming: Vec<(DateTime<Utc>, &Booking, usize)> = deduped
        .iter()
        .filter_map(|(b, n)| b.checkin.as_deref().and_then(parse_checkin).map(|d| (d, b, *n)))
        .filter(|(d, _, _)| *d >= now - chrono::Duration::days(1))
        .collect();
    upcoming.sort_by_key(|(d, _, _)| *d);
    upcoming.truncate(limit);
    if upcoming.is_empty() {
        println!("no upcoming bookings ({total} email records, {} unique bookings in store)", deduped.len());
        return Ok(());
    }
    println!(
        "{:<12}  {:<22}  {:<14}  {:<25}  {:<10}  {:>5}",
        "checkin", "property", "checkout", "host", "guests", "msgs"
    );
    println!("{}", "─".repeat(94));
    for (d, b, n) in &upcoming {
        let property = b.property.as_deref().unwrap_or("—");
        let host = b.host.as_deref().unwrap_or("—");
        let guests = b.guests.as_deref().unwrap_or("—");
        let checkout = b.checkout.as_deref().unwrap_or("—");
        let checkin_disp = compound_when(Some(&d.format("%Y-%m-%d").to_string()), b.checkin_time.as_deref());
        println!(
            "{:<12}  {:<22}  {:<14}  {:<25}  {:<10}  {n:>5}",
            checkin_disp,
            truncate(property, 22),
            truncate(checkout, 14),
            truncate(host, 25),
            truncate(guests, 10)
        );
    }
    Ok(())
}

pub fn list(year: Option<i32>, limit: usize) -> Result<()> {
    let all = load_bookings()?;
    let total = all.len();
    let deduped = dedupe_by_booking(all);
    let filtered: Vec<(&Booking, usize)> = deduped
        .iter()
        .filter(|(b, _)| match (year, b.received) {
            (Some(y), Some(d)) => d.year() == y,
            (Some(_), None) => false,
            (None, _) => true,
        })
        .take(limit)
        .map(|(b, n)| (b, *n))
        .collect();
    print_bookings(&filtered, total, deduped.len());
    Ok(())
}

pub fn find(query: &str, limit: usize) -> Result<()> {
    let q = query.to_lowercase();
    let all = load_bookings()?;
    let total = all.len();
    let deduped = dedupe_by_booking(all);
    let filtered: Vec<(&Booking, usize)> = deduped
        .iter()
        .filter(|(b, _)| {
            b.subject.to_lowercase().contains(&q)
                || b.property.as_deref().is_some_and(|s| s.to_lowercase().contains(&q))
                || b.host.as_deref().is_some_and(|s| s.to_lowercase().contains(&q))
        })
        .take(limit)
        .map(|(b, n)| (b, *n))
        .collect();
    print_bookings(&filtered, total, deduped.len());
    Ok(())
}

fn print_bookings(items: &[(&Booking, usize)], emails_in_store: usize, unique_bookings: usize) {
    if items.is_empty() {
        println!(
            "no bookings matching filter ({emails_in_store} email records, {unique_bookings} unique bookings in store)"
        );
        return;
    }
    println!(
        "{:<22}  {:<22}  {:<22}  {:<10}  {:>5}",
        "property", "checkin", "checkout", "guests", "msgs"
    );
    println!("{}", "─".repeat(88));
    for (b, n) in items {
        let property = b.property.as_deref().unwrap_or("—");
        let checkin = compound_when(b.checkin.as_deref(), b.checkin_time.as_deref());
        let checkout = compound_when(b.checkout.as_deref(), b.checkout_time.as_deref());
        let guests = b.guests.as_deref().unwrap_or("—");
        println!(
            "{:<22}  {:<22}  {:<22}  {:<10}  {n:>5}",
            truncate(property, 22),
            truncate(&checkin, 22),
            truncate(&checkout, 22),
            truncate(guests, 10)
        );
    }
    println!();
    println!(
        "{} unique booking(s) shown / {} unique in store / {} email records",
        items.len(),
        unique_bookings,
        emails_in_store
    );
}

/// Render check-in / check-out as date + time, or just date if time unknown.
/// "Friday 24 April 2026 16:00" → keep as-is; "24 Apr" → "24 Apr".
fn compound_when(date: Option<&str>, time: Option<&str>) -> String {
    match (date, time) {
        (Some(d), Some(t)) if !d.contains(t) => format!("{d} {t}"),
        (Some(d), _) => d.to_string(),
        (None, Some(t)) => t.to_string(),
        (None, None) => "—".to_string(),
    }
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
