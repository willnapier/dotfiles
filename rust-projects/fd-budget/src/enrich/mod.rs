//! Enrichment: join email-evidence (mailcurator bills.jsonl) against bank rows.
//!
//! Produces `matches.jsonl` keyed by `bank_import_id`, one row per bank row.
//! Confidence tiers: `high`, `medium`, `ambiguous`, `internal-transfer`, `none`.

use crate::Transaction;
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::str::FromStr;

// ---------------------------------------------------------------------------
// Email row (parsed from bills.jsonl)
// ---------------------------------------------------------------------------

/// A single row from `bills.jsonl`, normalised across utility-style and PayPal-style shapes.
#[derive(Debug, Clone)]
pub struct EmailRow {
    pub message_id: String,
    pub vendor: Option<String>,
    pub counterparty: Option<String>,
    /// Amount in GBP — utility rows use `amount`; PayPal rows use `amount_gbp`.
    pub amount: Option<Decimal>,
    /// Date parsed from `received` (RFC2822-ish).
    pub received_date: Option<NaiveDate>,
    /// Optional due_date `dd/mm/yyyy`. Utility rows only.
    pub due_date: Option<NaiveDate>,
    /// "received" or "refund" or "sent" or null. Most rows have no direction.
    pub direction: Option<String>,
    pub policy: Option<String>,
}

impl EmailRow {
    /// Effective vendor name for matching.
    /// PayPal is an intermediary — use counterparty when available.
    pub fn effective_vendor(&self) -> Option<&str> {
        match self.vendor.as_deref() {
            Some(v) if v.eq_ignore_ascii_case("paypal") => self.counterparty.as_deref(),
            Some(v) => Some(v),
            None => self.counterparty.as_deref(),
        }
    }

    pub fn is_paypal(&self) -> bool {
        matches!(self.vendor.as_deref(), Some(v) if v.eq_ignore_ascii_case("paypal"))
    }

    /// "Best guess" date — closest of due_date or received_date to a target.
    /// Falls back to whichever exists.
    pub fn best_date_for(&self, target: NaiveDate) -> Option<NaiveDate> {
        match (self.due_date, self.received_date) {
            (Some(d), Some(r)) => {
                let dd = (d - target).num_days().abs();
                let rd = (r - target).num_days().abs();
                Some(if dd <= rd { d } else { r })
            }
            (Some(d), None) => Some(d),
            (None, Some(r)) => Some(r),
            (None, None) => None,
        }
    }
}

/// Parse a single JSONL line into an [`EmailRow`].
fn parse_email_row(line: &str) -> Option<EmailRow> {
    let raw: serde_json::Value = serde_json::from_str(line).ok()?;
    let obj = raw.as_object()?;

    let message_id = obj
        .get("message_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())?;

    let vendor = obj
        .get("vendor")
        .and_then(|v| v.as_str())
        .map(String::from);

    let counterparty = obj
        .get("counterparty")
        .and_then(|v| v.as_str())
        .map(String::from);

    let amount = obj
        .get("amount_gbp")
        .or_else(|| obj.get("amount"))
        .and_then(|v| v.as_str())
        .and_then(|s| Decimal::from_str(s).ok());

    let received_date = obj
        .get("received")
        .and_then(|v| v.as_str())
        .and_then(parse_rfc2822_date);

    let due_date = obj
        .get("due_date")
        .and_then(|v| v.as_str())
        .and_then(parse_due_date);

    let direction = obj
        .get("direction")
        .and_then(|v| v.as_str())
        .map(String::from);

    let policy = obj
        .get("policy")
        .and_then(|v| v.as_str())
        .map(String::from);

    Some(EmailRow {
        message_id,
        vendor,
        counterparty,
        amount,
        received_date,
        due_date,
        direction,
        policy,
    })
}

fn parse_rfc2822_date(s: &str) -> Option<NaiveDate> {
    // chrono is strict; many real headers are not strict RFC2822.
    DateTime::parse_from_rfc2822(s.trim())
        .ok()
        .map(|dt| dt.naive_utc().date())
        .or_else(|| {
            // Some headers have a trailing "(UTC)" or similar — strip parenthetical and retry.
            let cleaned = s.split('(').next()?.trim();
            DateTime::parse_from_rfc2822(cleaned)
                .ok()
                .map(|dt| dt.naive_utc().date())
        })
}

fn parse_due_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%d/%m/%Y").ok()
}

/// Load all email rows from a JSONL file, skipping invalid lines.
pub fn load_email_rows<P: AsRef<Path>>(path: P) -> std::io::Result<Vec<EmailRow>> {
    let file = File::open(path.as_ref())?;
    let reader = BufReader::new(file);
    let mut rows = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(row) = parse_email_row(trimmed) {
            rows.push(row);
        }
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Confidence {
    High,
    Medium,
    Ambiguous,
    InternalTransfer,
    None,
}

impl Confidence {
    pub fn as_str(&self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Ambiguous => "ambiguous",
            Confidence::InternalTransfer => "internal-transfer",
            Confidence::None => "none",
        }
    }
}

/// Internal candidate during the per-bank-row match attempt.
#[derive(Debug, Clone)]
struct Candidate<'e> {
    row: &'e EmailRow,
    tier: Tier,
    amount_delta: Decimal,
    date_delta_days: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tier {
    Medium = 1,
    High = 2,
}

#[derive(Debug, Clone)]
pub struct MatchResult {
    pub bank_import_id: String,
    pub confidence: Confidence,
    pub email_message_ids: Vec<String>,
    pub amount_delta: Option<Decimal>,
    pub date_delta_days: Option<i64>,
    pub counterparty_hint: Option<String>,
    pub candidates: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
pub struct MatchOptions {
    pub amount_tolerance: Decimal,
    pub date_window_days: i64,
}

impl Default for MatchOptions {
    fn default() -> Self {
        Self {
            amount_tolerance: Decimal::new(1, 2), // 0.01
            date_window_days: 3,
        }
    }
}

/// Detect First Direct VISA-payoff direct-debit lines.
pub fn is_internal_transfer(description: &str) -> bool {
    let lower = description.to_lowercase();
    lower.contains("first direct visa") && lower.contains("direct debit")
}

/// Tokenise into ASCII alphanumeric tokens of len ≥ 3, lowercased.
fn tokenize(s: &str) -> HashSet<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 3)
        .map(|t| t.to_lowercase())
        .collect()
}

/// Compute name-match tier between bank description and email's effective vendor.
///
/// Returns `Some(High)` for substring match, `Some(Medium)` for token overlap > 0.5,
/// or `None` if neither.
fn name_tier(bank_desc: &str, email: &EmailRow) -> Option<Tier> {
    let vendor = email.effective_vendor()?;
    if vendor.is_empty() {
        return None;
    }

    let desc_lower = bank_desc.to_lowercase();
    let vendor_lower = vendor.to_lowercase();

    // Substring check (vendor word inside bank description, or vice versa).
    let desc_tokens = tokenize(&desc_lower);
    let vendor_tokens = tokenize(&vendor_lower);

    for vt in &vendor_tokens {
        if desc_lower.contains(vt) {
            return Some(Tier::High);
        }
    }
    for dt in &desc_tokens {
        if vendor_lower.contains(dt) {
            return Some(Tier::High);
        }
    }

    // PayPal-specific fallback: bank desc starts with "PAYPAL *"
    if desc_lower.starts_with("paypal *") || desc_lower.starts_with("paypal*") {
        let stripped = desc_lower
            .trim_start_matches("paypal *")
            .trim_start_matches("paypal*")
            .trim();
        // Allow short substring overlap; the merchant code is heavily truncated.
        // Try 3-char windows from the bank fragment.
        let fragment: String = stripped.chars().take_while(|c| !c.is_whitespace()).collect();
        if fragment.len() >= 3 {
            for i in 0..=fragment.len().saturating_sub(3) {
                let win = &fragment[i..i + 3];
                if vendor_lower.contains(win) {
                    return Some(Tier::High);
                }
            }
        }
    }

    // Token overlap.
    if desc_tokens.is_empty() || vendor_tokens.is_empty() {
        return None;
    }
    let intersection = desc_tokens.intersection(&vendor_tokens).count();
    let denom = desc_tokens.len().min(vendor_tokens.len()).max(1);
    let overlap = intersection as f64 / denom as f64;
    if overlap > 0.5 {
        Some(Tier::Medium)
    } else {
        None
    }
}

/// Match a single bank row against pre-bucketed email rows.
fn match_one<'e>(
    tx: &Transaction,
    by_date: &BTreeMap<NaiveDate, Vec<&'e EmailRow>>,
    opts: MatchOptions,
) -> MatchResult {
    if is_internal_transfer(&tx.description) || is_internal_transfer(&tx.raw_description) {
        return MatchResult {
            bank_import_id: tx.import_id.clone(),
            confidence: Confidence::InternalTransfer,
            email_message_ids: Vec::new(),
            amount_delta: None,
            date_delta_days: None,
            counterparty_hint: None,
            candidates: None,
        };
    }

    let bank_amount_abs = tx.amount.abs();
    let is_credit = tx.is_credit();

    // Walk ±N days and collect candidates.
    let mut candidates: Vec<Candidate> = Vec::new();
    let window = opts.date_window_days;
    for offset in -window..=window {
        let probe_date = match tx.date.checked_add_signed(chrono::Duration::days(offset)) {
            Some(d) => d,
            None => continue,
        };
        let Some(bucket) = by_date.get(&probe_date) else {
            continue;
        };
        for email in bucket {
            // Sign-direction filter.
            // Most rows have direction == None — those pass for both signs.
            if let Some(dir) = email.direction.as_deref() {
                let dir_lower = dir.to_lowercase();
                let is_credit_email =
                    dir_lower == "received" || dir_lower == "refund";
                if is_credit && !is_credit_email && dir_lower != "sent" {
                    // bank credit but email isn't a credit — skip
                    // We allow "sent" only for debit-side.
                    continue;
                }
                if !is_credit && (dir_lower == "received" || dir_lower == "refund") {
                    continue;
                }
            }

            // Amount filter.
            let Some(email_amount) = email.amount else {
                continue;
            };
            let delta = (email_amount - bank_amount_abs).abs();
            if delta > opts.amount_tolerance {
                continue;
            }

            // Date delta — if both dates exist, use the closer one.
            let email_date = match email.best_date_for(tx.date) {
                Some(d) => d,
                None => continue,
            };
            let date_delta = (email_date - tx.date).num_days().abs();
            if date_delta > window {
                continue;
            }

            // Name match tier.
            let Some(tier) = name_tier(&tx.description, email)
                .or_else(|| name_tier(&tx.raw_description, email))
            else {
                continue;
            };

            // High tier requires date_delta == 0.
            // Medium permits within window.
            let final_tier = if tier == Tier::High && date_delta != 0 {
                Tier::Medium
            } else {
                tier
            };

            candidates.push(Candidate {
                row: email,
                tier: final_tier,
                amount_delta: delta,
                date_delta_days: date_delta,
            });
        }
    }

    if candidates.is_empty() {
        return MatchResult {
            bank_import_id: tx.import_id.clone(),
            confidence: Confidence::None,
            email_message_ids: Vec::new(),
            amount_delta: None,
            date_delta_days: None,
            counterparty_hint: None,
            candidates: None,
        };
    }

    // Keep highest tier only.
    let max_tier = candidates.iter().map(|c| c.tier).max().unwrap();
    candidates.retain(|c| c.tier == max_tier);

    // De-dup by message_id (some emails may be matched twice via due_date+received_date).
    let mut seen = HashSet::new();
    candidates.retain(|c| seen.insert(c.row.message_id.clone()));

    if candidates.len() == 1 {
        let c = &candidates[0];
        let confidence = match c.tier {
            Tier::High => Confidence::High,
            Tier::Medium => Confidence::Medium,
        };
        let counterparty_hint = if matches!(c.tier, Tier::Medium) {
            c.row.effective_vendor().map(String::from)
        } else {
            None
        };
        MatchResult {
            bank_import_id: tx.import_id.clone(),
            confidence,
            email_message_ids: vec![c.row.message_id.clone()],
            amount_delta: Some(c.amount_delta),
            date_delta_days: Some(c.date_delta_days),
            counterparty_hint,
            candidates: None,
        }
    } else {
        let count = candidates.len();
        let ids: Vec<String> = candidates.iter().map(|c| c.row.message_id.clone()).collect();
        MatchResult {
            bank_import_id: tx.import_id.clone(),
            confidence: Confidence::Ambiguous,
            email_message_ids: ids,
            amount_delta: None,
            date_delta_days: None,
            counterparty_hint: None,
            candidates: Some(count),
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level orchestration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct EnrichSummary {
    pub bank_rows: usize,
    pub email_rows: usize,
    pub by_tier: HashMap<Confidence, usize>,
}

impl EnrichSummary {
    pub fn enriched_count(&self) -> usize {
        self.by_tier
            .iter()
            .filter(|(k, _)| !matches!(k, Confidence::None))
            .map(|(_, v)| *v)
            .sum()
    }

    pub fn count(&self, tier: Confidence) -> usize {
        self.by_tier.get(&tier).copied().unwrap_or(0)
    }
}

pub fn enrich(
    transactions: &[Transaction],
    email_rows: &[EmailRow],
    opts: MatchOptions,
) -> (Vec<MatchResult>, EnrichSummary) {
    // Bucket emails by their date(s) — both due_date and received_date go into
    // their own buckets so date-window scans hit both.
    let mut by_date: BTreeMap<NaiveDate, Vec<&EmailRow>> = BTreeMap::new();
    for row in email_rows {
        if let Some(d) = row.due_date {
            by_date.entry(d).or_default().push(row);
        }
        if let Some(d) = row.received_date {
            // Avoid duplicate entry if both exist on same date.
            let bucket = by_date.entry(d).or_default();
            if row.due_date != Some(d) {
                bucket.push(row);
            }
        }
    }

    let mut results: Vec<MatchResult> =
        transactions.iter().map(|t| match_one(t, &by_date, opts)).collect();
    results.sort_by(|a, b| a.bank_import_id.cmp(&b.bank_import_id));

    let mut summary = EnrichSummary {
        bank_rows: transactions.len(),
        email_rows: email_rows.len(),
        by_tier: HashMap::new(),
    };
    for r in &results {
        *summary.by_tier.entry(r.confidence).or_insert(0) += 1;
    }

    (results, summary)
}

// ---------------------------------------------------------------------------
// JSONL output
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct OutputRow<'a> {
    bank_import_id: &'a str,
    email_message_ids: &'a [String],
    confidence: &'a str,
    matched_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    amount_delta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    date_delta_days: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    counterparty_hint: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidates: Option<usize>,
}

pub fn write_matches<P: AsRef<Path>>(path: P, results: &[MatchResult]) -> std::io::Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = File::create(path.as_ref())?;
    let mut writer = BufWriter::new(file);
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    for r in results {
        let row = OutputRow {
            bank_import_id: &r.bank_import_id,
            email_message_ids: &r.email_message_ids,
            confidence: r.confidence.as_str(),
            matched_at: now.clone(),
            amount_delta: r.amount_delta.map(|d| d.to_string()),
            date_delta_days: r.date_delta_days,
            counterparty_hint: r.counterparty_hint.as_deref(),
            candidates: r.candidates,
        };
        let line = serde_json::to_string(&row).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::Other, format!("JSON error: {e}"))
        })?;
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Account, TxType};

    fn mk_tx(date_str: &str, amount: &str, desc: &str, id: &str) -> Transaction {
        Transaction {
            date: NaiveDate::parse_from_str(date_str, "%Y-%m-%d").unwrap(),
            account: Account::Current,
            tx_type: TxType::Unknown(0),
            amount: Decimal::from_str(amount).unwrap(),
            description: desc.to_string(),
            raw_description: desc.to_string(),
            balance: None,
            tags: Vec::new(),
            import_id: id.to_string(),
        }
    }

    fn mk_email(
        msg_id: &str,
        vendor: Option<&str>,
        counterparty: Option<&str>,
        amount: Option<&str>,
        received: Option<&str>,
        due: Option<&str>,
    ) -> EmailRow {
        EmailRow {
            message_id: msg_id.to_string(),
            vendor: vendor.map(String::from),
            counterparty: counterparty.map(String::from),
            amount: amount.and_then(|s| Decimal::from_str(s).ok()),
            received_date: received.and_then(parse_rfc2822_date),
            due_date: due.and_then(parse_due_date),
            direction: None,
            policy: None,
        }
    }

    #[test]
    fn test_token_overlap_substring() {
        let email = mk_email("<v@1>", Some("Vodafone"), None, Some("55.67"), None, None);
        let tier = name_tier("VODAFONE LTD", &email);
        assert_eq!(tier, Some(Tier::High));
    }

    #[test]
    fn test_paypal_prefix_strip() {
        let email = mk_email(
            "<p@1>",
            Some("PayPal"),
            Some("Dropbox International"),
            Some("130.87"),
            None,
            None,
        );
        // Real PayPal merchant codes like "PAYPAL *DROPBOXIN" — first 3 chars "dro" should match
        let tier = name_tier("PAYPAL *DROPBOXIN", &email);
        assert_eq!(tier, Some(Tier::High));
    }

    #[test]
    fn test_internal_transfer_detection() {
        // Spec mandates BOTH "FIRST DIRECT VISA" AND "DIRECT DEBIT".
        assert!(is_internal_transfer(
            "FIRST DIRECT VISA DIRECT DEBIT"
        ));
        assert!(is_internal_transfer(
            "first direct visa via direct debit"
        ));
        assert!(!is_internal_transfer("FIRST DIRECT VISA"));
        assert!(!is_internal_transfer("DIRECT DEBIT only"));
        assert!(!is_internal_transfer("groceries"));
    }

    #[test]
    fn test_sign_normalised_match() {
        let bank = mk_tx("2025-10-04", "-55.67", "VODAFONE LTD DD", "abc");
        let email = mk_email(
            "<v@1>",
            Some("Vodafone"),
            None,
            Some("55.67"),
            Some("Sat, 4 Oct 2025 11:04:54 -0000"),
            Some("13/10/2025"),
        );
        let (results, _) = enrich(&[bank], &[email], MatchOptions::default());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].confidence, Confidence::High);
        assert_eq!(results[0].email_message_ids, vec!["<v@1>".to_string()]);
    }

    #[test]
    fn test_amount_tolerance() {
        let bank = mk_tx("2025-10-04", "-55.66", "VODAFONE LTD", "abc");
        let email = mk_email(
            "<v@1>",
            Some("Vodafone"),
            None,
            Some("55.67"),
            Some("Sat, 4 Oct 2025 11:04:54 -0000"),
            None,
        );
        let (results, _) = enrich(&[bank], &[email], MatchOptions::default());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].confidence, Confidence::High);
    }

    #[test]
    fn test_amount_tolerance_exceeded() {
        let bank = mk_tx("2025-10-04", "-55.50", "VODAFONE LTD", "abc");
        let email = mk_email(
            "<v@1>",
            Some("Vodafone"),
            None,
            Some("55.67"),
            Some("Sat, 4 Oct 2025 11:04:54 -0000"),
            None,
        );
        let (results, _) = enrich(&[bank], &[email], MatchOptions::default());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].confidence, Confidence::None);
    }

    #[test]
    fn test_paypal_intermediary_uses_counterparty() {
        let email = mk_email(
            "<p@1>",
            Some("PayPal"),
            Some("Netflix Services UK"),
            Some("12.99"),
            None,
            None,
        );
        assert_eq!(email.effective_vendor(), Some("Netflix Services UK"));
    }

    #[test]
    fn test_due_date_window() {
        // Bank line on due date, email received earlier — utility-bill case.
        let bank = mk_tx("2025-10-13", "-55.67", "VODAFONE", "abc");
        let email = mk_email(
            "<v@1>",
            Some("Vodafone"),
            None,
            Some("55.67"),
            Some("Sat, 4 Oct 2025 11:04:54 -0000"),
            Some("13/10/2025"),
        );
        let (results, _) = enrich(&[bank], &[email], MatchOptions::default());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].confidence, Confidence::High);
    }
}
