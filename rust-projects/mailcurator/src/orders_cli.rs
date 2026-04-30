//! `mailcurator orders` — query interface to ~/.local/share/mailcurator/orders.jsonl
//!
//! The whole point of extract-and-destroy is that the jsonl IS the
//! authoritative store. This module makes it queryable so you reach for it
//! first instead of grepping email.

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Utc};
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};

use crate::store;

#[derive(Debug, Clone)]
struct Order {
    received: Option<DateTime<Utc>>,
    subject: String,
    order_id: Option<String>,
    total: Option<String>,
    eta: Option<String>,
    items: Vec<String>,
    raw: Value,
}

fn load_orders() -> Result<Vec<Order>> {
    let path = store::category_path("orders")?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let f = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let mut out = Vec::new();
    // Defensive dedup-by-message-id at load time. Gmail label-pull
    // (Inbox + Sent + All Mail) can produce 3x duplicates per message
    // in some accounts; the extract-time dedup handles it for fresh
    // runs but historical jsonl may still have legacy dups. Keeping
    // the most-information-rich record per message-id.
    let mut seen_mids: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
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
            .and_then(parse_date);
        let subject = obj
            .get("subject")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let order_id = obj
            .get("order_id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let total = obj
            .get("total")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let eta = obj
            .get("eta")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let items = obj
            .get("items")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mid = obj
            .get("message_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_default();
        let new_record = Order {
            received,
            subject,
            order_id,
            total,
            eta,
            items,
            raw: obj,
        };
        if !mid.is_empty() {
            if let Some(&idx) = seen_mids.get(&mid) {
                // Replace existing if new record is more populated.
                let new_score = order_populated(&new_record);
                let old_score = order_populated(&out[idx]);
                if new_score > old_score {
                    out[idx] = new_record;
                }
                continue;
            }
            seen_mids.insert(mid, out.len());
        }
        out.push(new_record);
    }
    // Newest first.
    out.sort_by(|a, b| b.received.cmp(&a.received));
    Ok(out)
}

fn order_populated(o: &Order) -> usize {
    [
        o.order_id.is_some(),
        o.total.is_some(),
        o.eta.is_some(),
        !o.items.is_empty(),
    ]
    .iter()
    .filter(|x| **x)
    .count()
}

fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    // Header dates are RFC2822, sometimes with trailing parentheticals.
    DateTime::parse_from_rfc2822(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

pub fn list(year: Option<i32>, limit: usize) -> Result<()> {
    let orders = load_orders()?;
    let filtered: Vec<&Order> = orders
        .iter()
        .filter(|o| match (year, o.received) {
            (Some(y), Some(d)) => d.year() == y,
            (Some(_), None) => false,
            (None, _) => true,
        })
        .take(limit)
        .collect();
    print_orders(&filtered, orders.len());
    Ok(())
}

pub fn find(query: &str, limit: usize) -> Result<()> {
    let orders = load_orders()?;
    let q = query.to_lowercase();
    let filtered: Vec<&Order> = orders
        .iter()
        .filter(|o| {
            o.subject.to_lowercase().contains(&q)
                || o.order_id.as_deref().is_some_and(|i| i.to_lowercase().contains(&q))
                || o.items.iter().any(|i| i.to_lowercase().contains(&q))
        })
        .take(limit)
        .collect();
    print_orders(&filtered, orders.len());
    Ok(())
}

pub fn recent(days: i64, limit: usize) -> Result<()> {
    let cutoff = Utc::now() - chrono::Duration::days(days);
    let orders = load_orders()?;
    let filtered: Vec<&Order> = orders
        .iter()
        .filter(|o| o.received.map(|d| d > cutoff).unwrap_or(false))
        .take(limit)
        .collect();
    print_orders(&filtered, orders.len());
    Ok(())
}

pub fn total(year: Option<i32>) -> Result<()> {
    let orders = load_orders()?;
    let mut sum = 0.0_f64;
    let mut counted = 0usize;
    let mut missing_total = 0usize;
    let mut missing_year = 0usize;
    for o in &orders {
        if let Some(y) = year {
            match o.received {
                Some(d) if d.year() == y => {}
                _ => {
                    missing_year += 1;
                    continue;
                }
            }
        }
        match o.total.as_deref().and_then(|t| t.replace(',', "").parse::<f64>().ok()) {
            Some(v) => {
                sum += v;
                counted += 1;
            }
            None => missing_total += 1,
        }
    }
    let label = year.map(|y| format!("year {y}")).unwrap_or_else(|| "all years".to_string());
    println!("orders {label}: counted={counted}  missing-total={missing_total}");
    if year.is_none() {
        println!("(records skipped due to year filter: {missing_year})");
    }
    println!("sum (£): {sum:.2}");
    Ok(())
}

fn print_orders(orders: &[&Order], total_in_store: usize) {
    if orders.is_empty() {
        println!("no orders matching filter (store has {total_in_store} total)");
        return;
    }
    println!(
        "{:<10}  {:<19}  {:>8}  {}",
        "date", "order_id", "total £", "subject"
    );
    println!("{}", "─".repeat(80));
    for o in orders {
        let date_s = o
            .received
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "?".to_string());
        let order_id = o.order_id.as_deref().unwrap_or("—");
        let total = o.total.as_deref().unwrap_or("—");
        let subj = truncate(&o.subject, 50);
        println!("{date_s:<10}  {order_id:<19}  {total:>8}  {subj}");
        if !o.items.is_empty() {
            for item in o.items.iter().take(3) {
                println!("            └─ {}", truncate(item, 60));
            }
            if o.items.len() > 3 {
                println!("            └─ … and {} more", o.items.len() - 3);
            }
        }
    }
    let _ = &orders[0].raw; // silence unused-field warning if `raw` not referenced elsewhere
    println!();
    println!("{} matching / {} in store", orders.len(), total_in_store);
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
