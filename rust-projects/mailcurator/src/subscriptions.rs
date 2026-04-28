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
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
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

/// Normalise a raw frequency string to the canonical schema enum value.
/// Vendors emit varied forms (`year`, `Yearly`, `Monthly`, `weekly`); this
/// maps them to the canonical `annual` / `monthly` / `weekly` / `quarterly`.
/// Unrecognised input is preserved as-is (lowercased) so downstream code
/// can still display it; the report multiplier table treats unknown strings
/// as "unparseable frequency" and counts them separately.
fn normalise_frequency(raw: &str) -> String {
    match raw.trim().to_lowercase().as_str() {
        "year" | "yearly" | "annual" | "annually" | "1y" => "annual".to_string(),
        "month" | "monthly" | "1m" => "monthly".to_string(),
        "week" | "weekly" | "1w" => "weekly".to_string(),
        "quarter" | "quarterly" | "3m" => "quarterly".to_string(),
        other => other.to_string(),
    }
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
        // Frequency is normalised at the synthesis boundary because extractors
        // emit raw vendor strings (Apple uses "Monthly"/"Yearly"/"year" etc.)
        // while the canonical schema is monthly/annual/quarterly/weekly.
        // Normalising here means `report` and other downstream code can rely
        // on canonical values.
        let amount = evs.iter().rev().find_map(|e| e.amount.clone());
        let frequency = evs.iter().rev().find_map(|e| e.frequency.clone()).map(|f| normalise_frequency(&f));
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

/// `mailcurator subscriptions discover [--commit]` — Agent B (Track A heuristic).
///
/// Scans inbox messages within `window` for subscription-pattern subjects,
/// excludes services already known via non-Candidate events in the log,
/// and either prints candidates (default) or appends them as Candidate
/// events to subscriptions.jsonl (with `--commit`).
pub fn discover(commit: bool, window: &str) -> Result<()> {
    // Master subscription-pattern regex (case-insensitive).
    // Hits become candidates.
    let pattern = Regex::new(
        r"(?i)(subscription|renew|renewal|will renew|will be charged|recurring|auto.?renew|membership|next billing|billing cycle)"
    ).context("compiling subscription-pattern regex")?;

    // Strong/medium-confidence anchors.
    let strong = Regex::new(
        r"(?i)(renewal|subscription|auto.?renew|billing cycle)"
    ).context("compiling strong-keyword regex")?;
    // Date-ish patterns ("on May 15", "2026-05-15", "15/05/2026", "May 15, 2026", "in 7 days")
    let date_like = Regex::new(
        r"(?i)(\b\d{4}-\d{2}-\d{2}\b|\b\d{1,2}[/-]\d{1,2}[/-]\d{2,4}\b|\b(?:jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\s+\d{1,2}\b|\bin\s+\d+\s+days?\b|\bon\s+\w+\s+\d{1,2}\b)"
    ).context("compiling date-like regex")?;

    // Build query, fetch matching messages.
    let query = format!("tag:inbox and not tag:trash and date:{window}..");
    let messages = store::list_messages(&query)
        .with_context(|| format!("listing messages for query: {query}"))?;

    // Load existing events once; build set of known services (non-Candidate only).
    let existing = load_events().context("loading existing subscription events")?;
    let known: HashSet<String> = existing
        .iter()
        .filter(|e| e.event != EventType::Candidate)
        .map(|e| e.service.clone())
        .collect();

    // Gather candidates.
    struct Candidate {
        service: String,
        from: String,
        subject: String,
        confidence: &'static str,
        reason: String,
        source: String,
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut skipped_known = 0usize;

    for m in &messages {
        // Find all unique pattern keyword hits in the subject.
        let hits: Vec<String> = pattern
            .find_iter(&m.subject)
            .map(|mat| mat.as_str().to_lowercase())
            .collect();
        if hits.is_empty() {
            continue;
        }
        let unique_hits: HashSet<String> = hits.iter().cloned().collect();

        let service = service_from_from(&m.from);
        if known.contains(&service) {
            skipped_known += 1;
            continue;
        }

        // Confidence rating
        let strong_hit = strong.is_match(&m.subject);
        let renew_hit = Regex::new(r"(?i)\brenew").unwrap().is_match(&m.subject);
        let date_hit = date_like.is_match(&m.subject);

        let (confidence, conf_reason) = if unique_hits.len() >= 2 {
            ("high", format!("{} keywords", unique_hits.len()))
        } else if renew_hit && date_hit {
            ("high", "renew + date".to_string())
        } else if strong_hit {
            ("medium", "strong keyword".to_string())
        } else {
            ("low", "weak keyword".to_string())
        };

        let first_hit = hits.first().cloned().unwrap_or_else(|| "match".to_string());
        let reason = format!(
            "subject matched /{}/i; confidence={} ({})",
            first_hit, confidence, conf_reason
        );

        candidates.push(Candidate {
            service,
            from: m.from.clone(),
            subject: m.subject.clone(),
            confidence,
            reason,
            source: m.message_id.clone(),
        });
    }

    // Sort: high → medium → low, then by service.
    fn rank(c: &str) -> u8 {
        match c {
            "high" => 0,
            "medium" => 1,
            _ => 2,
        }
    }
    candidates.sort_by(|a, b| {
        rank(a.confidence)
            .cmp(&rank(b.confidence))
            .then_with(|| a.service.cmp(&b.service))
    });

    if !commit {
        // Print table.
        println!(
            "{:<10}  {:<25}  {:<40}  {}",
            "conf", "service", "from", "subject"
        );
        println!("{}", "-".repeat(110));
        for c in &candidates {
            println!(
                "{:<10}  {:<25}  {:<40}  {}",
                c.confidence,
                truncate_local(&c.service, 25),
                truncate_local(&c.from, 40),
                truncate_local(&c.subject, 80),
            );
        }
        println!();
        println!(
            "{} candidates ({} skipped as already-known)",
            candidates.len(),
            skipped_known
        );
        return Ok(());
    }

    // Commit: write each candidate as a Candidate SubscriptionEvent.
    let now = Utc::now().to_rfc3339();
    let mut written = 0usize;
    for c in &candidates {
        let evt = SubscriptionEvent {
            ts: now.clone(),
            event: EventType::Candidate,
            service: c.service.clone(),
            source: c.source.clone(),
            next_renewal: None,
            amount: None,
            currency: None,
            frequency: None,
            cancellation_notice_days: None,
            subject: Some(c.subject.clone()),
            from: Some(c.from.clone()),
            extracted_at: Some(now.clone()),
            confidence: Some(c.confidence.to_string()),
            reason: Some(c.reason.clone()),
        };
        append_event(&evt)
            .with_context(|| format!("appending candidate for service {}", c.service))?;
        written += 1;
    }
    println!(
        "Wrote {} candidates to subscriptions.jsonl ({} skipped as already-known).",
        written, skipped_known
    );
    Ok(())
}

/// Extract a normalised service identifier from a "from" header value.
///
/// Strategy:
///   - Prefer the address inside `<...>`.
///   - Otherwise, take the first whitespace-delimited token containing `@`.
///   - Take the part after the last `@`, lowercase, strip common subdomains.
///   - If no domain is extractable, return `unknown:<ascii-prefix-of-from>`.
fn service_from_from(from: &str) -> String {
    // notmuch authors can be pipe-separated; take the first.
    let first = from.split('|').next().unwrap_or(from).trim();

    let addr: Option<String> = if let (Some(s), Some(e)) = (first.find('<'), first.find('>')) {
        if e > s {
            Some(first[s + 1..e].trim().to_string())
        } else {
            None
        }
    } else {
        // Fallback: a token containing '@'.
        first
            .split_whitespace()
            .find(|t| t.contains('@'))
            .map(|s| s.trim_matches(|c: char| !c.is_ascii_graphic()).to_string())
            .or_else(|| {
                if first.contains('@') {
                    Some(first.to_string())
                } else {
                    None
                }
            })
    };

    let domain = addr.and_then(|a| a.rsplit_once('@').map(|(_, d)| d.to_lowercase()));

    if let Some(mut d) = domain {
        // Strip a stray trailing `>` if present.
        if let Some(stripped) = d.strip_suffix('>') {
            d = stripped.to_string();
        }
        let prefixes = [
            "mail.",
            "email.",
            "noreply.",
            "no-reply.",
            "notifications.",
            "info.",
        ];
        for p in &prefixes {
            if let Some(rest) = d.strip_prefix(p) {
                d = rest.to_string();
                break;
            }
        }
        if !d.is_empty() {
            return d;
        }
    }

    // Last-ditch: ascii-only short prefix of the raw from field.
    let ascii: String = first
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(8)
        .collect();
    if ascii.is_empty() {
        "unknown:nofrom".to_string()
    } else {
        format!("unknown:{}", ascii.to_lowercase())
    }
}

fn truncate_local(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
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
