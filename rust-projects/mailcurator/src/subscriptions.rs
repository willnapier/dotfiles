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
use serde::{Deserialize, Serialize};

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

/// `mailcurator subscriptions discover [--commit]` — Agent B.
pub fn discover(_commit: bool, _window: &str) -> Result<()> {
    anyhow::bail!("subscriptions discover: not yet implemented (Agent B pending)")
}
