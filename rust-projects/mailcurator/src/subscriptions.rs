// Subscription monitoring — append-only event log + synthesised state.
//
// Schema + design contract: see SUBSCRIPTIONS.md at the crate root.
//
// Storage: ~/.local/share/mailcurator/subscriptions.jsonl (one event per line).
// Source of truth is the event log; current state is synthesised on demand.
//
// This module is the FOUNDATION. Three independent work packages extend it:
//   - Agent A fills in `list`, `check`, `report` (read side)
//   - Agent B fills in `discover` (Track A heuristic)
//   - Agent C wires Apple subscription extractor (calls `append_event` from
//     extract.rs path; no changes needed in this module)
//
// All three rely on the schema declared here. Don't change field shapes or
// names without updating SUBSCRIPTIONS.md.

use anyhow::{Context, Result};
use chrono::{Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::process::Command;

use crate::store;

/// One event in the subscription log. Matches the schema in SUBSCRIPTIONS.md.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionEvent {
    pub ts: String,
    pub event: EventType,
    pub service: String,
    pub source: String,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub next_renewal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub frequency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cancellation_notice_days: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub extracted_at: Option<String>,

    // Candidate-only fields (event = Candidate)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub confidence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    RenewalReminder,
    Charged,
    SubscriptionStarted,
    CancellationConfirmed,
    Candidate,
}

/// Synthesised current state for one service. Computed on demand from the
/// event log; not persisted.
#[derive(Debug, Clone)]
pub struct SubscriptionStatus {
    pub service: String,
    pub status: ServiceStatus,
    pub last_seen: String,
    pub next_renewal: Option<String>,
    pub amount: Option<String>,
    pub frequency: Option<String>,
    pub cancellation_notice_days: Option<i64>,
    /// Full chronological event list. Carried through synthesis so callers
    /// (including downstream tooling) can drill into history without re-loading.
    #[allow(dead_code)]
    pub events: Vec<SubscriptionEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceStatus {
    Active,
    Cancelled,
    Dormant,
}

impl ServiceStatus {
    fn as_str(&self) -> &'static str {
        match self {
            ServiceStatus::Active => "active",
            ServiceStatus::Cancelled => "cancelled",
            ServiceStatus::Dormant => "dormant",
        }
    }
}

/// Append one event to ~/.local/share/mailcurator/subscriptions.jsonl.
/// Used by both extract.rs (Track B extractors) and Track A discover.
#[allow(dead_code)] // wired up by Agents B/C
pub fn append_event(event: &SubscriptionEvent) -> Result<()> {
    store::append_record("subscriptions", event)
        .context("appending to subscriptions.jsonl")
}

/// Load all events from disk. Empty Vec if the file doesn't exist yet.
pub fn load_events() -> Result<Vec<SubscriptionEvent>> {
    let path = store::store_dir()?.join("subscriptions.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let mut out = Vec::new();
    for (n, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let evt: SubscriptionEvent = serde_json::from_str(line)
            .with_context(|| format!("parsing subscriptions.jsonl line {}", n + 1))?;
        out.push(evt);
    }
    Ok(out)
}

/// Group events by service and synthesise current state per service.
/// See SUBSCRIPTIONS.md for the synthesis rules.
pub fn synthesise(events: &[SubscriptionEvent]) -> Vec<SubscriptionStatus> {
    // Group by service. BTreeMap gives deterministic service ordering before
    // any caller re-sorts (e.g. by next_renewal).
    let mut by_service: BTreeMap<String, Vec<SubscriptionEvent>> = BTreeMap::new();
    for ev in events {
        by_service.entry(ev.service.clone()).or_default().push(ev.clone());
    }

    let now = Utc::now();
    let ninety_days = Duration::days(90);

    let mut out = Vec::with_capacity(by_service.len());
    for (service, mut evs) in by_service {
        // Chronological order by ts. Lexicographic sort on RFC 3339 UTC strings
        // matches chronological order, so no parsing needed.
        evs.sort_by(|a, b| a.ts.cmp(&b.ts));

        let last_seen = evs.last().map(|e| e.ts.clone()).unwrap_or_default();

        // status:
        //   - Cancelled if the latest event is CancellationConfirmed
        //   - Active if any non-cancellation event has ts within last 90 days
        //   - Dormant otherwise
        let latest_is_cancellation = evs
            .last()
            .map(|e| e.event == EventType::CancellationConfirmed)
            .unwrap_or(false);

        let status = if latest_is_cancellation {
            ServiceStatus::Cancelled
        } else {
            let any_recent_non_cancel = evs.iter().any(|e| {
                if e.event == EventType::CancellationConfirmed {
                    return false;
                }
                match chrono::DateTime::parse_from_rfc3339(&e.ts) {
                    Ok(dt) => now.signed_duration_since(dt.with_timezone(&Utc)) <= ninety_days,
                    Err(_) => false,
                }
            });
            if any_recent_non_cancel {
                ServiceStatus::Active
            } else {
                ServiceStatus::Dormant
            }
        };

        // next_renewal: from the most recent RenewalReminder for this service
        // that has a populated next_renewal field.
        let next_renewal = evs
            .iter()
            .rev()
            .find(|e| e.event == EventType::RenewalReminder && e.next_renewal.is_some())
            .and_then(|e| e.next_renewal.clone());

        // amount / frequency / cancellation_notice_days: most recent populated
        // value across any event for this service.
        let amount = evs.iter().rev().find_map(|e| e.amount.clone());
        let frequency = evs.iter().rev().find_map(|e| e.frequency.clone());
        let cancellation_notice_days = evs.iter().rev().find_map(|e| e.cancellation_notice_days);

        out.push(SubscriptionStatus {
            service,
            status,
            last_seen,
            next_renewal,
            amount,
            frequency,
            cancellation_notice_days,
            events: evs,
        });
    }
    out
}

// ============================================================================
// Subcommand entry points
// ============================================================================

/// `mailcurator subscriptions list`
pub fn list() -> Result<()> {
    let events = load_events()?;
    let mut statuses = synthesise(&events);
    if statuses.is_empty() {
        println!("(no subscriptions logged yet)");
        return Ok(());
    }

    // Sort by next_renewal ascending; None goes last.
    statuses.sort_by(|a, b| match (&a.next_renewal, &b.next_renewal) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.service.cmp(&b.service),
    });

    println!(
        "{:<28}  {:<9}  {:<12}  {:<10}  {:<10}  last_seen",
        "service", "status", "next_renew", "amount", "frequency"
    );
    for s in &statuses {
        println!(
            "{:<28}  {:<9}  {:<12}  {:<10}  {:<10}  {}",
            truncate(&s.service, 28),
            s.status.as_str(),
            s.next_renewal.as_deref().unwrap_or("—"),
            truncate(s.amount.as_deref().unwrap_or("—"), 10),
            truncate(s.frequency.as_deref().unwrap_or("—"), 10),
            truncate(&s.last_seen, 25),
        );
    }
    Ok(())
}

/// `mailcurator subscriptions check [--alert] [--buffer-days N]`
pub fn check(alert: bool, buffer_days: i64) -> Result<()> {
    let events = load_events()?;
    let statuses = synthesise(&events);

    let today = Utc::now().date_naive();
    let mut flagged: Vec<(SubscriptionStatus, NaiveDate, NaiveDate)> = Vec::new();

    for s in statuses {
        if s.status != ServiceStatus::Active {
            continue;
        }
        let Some(nr_str) = s.next_renewal.as_deref() else {
            continue;
        };
        let Some(notice_days) = s.cancellation_notice_days else {
            continue;
        };
        let next_renewal = match NaiveDate::parse_from_str(nr_str, "%Y-%m-%d") {
            Ok(d) => d,
            Err(_) => continue,
        };

        let decide_by = next_renewal - Duration::days(notice_days) - Duration::days(buffer_days);
        if today >= decide_by {
            flagged.push((s, next_renewal, decide_by));
        }
    }

    if flagged.is_empty() {
        println!("no subscriptions in cancellation-decision window");
        return Ok(());
    }

    println!(
        "{:<28}  {:<12}  {:<12}  {:<10}",
        "service", "next_renew", "decide_by", "amount"
    );
    for (s, nr, db) in &flagged {
        println!(
            "{:<28}  {:<12}  {:<12}  {:<10}",
            truncate(&s.service, 28),
            nr,
            db,
            truncate(s.amount.as_deref().unwrap_or("—"), 10),
        );
    }

    if alert {
        for (s, nr, db) in &flagged {
            let entry = format!("subs:: {} renews {}, decide by {}", s.service, nr, db);
            let status = Command::new("daypage-append")
                .arg(&entry)
                .status()
                .with_context(|| format!("spawning daypage-append for '{}'", s.service))?;
            if !status.success() {
                anyhow::bail!(
                    "daypage-append failed (exit {:?}) for service '{}'",
                    status.code(),
                    s.service
                );
            }
        }
        println!("\nqueued {} alert(s) to today's DayPage", flagged.len());
    }

    Ok(())
}

/// `mailcurator subscriptions report [--period 30d]`
pub fn report(period: &str) -> Result<()> {
    let events = load_events()?;
    let statuses = synthesise(&events);

    let mut active: Vec<&SubscriptionStatus> = Vec::new();
    let mut dormant: Vec<&SubscriptionStatus> = Vec::new();
    let mut cancelled: Vec<&SubscriptionStatus> = Vec::new();
    for s in &statuses {
        match s.status {
            ServiceStatus::Active => active.push(s),
            ServiceStatus::Dormant => dormant.push(s),
            ServiceStatus::Cancelled => cancelled.push(s),
        }
    }

    println!("=== subscriptions report (period: {period}) ===");
    println!();
    println!("active:    {}", active.len());
    println!("dormant:   {}", dormant.len());
    println!("cancelled: {}", cancelled.len());
    println!();

    // Estimated annual cost: sum across active services with a parseable amount
    // and a known frequency multiplier.
    let mut annual_total: f64 = 0.0;
    let mut currency_hint: Option<String> = None;
    let mut unparseable: usize = 0;
    let mut no_frequency: usize = 0;
    for s in &active {
        let Some(amount_str) = s.amount.as_deref() else {
            unparseable += 1;
            continue;
        };
        let Some(freq) = s.frequency.as_deref() else {
            no_frequency += 1;
            continue;
        };
        let multiplier: f64 = match freq {
            "monthly" => 12.0,
            "annual" => 1.0,
            "quarterly" => 4.0,
            "weekly" => 52.0,
            _ => {
                no_frequency += 1;
                continue;
            }
        };
        match parse_amount(amount_str) {
            Some((sym, n)) => {
                annual_total += n * multiplier;
                if currency_hint.is_none() {
                    currency_hint = sym;
                }
            }
            None => unparseable += 1,
        }
    }

    let sym = currency_hint.as_deref().unwrap_or("");
    println!("estimated annual cost (active subs): {sym}{annual_total:.2}");
    if unparseable > 0 {
        println!("  ({unparseable} active subs had unparseable amounts)");
    }
    if no_frequency > 0 {
        println!("  ({no_frequency} active subs had unknown/missing frequency)");
    }
    println!();

    if !dormant.is_empty() {
        println!("=== dormant services (>90 days since last event) ===");
        for s in &dormant {
            println!(
                "  {:<28}  last_seen={}",
                truncate(&s.service, 28),
                s.last_seen
            );
        }
        println!();
    }

    let blind_spots: Vec<&&SubscriptionStatus> = active
        .iter()
        .filter(|s| s.next_renewal.is_none())
        .collect();
    if !blind_spots.is_empty() {
        println!("=== active services with no known next_renewal (blind spots) ===");
        for s in &blind_spots {
            println!("  {}", s.service);
        }
        println!();
    }

    Ok(())
}

/// `mailcurator subscriptions discover [--commit]` — Agent B.
pub fn discover(_commit: bool, _window: &str) -> Result<()> {
    anyhow::bail!("subscriptions discover: not yet implemented (Agent B pending)")
}

// ============================================================================
// Helpers
// ============================================================================

/// Parse an amount string like "£8.99", "$9.99", "12.50" into
/// (Some(currency_symbol_or_None), numeric_value). Returns None if no number
/// can be extracted.
fn parse_amount(s: &str) -> Option<(Option<String>, f64)> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut symbol = String::new();
    let mut digits = String::new();
    let mut seen_digit = false;
    for ch in trimmed.chars() {
        let is_numeric_char = ch.is_ascii_digit() || ch == '.' || ch == '-';
        if !seen_digit && !is_numeric_char {
            // Leading currency symbol or letters: capture as symbol prefix.
            if !ch.is_whitespace() {
                symbol.push(ch);
            }
            continue;
        }
        if is_numeric_char {
            digits.push(ch);
            seen_digit = true;
        } else if ch == ',' {
            // Allow thousands separator.
            continue;
        } else {
            // Hit non-numeric after digits — stop.
            break;
        }
    }
    if digits.is_empty() {
        return None;
    }
    let n: f64 = digits.parse().ok()?;
    let sym = if symbol.is_empty() { None } else { Some(symbol) };
    Some((sym, n))
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(ts: &str, event: EventType, service: &str) -> SubscriptionEvent {
        SubscriptionEvent {
            ts: ts.to_string(),
            event,
            service: service.to_string(),
            source: "<test>".to_string(),
            next_renewal: None,
            amount: None,
            currency: None,
            frequency: None,
            cancellation_notice_days: None,
            subject: None,
            from: None,
            extracted_at: None,
            confidence: None,
            reason: None,
        }
    }

    #[test]
    fn synthesise_groups_and_picks_latest() {
        let now = Utc::now();
        let recent = (now - Duration::days(10)).to_rfc3339();
        let old = (now - Duration::days(400)).to_rfc3339();

        let mut e1 = ev(&recent, EventType::RenewalReminder, "apple.com");
        e1.next_renewal = Some("2026-05-15".into());
        e1.amount = Some("£8.99".into());
        e1.frequency = Some("monthly".into());
        e1.cancellation_notice_days = Some(2);

        let mut e2 = ev(&old, EventType::SubscriptionStarted, "drop.app");
        e2.amount = Some("$9.99".into());
        e2.frequency = Some("monthly".into());

        let e3 = ev(&recent, EventType::CancellationConfirmed, "drop.app");

        let out = synthesise(&[e1, e2, e3]);
        assert_eq!(out.len(), 2);

        let apple = out.iter().find(|s| s.service == "apple.com").unwrap();
        assert_eq!(apple.status, ServiceStatus::Active);
        assert_eq!(apple.next_renewal.as_deref(), Some("2026-05-15"));
        assert_eq!(apple.amount.as_deref(), Some("£8.99"));

        let drop = out.iter().find(|s| s.service == "drop.app").unwrap();
        assert_eq!(drop.status, ServiceStatus::Cancelled);
    }

    #[test]
    fn dormant_when_no_recent_events() {
        let now = Utc::now();
        let old = (now - Duration::days(400)).to_rfc3339();
        let e = ev(&old, EventType::Charged, "old.svc");
        let out = synthesise(&[e]);
        assert_eq!(out[0].status, ServiceStatus::Dormant);
    }

    #[test]
    fn parse_amount_basics() {
        assert_eq!(parse_amount("£8.99").unwrap().1, 8.99);
        assert_eq!(parse_amount("$9.99").unwrap().1, 9.99);
        assert_eq!(parse_amount("12.50").unwrap().1, 12.50);
        assert!(parse_amount("free").is_none());
    }

    #[test]
    fn synthesise_empty_input() {
        assert!(synthesise(&[]).is_empty());
    }
}
