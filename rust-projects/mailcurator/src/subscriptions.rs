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
use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

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
#[allow(dead_code)] // Agent A will use these
#[derive(Debug, Clone)]
pub struct SubscriptionStatus {
    pub service: String,
    pub status: ServiceStatus,
    pub last_seen: String,
    pub next_renewal: Option<String>,
    pub amount: Option<String>,
    pub frequency: Option<String>,
    pub cancellation_notice_days: Option<i64>,
    pub events: Vec<SubscriptionEvent>,
}

#[allow(dead_code)] // Agent A will use these
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceStatus {
    Active,
    Cancelled,
    Dormant,
}

/// Append one event to ~/.local/share/mailcurator/subscriptions.jsonl.
/// Used by both extract.rs (Track B extractors) and Track A discover.
#[allow(dead_code)] // wired up by Agents A/B/C
pub fn append_event(event: &SubscriptionEvent) -> Result<()> {
    store::append_record("subscriptions", event)
        .context("appending to subscriptions.jsonl")
}

/// Load all events from disk. Empty Vec if the file doesn't exist yet.
#[allow(dead_code)] // Agent A will use this
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

// ============================================================================
// Subcommand entry points — STUBBED. Filled in by parallel agents.
// ============================================================================

/// `mailcurator subscriptions list` — Agent A.
pub fn list() -> Result<()> {
    anyhow::bail!("subscriptions list: not yet implemented (Agent A pending)")
}

/// `mailcurator subscriptions check [--alert]` — Agent A.
pub fn check(_alert: bool, _buffer_days: i64) -> Result<()> {
    anyhow::bail!("subscriptions check: not yet implemented (Agent A pending)")
}

/// `mailcurator subscriptions report [--period 30d]` — Agent A.
pub fn report(_period: &str) -> Result<()> {
    anyhow::bail!("subscriptions report: not yet implemented (Agent A pending)")
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
