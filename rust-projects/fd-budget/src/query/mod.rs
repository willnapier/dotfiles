//! Stage 2 query commands: aggregate spend, vendor drill-down, and unmatched-rows
//! reporting over the (transactions.csv + bills.jsonl + matches.jsonl) triple.
//!
//! All three commands consume the three sources in memory and render plaintext
//! tables to stdout. They share the join logic in [`load_joined`].

use crate::enrich::{self, EmailRow};
use crate::Transaction;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

// ---------------------------------------------------------------------------
// matches.jsonl loader
// ---------------------------------------------------------------------------

/// One row from `matches.jsonl`, parsed back into memory.
///
/// We deliberately re-parse the file rather than re-running enrich here:
/// the file is the canonical join product, and downstream queries should be
/// stable across enrich-time changes.
#[derive(Debug, Clone, Deserialize)]
pub struct MatchRow {
    pub bank_import_id: String,
    pub confidence: String,
    #[serde(default)]
    pub email_message_ids: Vec<String>,
}

pub fn load_matches<P: AsRef<Path>>(path: P) -> std::io::Result<Vec<MatchRow>> {
    let file = File::open(path.as_ref())?;
    let reader = BufReader::new(file);
    let mut rows = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Skip silently on parse failure — JSONL conventionally tolerates bad lines.
        if let Ok(row) = serde_json::from_str::<MatchRow>(trimmed) {
            rows.push(row);
        }
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Joined view
// ---------------------------------------------------------------------------

/// A bank row joined to its match record and (resolved) email evidence.
#[derive(Debug, Clone)]
pub struct JoinedRow<'a> {
    pub tx: &'a Transaction,
    pub confidence: &'a str,
    pub emails: Vec<&'a EmailRow>,
}

impl<'a> JoinedRow<'a> {
    pub fn is_internal_transfer(&self) -> bool {
        self.confidence == "internal-transfer"
    }

    pub fn has_evidence(&self) -> bool {
        !self.emails.is_empty()
    }

    /// The resolved counterparty for this row.
    ///
    /// Rules:
    /// - If any email evidence: use the first email's `effective_vendor()`
    ///   (utility row → `vendor`; PayPal row → `counterparty`).
    /// - Otherwise: normalise the bank `description` (uppercase, collapse
    ///   whitespace, first `~25` chars).
    pub fn counterparty_name(&self) -> String {
        if let Some(email) = self.emails.first() {
            if let Some(v) = email.effective_vendor() {
                return v.to_string();
            }
        }
        normalise_description(&self.tx.description)
    }

    /// Source label for reporting.
    pub fn source(&self) -> Source {
        match self.emails.first() {
            None => Source::BankOnly,
            Some(e) if e.is_paypal() => Source::EmailViaPayPal,
            Some(_) => Source::EmailDirect,
        }
    }
}

/// Reported provenance category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    EmailDirect,
    EmailViaPayPal,
    BankOnly,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::EmailDirect => "email-direct",
            Source::EmailViaPayPal => "email-via-PayPal",
            Source::BankOnly => "bank-only",
        }
    }
}

/// Uppercase + collapse whitespace + truncate to 25 chars.
pub fn normalise_description(desc: &str) -> String {
    let collapsed: String = desc
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_uppercase();
    collapsed.chars().take(25).collect()
}

/// Build the joined dataset from the three source vectors.
pub fn join<'a>(
    transactions: &'a [Transaction],
    emails: &'a [EmailRow],
    matches: &'a [MatchRow],
) -> Vec<JoinedRow<'a>> {
    let by_msg_id: HashMap<&str, &EmailRow> =
        emails.iter().map(|e| (e.message_id.as_str(), e)).collect();
    let by_match_id: HashMap<&str, &MatchRow> = matches
        .iter()
        .map(|m| (m.bank_import_id.as_str(), m))
        .collect();

    transactions
        .iter()
        .map(|tx| {
            let m = by_match_id.get(tx.import_id.as_str());
            let confidence = m.map(|r| r.confidence.as_str()).unwrap_or("none");
            let email_ids = m.map(|r| r.email_message_ids.as_slice()).unwrap_or(&[]);
            let emails: Vec<&EmailRow> = email_ids
                .iter()
                .filter_map(|id| by_msg_id.get(id.as_str()).copied())
                .collect();
            JoinedRow {
                tx,
                confidence,
                emails,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Date filter
// ---------------------------------------------------------------------------

/// CLI-supplied date range.
#[derive(Debug, Default, Clone, Copy)]
pub struct DateFilter {
    pub since: Option<NaiveDate>,
    pub until: Option<NaiveDate>,
}

impl DateFilter {
    /// Year filter: `YYYY` → 1 Jan to 31 Dec.
    pub fn year(year: i32) -> Self {
        Self {
            since: NaiveDate::from_ymd_opt(year, 1, 1),
            until: NaiveDate::from_ymd_opt(year, 12, 31),
        }
    }

    /// Month filter: `YYYY-MM` → first of month to last of month.
    pub fn month(s: &str) -> anyhow::Result<Self> {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() != 2 {
            anyhow::bail!("--month must be YYYY-MM (got {})", s);
        }
        let y: i32 = parts[0].parse()?;
        let m: u32 = parts[1].parse()?;
        let since = NaiveDate::from_ymd_opt(y, m, 1)
            .ok_or_else(|| anyhow::anyhow!("invalid month {}", s))?;
        // Last day = first of next month minus one.
        let next_first = if m == 12 {
            NaiveDate::from_ymd_opt(y + 1, 1, 1)
        } else {
            NaiveDate::from_ymd_opt(y, m + 1, 1)
        }
        .ok_or_else(|| anyhow::anyhow!("invalid month {}", s))?;
        let until = next_first.pred_opt().unwrap();
        Ok(Self {
            since: Some(since),
            until: Some(until),
        })
    }

    /// Resolve `--year`, `--month`, `--since` flags into a single filter.
    /// `--month` overrides `--year`; `--since` further constrains the start.
    pub fn from_flags(
        year: Option<i32>,
        month: Option<&str>,
        since: Option<NaiveDate>,
    ) -> anyhow::Result<Self> {
        let mut filter = match (month, year) {
            (Some(m), _) => Self::month(m)?,
            (None, Some(y)) => Self::year(y),
            (None, None) => Self::default(),
        };
        if let Some(s) = since {
            filter.since = Some(match filter.since {
                Some(existing) if existing > s => existing,
                _ => s,
            });
        }
        Ok(filter)
    }

    pub fn matches(&self, date: NaiveDate) -> bool {
        if let Some(s) = self.since {
            if date < s {
                return false;
            }
        }
        if let Some(u) = self.until {
            if date > u {
                return false;
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Command 2a: stats --by-counterparty
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
struct CounterpartyAggregate {
    name: String,
    total_outgoing: Decimal, // Sum of |amount| for debits.
    count: usize,
    source: Source,
}

impl Default for Source {
    fn default() -> Self {
        Source::BankOnly
    }
}

/// Aggregate outgoing spend by counterparty.
///
/// Rules:
/// - internal-transfer rows are excluded entirely
/// - credit (positive amount) rows are excluded from the aggregate
/// - non-GBP email rows: their evidence is dropped (so the row falls back to
///   bank-only/normalised-description), but the bank amount is still counted
///
/// Return: (aggregate sorted by total descending, internal_transfer_count)
fn aggregate_by_counterparty(
    rows: &[JoinedRow<'_>],
    filter: DateFilter,
) -> (Vec<CounterpartyAggregate>, usize, Decimal, Decimal) {
    let mut buckets: HashMap<String, CounterpartyAggregate> = HashMap::new();
    let mut internal_count = 0usize;
    let mut reconciled_total = Decimal::ZERO;
    let mut bank_only_total = Decimal::ZERO;

    for row in rows {
        if !filter.matches(row.tx.date) {
            continue;
        }
        if row.is_internal_transfer() {
            internal_count += 1;
            continue;
        }
        // Aggregate is outgoing-spend only.
        if !row.tx.is_debit() {
            continue;
        }

        // Filter non-GBP email evidence: a non-GBP email shouldn't dictate
        // the counterparty since amounts are in mismatched units.
        let gbp_emails: Vec<&EmailRow> =
            row.emails.iter().filter(|e| e.is_gbp()).copied().collect();

        // Skip a row if its only evidence was non-GBP — fall back to bank-only.
        let (counterparty, source) = if let Some(email) = gbp_emails.first() {
            let name = email
                .effective_vendor()
                .map(String::from)
                .unwrap_or_else(|| normalise_description(&row.tx.description));
            let src = if email.is_paypal() {
                Source::EmailViaPayPal
            } else {
                Source::EmailDirect
            };
            (name, src)
        } else {
            (
                normalise_description(&row.tx.description),
                Source::BankOnly,
            )
        };

        let amount = row.tx.amount.abs();
        let entry = buckets
            .entry(counterparty.clone())
            .or_insert_with(|| CounterpartyAggregate {
                name: counterparty,
                total_outgoing: Decimal::ZERO,
                count: 0,
                source,
            });
        entry.total_outgoing += amount;
        entry.count += 1;
        // First-seen-source wins; if a row mixes sources we keep the original.
        // (TODO: in practice a vendor name should map to one source — flag if not.)

        match source {
            Source::EmailDirect | Source::EmailViaPayPal => reconciled_total += amount,
            Source::BankOnly => bank_only_total += amount,
        }
    }

    let mut out: Vec<CounterpartyAggregate> = buckets.into_values().collect();
    out.sort_by(|a, b| b.total_outgoing.cmp(&a.total_outgoing));
    (out, internal_count, reconciled_total, bank_only_total)
}

/// Print a stats-by-counterparty table.
pub fn cmd_stats_by_counterparty(
    rows: &[JoinedRow<'_>],
    filter: DateFilter,
    limit: usize,
) -> anyhow::Result<()> {
    let (agg, internal_count, reconciled_total, bank_only_total) =
        aggregate_by_counterparty(rows, filter);

    println!(
        "{:<32} {:>12}   {:>5}   {}",
        "Vendor", "Total", "Count", "Source"
    );
    println!("{}", "-".repeat(72));
    for entry in agg.iter().take(limit) {
        println!(
            "{:<32} {:>12}   {:>5}   {}",
            truncate(&entry.name, 32),
            format_money(entry.total_outgoing),
            entry.count,
            entry.source.as_str()
        );
    }

    println!("{}", "-".repeat(72));
    println!(
        "{:<32} {:>12}",
        "Total reconciled",
        format_money(reconciled_total)
    );
    println!(
        "{:<32} {:>12}",
        "Total bank-only",
        format_money(bank_only_total)
    );
    println!(
        "{:<32} {:>5} rows",
        "Internal transfers excluded", internal_count
    );

    if agg.len() > limit {
        println!("({} more rows below limit)", agg.len() - limit);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Command 2b: tx --vendor <NAME> --with-evidence
// ---------------------------------------------------------------------------

pub fn cmd_tx_by_vendor(
    rows: &[JoinedRow<'_>],
    filter: DateFilter,
    vendor: &str,
    with_evidence: bool,
) -> anyhow::Result<()> {
    let needle = vendor.to_lowercase();
    let matches: Vec<&JoinedRow> = rows
        .iter()
        .filter(|r| filter.matches(r.tx.date))
        .filter(|r| !r.is_internal_transfer())
        .filter(|r| r.counterparty_name().to_lowercase().contains(&needle))
        .collect();

    let total = matches.len();
    let with_ev = matches.iter().filter(|r| r.has_evidence()).count();

    for row in &matches {
        if with_evidence {
            let evidence = if let Some(email) = row.emails.first() {
                let mid = truncate_msg_id(&email.message_id, 36);
                format!(
                    "-> {:<38} {:<10}",
                    mid,
                    row.confidence
                )
            } else {
                format!("{:<41} {}", "(no email evidence)", row.confidence)
            };
            println!(
                "{}  {:>9}   {:<28}  {}",
                row.tx.date,
                format_money(row.tx.amount),
                truncate(&row.tx.description, 28),
                evidence
            );
        } else {
            println!(
                "{}  {:>9}   {}",
                row.tx.date,
                format_money(row.tx.amount),
                row.tx.description
            );
        }
    }

    if total == 0 {
        println!("(no rows match vendor '{}')", vendor);
    } else if with_evidence {
        let pct = if total > 0 {
            (with_ev as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        println!(
            "\n{} rows, {} with evidence ({:.1}%)",
            total, with_ev, pct
        );
    } else {
        println!("\n{} rows", total);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Command 2c: tx unmatched [--over <AMOUNT>]
// ---------------------------------------------------------------------------

pub fn cmd_tx_unmatched(
    rows: &[JoinedRow<'_>],
    filter: DateFilter,
    over: Option<Decimal>,
) -> anyhow::Result<()> {
    // Outgoing-spend focus: the spec example output is "find big things I
    // should have evidence for but don't", which is debit-focused. Credits
    // (refunds, incoming transfers) rarely need email evidence in this
    // workflow, and including them swamps the list.
    let unmatched: Vec<&JoinedRow> = rows
        .iter()
        .filter(|r| filter.matches(r.tx.date))
        .filter(|r| r.confidence == "none")
        .filter(|r| r.tx.is_debit())
        .collect();
    let total_unmatched = unmatched.len();

    let threshold = over.unwrap_or(Decimal::ZERO);
    let mut shown: Vec<&JoinedRow> = unmatched
        .iter()
        .filter(|r| r.tx.amount.abs() >= threshold)
        .copied()
        .collect();
    // Sort by absolute amount descending.
    shown.sort_by(|a, b| b.tx.amount.abs().cmp(&a.tx.amount.abs()));

    for row in &shown {
        println!(
            "{}  {:>9}   {}",
            row.tx.date,
            format_money(row.tx.amount),
            row.tx.description
        );
    }

    let below = total_unmatched.saturating_sub(shown.len());
    println!(
        "\n{} rows shown (of {} unmatched, {} below threshold)",
        shown.len(),
        total_unmatched,
        below
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

/// Format a decimal amount as GBP with sign preserved: "-£55.67", "£12.99".
fn format_money(amount: Decimal) -> String {
    if amount.is_sign_negative() {
        format!("-£{:.2}", amount.abs())
    } else {
        format!("£{:.2}", amount)
    }
}

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_string()
    } else {
        s.chars().take(width).collect()
    }
}

fn truncate_msg_id(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_string()
    } else {
        // Keep the leading "<" plus a prefix and ellipsis.
        let prefix: String = s.chars().take(width.saturating_sub(4)).collect();
        format!("{}...", prefix)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Account, TxType};
    use std::str::FromStr;

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
        currency: Option<&str>,
    ) -> EmailRow {
        EmailRow {
            message_id: msg_id.to_string(),
            vendor: vendor.map(String::from),
            counterparty: counterparty.map(String::from),
            amount: amount.and_then(|s| Decimal::from_str(s).ok()),
            received_date: None,
            due_date: None,
            direction: None,
            policy: None,
            currency: currency.map(String::from),
        }
    }

    fn mk_match(import_id: &str, confidence: &str, msg_ids: &[&str]) -> MatchRow {
        MatchRow {
            bank_import_id: import_id.to_string(),
            confidence: confidence.to_string(),
            email_message_ids: msg_ids.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn counterparty_uses_email_vendor_for_utility() {
        let tx = mk_tx("2025-10-13", "-55.67", "VODAFONE LTD DD", "tx1");
        let email = mk_email("<v@1>", Some("Vodafone"), None, Some("55.67"), None);
        let m = mk_match("tx1", "high", &["<v@1>"]);
        let txs = vec![tx];
        let emails = vec![email];
        let ms = vec![m];
        let joined = join(&txs, &emails, &ms);
        assert_eq!(joined[0].counterparty_name(), "Vodafone");
    }

    #[test]
    fn counterparty_uses_email_counterparty_for_paypal() {
        let tx = mk_tx("2025-10-13", "-130.87", "PAYPAL *DROPBOXIN", "tx1");
        let email = mk_email(
            "<p@1>",
            Some("PayPal"),
            Some("Dropbox International"),
            Some("130.87"),
            Some("GBP"),
        );
        let m = mk_match("tx1", "high", &["<p@1>"]);
        let txs = vec![tx];
        let emails = vec![email];
        let ms = vec![m];
        let joined = join(&txs, &emails, &ms);
        assert_eq!(joined[0].counterparty_name(), "Dropbox International");
        assert_eq!(joined[0].source(), Source::EmailViaPayPal);
    }

    #[test]
    fn counterparty_falls_back_to_normalised_description() {
        let tx = mk_tx("2025-10-13", "-12.50", "TFL TRAVEL CH", "tx1");
        // No match row at all.
        let txs = vec![tx];
        let emails: Vec<EmailRow> = vec![];
        let ms: Vec<MatchRow> = vec![];
        let joined = join(&txs, &emails, &ms);
        assert_eq!(joined[0].counterparty_name(), "TFL TRAVEL CH");
        assert_eq!(joined[0].source(), Source::BankOnly);
    }

    #[test]
    fn aggregate_excludes_non_gbp_email_evidence() {
        // Cliniko bills are in AUD; matching to a GBP bank row should fall
        // back to bank-only categorisation.
        let tx = mk_tx("2025-03-15", "-23.45", "CLINIKO PTY LTD", "tx1");
        let email = mk_email("<c@1>", Some("Cliniko"), None, Some("45.00"), Some("AUD"));
        let m = mk_match("tx1", "medium", &["<c@1>"]);
        let txs = vec![tx];
        let emails = vec![email];
        let ms = vec![m];
        let joined = join(&txs, &emails, &ms);
        let (agg, _, reconciled, bank_only) =
            aggregate_by_counterparty(&joined, DateFilter::default());
        assert_eq!(agg.len(), 1);
        assert_eq!(agg[0].source, Source::BankOnly);
        assert_eq!(agg[0].name, "CLINIKO PTY LTD");
        assert_eq!(reconciled, Decimal::ZERO);
        assert_eq!(bank_only, Decimal::from_str("23.45").unwrap());
    }

    #[test]
    fn aggregate_excludes_internal_transfer() {
        let tx_normal = mk_tx("2025-06-13", "-55.67", "VODAFONE LTD", "tx1");
        let tx_xfer = mk_tx("2025-06-23", "-46.07", "FIRST DIRECT VISA", "tx2");
        let email = mk_email("<v@1>", Some("Vodafone"), None, Some("55.67"), Some("GBP"));
        let ms = vec![
            mk_match("tx1", "high", &["<v@1>"]),
            mk_match("tx2", "internal-transfer", &[]),
        ];
        let txs = vec![tx_normal, tx_xfer];
        let emails = vec![email];
        let joined = join(&txs, &emails, &ms);
        let (agg, internal_count, _, _) =
            aggregate_by_counterparty(&joined, DateFilter::default());
        assert_eq!(internal_count, 1);
        assert_eq!(agg.len(), 1);
        assert_eq!(agg[0].name, "Vodafone");
    }

    #[test]
    fn aggregate_excludes_credits() {
        // Refunds (positive amount) should not appear in outgoing spend totals.
        let tx_debit = mk_tx("2025-06-13", "-55.67", "VODAFONE LTD", "tx1");
        let tx_credit = mk_tx("2025-07-13", "20.00", "VODAFONE REFUND", "tx2");
        let txs = vec![tx_debit, tx_credit];
        let emails: Vec<EmailRow> = vec![];
        let ms: Vec<MatchRow> = vec![];
        let joined = join(&txs, &emails, &ms);
        let (agg, _, _, bank_only) =
            aggregate_by_counterparty(&joined, DateFilter::default());
        let total: Decimal = agg.iter().map(|a| a.total_outgoing).sum();
        // Only the debit contributes.
        assert_eq!(total, Decimal::from_str("55.67").unwrap());
        assert_eq!(bank_only, Decimal::from_str("55.67").unwrap());
    }

    #[test]
    fn date_filter_year_includes_only_target_year() {
        let f = DateFilter::year(2025);
        assert!(f.matches(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()));
        assert!(f.matches(NaiveDate::from_ymd_opt(2025, 12, 31).unwrap()));
        assert!(!f.matches(NaiveDate::from_ymd_opt(2024, 12, 31).unwrap()));
        assert!(!f.matches(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()));
    }

    #[test]
    fn date_filter_month_inclusive() {
        let f = DateFilter::month("2025-02").unwrap();
        assert!(f.matches(NaiveDate::from_ymd_opt(2025, 2, 1).unwrap()));
        assert!(f.matches(NaiveDate::from_ymd_opt(2025, 2, 28).unwrap()));
        assert!(!f.matches(NaiveDate::from_ymd_opt(2025, 1, 31).unwrap()));
        assert!(!f.matches(NaiveDate::from_ymd_opt(2025, 3, 1).unwrap()));
    }

    #[test]
    fn date_filter_year_aggregate() {
        // 2025 row should appear; 2024 row should not.
        let tx_2024 = mk_tx("2024-06-13", "-55.67", "VODAFONE LTD", "tx1");
        let tx_2025 = mk_tx("2025-06-13", "-55.67", "VODAFONE LTD", "tx2");
        let txs = vec![tx_2024, tx_2025];
        let emails: Vec<EmailRow> = vec![];
        let ms: Vec<MatchRow> = vec![];
        let joined = join(&txs, &emails, &ms);
        let (agg, _, _, _) = aggregate_by_counterparty(&joined, DateFilter::year(2025));
        assert_eq!(agg.len(), 1);
        assert_eq!(agg[0].count, 1);
    }

    #[test]
    fn unmatched_filter_over_threshold() {
        let tx_big = mk_tx("2025-06-13", "-237.45", "POSITIVE INTERLONDON", "tx1");
        let tx_small = mk_tx("2025-06-13", "-12.50", "TFL TRAVEL CH", "tx2");
        let txs = vec![tx_big, tx_small];
        let emails: Vec<EmailRow> = vec![];
        let ms: Vec<MatchRow> = vec![]; // both unmatched (treated as "none")
        let joined = join(&txs, &emails, &ms);

        let unmatched: Vec<&JoinedRow> = joined
            .iter()
            .filter(|r| r.confidence == "none")
            .collect();
        assert_eq!(unmatched.len(), 2);

        let over_50: Vec<&JoinedRow> = unmatched
            .iter()
            .filter(|r| r.tx.amount.abs() >= Decimal::from(50))
            .copied()
            .collect();
        assert_eq!(over_50.len(), 1);
        assert_eq!(over_50[0].tx.import_id, "tx1");
    }

    #[test]
    fn normalise_description_uppercase_and_collapse() {
        assert_eq!(normalise_description("tfl  travel ch"), "TFL TRAVEL CH");
        assert_eq!(
            normalise_description("a b c d e f g h i j k l m n o p q r"),
            "A B C D E F G H I J K L M"
        );
    }

    #[test]
    fn date_filter_since_overrides_year_when_later() {
        // --since constrains start; if --since is later than --year start, it wins.
        let f = DateFilter::from_flags(
            Some(2025),
            None,
            NaiveDate::from_ymd_opt(2025, 6, 1),
        )
        .unwrap();
        assert!(!f.matches(NaiveDate::from_ymd_opt(2025, 5, 31).unwrap()));
        assert!(f.matches(NaiveDate::from_ymd_opt(2025, 6, 1).unwrap()));
        assert!(f.matches(NaiveDate::from_ymd_opt(2025, 12, 31).unwrap()));
    }
}

// ---------------------------------------------------------------------------
// Top-level loaders for use from main.rs
// ---------------------------------------------------------------------------

/// Convenience: load all three sources from disk in one go.
pub fn load_all(
    transactions_path: &Path,
    bills_path: &Path,
    matches_path: &Path,
) -> anyhow::Result<(Vec<Transaction>, Vec<EmailRow>, Vec<MatchRow>)> {
    let store = crate::store::CsvStore::new(transactions_path);
    let txs = store
        .load_all()
        .map_err(|e| anyhow::anyhow!("failed to load transactions: {e}"))?;
    let emails = enrich::load_email_rows(bills_path)
        .map_err(|e| anyhow::anyhow!("failed to load {}: {}", bills_path.display(), e))?;
    let matches = load_matches(matches_path)
        .map_err(|e| anyhow::anyhow!("failed to load {}: {}", matches_path.display(), e))?;
    Ok((txs, emails, matches))
}
