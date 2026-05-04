//! `mailcurator bills` — query interface to ~/.local/share/mailcurator/bills.jsonl
//!
//! Symmetric with `orders` / `journeys` / `bookings` / `tesla` query
//! commands. Bills are multi-vendor (Octopus, Vodafone, PayPal-via-PayPal
//! receipts, Direct Line, etc.) so the lookup is a flat per-record query
//! with `--vendor` / `--year` / `--limit` filters.
//!
//! Vendor matching is case-insensitive substring against either `vendor`
//! (utility rows) or `counterparty` (PayPal rows where the canonical
//! vendor is "PayPal" but the merchant is what you actually want).

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Utc};
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};

use crate::store;

#[derive(Debug, Clone)]
struct Bill {
    received: Option<DateTime<Utc>>,
    subject: String,
    vendor: Option<String>,
    counterparty: Option<String>,
    /// Display amount (raw string from JSONL — preserves precision).
    /// Utility rows use `amount`; PayPal rows use `amount_gbp`.
    amount: Option<String>,
    currency: Option<String>,
    message_id: String,
}

impl Bill {
    /// "Effective vendor" for matching purposes — PayPal rows substitute
    /// counterparty since the literal vendor is just the intermediary.
    fn effective_vendor(&self) -> Option<&str> {
        match self.vendor.as_deref() {
            Some(v) if v.eq_ignore_ascii_case("paypal") => self.counterparty.as_deref(),
            Some(v) => Some(v),
            None => self.counterparty.as_deref(),
        }
    }
}

fn load_bills() -> Result<Vec<Bill>> {
    let path = store::category_path("bills")?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let f = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
    let mut out: Vec<Bill> = Vec::new();
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
        let vendor = obj.get("vendor").and_then(|v| v.as_str()).map(str::to_string);
        let counterparty = obj
            .get("counterparty")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        // Utility rows expose `amount`; PayPal-style rows expose `amount_gbp`.
        let amount = obj
            .get("amount")
            .or_else(|| obj.get("amount_gbp"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let currency = obj
            .get("currency")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let mid = obj
            .get("message_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_default();

        let new_record = Bill {
            received,
            subject,
            vendor,
            counterparty,
            amount,
            currency,
            message_id: mid.clone(),
        };

        // Defensive dedup-by-message-id at load time. Multi-mailbox Gmail
        // pulls (Inbox + Sent + All Mail) can yield duplicate rows for
        // the same message; keep the most populated one.
        if !mid.is_empty() {
            if let Some(&idx) = seen_mids.get(&mid) {
                let new_score = bill_populated(&new_record);
                let old_score = bill_populated(&out[idx]);
                if new_score > old_score {
                    out[idx] = new_record;
                }
                continue;
            }
            seen_mids.insert(mid, out.len());
        }
        out.push(new_record);
    }
    out.sort_by(|a, b| b.received.cmp(&a.received));
    Ok(out)
}

fn bill_populated(b: &Bill) -> usize {
    [
        b.amount.is_some(),
        b.vendor.is_some() || b.counterparty.is_some(),
    ]
    .iter()
    .filter(|x| **x)
    .count()
}

fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(s)
        .ok()
        .or_else(|| {
            // Some headers carry a trailing "(UTC)"-style parenthetical.
            let cleaned = s.split('(').next()?.trim();
            DateTime::parse_from_rfc2822(cleaned).ok()
        })
        .map(|dt| dt.with_timezone(&Utc))
}

/// Query bills.jsonl with optional vendor / year filters.
pub fn list(vendor: Option<&str>, year: Option<i32>, limit: usize) -> Result<()> {
    let bills = load_bills()?;
    let v_lower = vendor.map(|v| v.to_lowercase());
    let filtered: Vec<&Bill> = bills
        .iter()
        .filter(|b| match (year, b.received) {
            (Some(y), Some(d)) => d.year() == y,
            (Some(_), None) => false,
            (None, _) => true,
        })
        .filter(|b| match &v_lower {
            None => true,
            Some(v) => b
                .effective_vendor()
                .map(|name| name.to_lowercase().contains(v))
                .unwrap_or(false),
        })
        .take(limit)
        .collect();
    print_bills(&filtered, bills.len());
    Ok(())
}

fn print_bills(bills: &[&Bill], total_in_store: usize) {
    if bills.is_empty() {
        println!("no bills matching filter (store has {total_in_store} total)");
        return;
    }
    println!(
        "{:<10}  {:<22}  {:>10}  {:<48}  {}",
        "date", "vendor", "amount", "subject", "message_id"
    );
    println!("{}", "─".repeat(110));
    for b in bills {
        let date_s = b
            .received
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "?".to_string());
        let vendor = truncate(b.effective_vendor().unwrap_or("—"), 22);
        let amount = format_money(b.amount.as_deref(), b.currency.as_deref());
        let subj = truncate(&b.subject, 48);
        let mid = truncate(&b.message_id, 32);
        println!("{date_s:<10}  {vendor:<22}  {amount:>10}  {subj:<48}  {mid}");
    }
    println!();
    println!("{} matching / {} in store", bills.len(), total_in_store);
}

fn format_money(amount: Option<&str>, currency: Option<&str>) -> String {
    let Some(a) = amount else { return "—".to_string() };
    let code = currency.unwrap_or("GBP");
    let symbol = match code {
        "GBP" => "£",
        "EUR" => "€",
        "USD" => "$",
        "JPY" => "¥",
        "AUD" => "A$",
        _ => "",
    };
    if symbol.is_empty() {
        format!("{a} {code}")
    } else {
        format!("{symbol}{a}")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_bill(
        vendor: Option<&str>,
        counterparty: Option<&str>,
        amount: Option<&str>,
        received_year: i32,
    ) -> Bill {
        let received = chrono::NaiveDate::from_ymd_opt(received_year, 6, 15)
            .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc());
        Bill {
            received,
            subject: "test subject".to_string(),
            vendor: vendor.map(String::from),
            counterparty: counterparty.map(String::from),
            amount: amount.map(String::from),
            currency: Some("GBP".to_string()),
            message_id: "<test@host>".to_string(),
        }
    }

    #[test]
    fn effective_vendor_uses_counterparty_for_paypal() {
        let b = mk_bill(Some("PayPal"), Some("Dropbox"), Some("12.50"), 2025);
        assert_eq!(b.effective_vendor(), Some("Dropbox"));
    }

    #[test]
    fn effective_vendor_uses_vendor_otherwise() {
        let b = mk_bill(Some("Vodafone"), None, Some("55.67"), 2025);
        assert_eq!(b.effective_vendor(), Some("Vodafone"));
    }

    #[test]
    fn effective_vendor_falls_back_to_counterparty_when_vendor_absent() {
        let b = mk_bill(None, Some("ACME"), None, 2025);
        assert_eq!(b.effective_vendor(), Some("ACME"));
    }

    #[test]
    fn format_money_gbp() {
        assert_eq!(format_money(Some("12.50"), Some("GBP")), "£12.50");
        assert_eq!(format_money(Some("12.50"), None), "£12.50");
        assert_eq!(format_money(None, Some("GBP")), "—");
    }

    #[test]
    fn format_money_aud() {
        assert_eq!(format_money(Some("100.00"), Some("AUD")), "A$100.00");
    }

    #[test]
    fn parse_date_rfc2822() {
        let d = parse_date("Sat, 4 Oct 2025 11:04:54 -0000").unwrap();
        assert_eq!(d.year(), 2025);
        assert_eq!(d.month(), 10);
        assert_eq!(d.day(), 4);
    }
}
