//! Subscriptions audit (feature P3).
//!
//! Detects recurring same-merchant / same-amount outgoings and reports them as
//! subscriptions, with an annualised cost so the total drag of recurring
//! commitments is visible at a glance. The real value: surfacing *forgotten* or
//! *duplicate* subscriptions — e.g. two distinct £12.99 streams (a video service
//! and a music service) that are easy to miss in the spending tail.
//!
//! Self-contained: loads via [`crate::store::CsvStore`], reuses
//! [`crate::query::normalise_description`] for the merchant key, and reuses
//! [`crate::query::DateFilter`] for the standard `--year`/`--month`/`--since`
//! window. No mutation of `transactions.csv`.
//!
//! ## Heuristic (in brief)
//! - Consider **debits** only (outgoing money). Group by
//!   `(normalised-merchant, amount-to-the-penny)`. We deliberately group on the
//!   *exact* amount rather than a tolerance band: subscriptions hold a fixed
//!   price, and an exact key is what lets us catch two distinct £12.99 streams
//!   (a band would merge a £12.99 and a £13.49 line and defeat the
//!   duplicate-detection goal). Price changes show up — correctly — as the same
//!   merchant appearing at two prices, which the review section flags.
//! - Sort each group's dates, compute consecutive day-gaps, take the **median**
//!   gap. Classify monthly (median 25..=35 days, >= 3 occurrences) or annual
//!   (median 350..=380 days, >= 2 occurrences). The median absorbs a single
//!   doubled gap, so detection survives one missing month.
//! - **Duplicate review**: after detection, flag (a) the same monetary amount
//!   billed by two *distinct* detected merchants (the two-£12.99 case) and (b)
//!   the same merchant detected at two *distinct* prices (a likely price change
//!   or an overlapping plan).

use crate::query::{normalise_description, DateFilter};
use crate::Transaction;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use std::collections::BTreeMap;

/// Detected billing cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadence {
    Monthly,
    Annual,
}

impl Cadence {
    pub fn as_str(&self) -> &'static str {
        match self {
            Cadence::Monthly => "monthly",
            Cadence::Annual => "annual",
        }
    }

    /// Multiplier turning one charge into an annual cost.
    fn annual_multiplier(&self) -> Decimal {
        match self {
            Cadence::Monthly => Decimal::from(12),
            Cadence::Annual => Decimal::from(1),
        }
    }
}

/// Tunables for detection. `Default` matches the spec's suggested knobs.
#[derive(Debug, Clone, Copy)]
pub struct DetectOptions {
    /// Minimum occurrences for a *monthly* group to count.
    pub min_monthly: usize,
    /// Minimum occurrences for an *annual* group to count.
    pub min_annual: usize,
}

impl Default for DetectOptions {
    fn default() -> Self {
        Self {
            min_monthly: 3,
            min_annual: 2,
        }
    }
}

/// One detected subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    pub merchant: String,
    /// The (positive) per-charge amount.
    pub amount: Decimal,
    pub cadence: Cadence,
    pub occurrences: usize,
    pub first_seen: NaiveDate,
    pub last_seen: NaiveDate,
}

impl Subscription {
    /// Annualised cost: monthly charge x12, annual charge x1.
    pub fn annualised(&self) -> Decimal {
        self.amount * self.cadence.annual_multiplier()
    }
}

/// A flagged review item: a cluster worth a human glance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewFlag {
    /// Two or more *distinct* merchants billing the same amount (e.g. two
    /// separate £12.99 streams). The classic forgotten-duplicate signal.
    SameAmountDifferentMerchants {
        amount: Decimal,
        merchants: Vec<String>,
    },
    /// One merchant detected at two or more *distinct* prices (price change or
    /// overlapping plan).
    SameMerchantDifferentAmounts {
        merchant: String,
        amounts: Vec<Decimal>,
    },
}

/// Full audit output.
#[derive(Debug, Clone, Default)]
pub struct Audit {
    /// Detected subscriptions, sorted by annualised cost descending.
    pub subscriptions: Vec<Subscription>,
    /// Review flags (duplicates / suspicious clusters).
    pub flags: Vec<ReviewFlag>,
}

impl Audit {
    /// Grand total of annualised subscription spend.
    pub fn total_annualised(&self) -> Decimal {
        self.subscriptions.iter().map(|s| s.annualised()).sum()
    }
}

/// Median of a *non-empty* sorted slice of day-gaps.
fn median_gap(sorted_gaps: &[i64]) -> i64 {
    let n = sorted_gaps.len();
    if n % 2 == 1 {
        sorted_gaps[n / 2]
    } else {
        // Even count: mean of the two central values (integer-rounded).
        (sorted_gaps[n / 2 - 1] + sorted_gaps[n / 2]) / 2
    }
}

/// Classify a group of (already sorted, deduped-by-date) charges into a cadence.
///
/// Returns `None` when the spacing is irregular or there are too few
/// occurrences for either cadence.
fn classify(dates: &[NaiveDate], opts: &DetectOptions) -> Option<Cadence> {
    if dates.len() < 2 {
        return None;
    }
    let mut gaps: Vec<i64> = dates.windows(2).map(|w| (w[1] - w[0]).num_days()).collect();
    gaps.sort_unstable();
    let median = median_gap(&gaps);

    // Monthly: ~28-31 day cadence. Allow 25..=35 to tolerate billing-day drift
    // and short/long months. Median absorbs one doubled gap (a missed month).
    if (25..=35).contains(&median) && dates.len() >= opts.min_monthly {
        return Some(Cadence::Monthly);
    }
    // Annual: ~365 day cadence. Allow 350..=380 for leap years / billing drift.
    if (350..=380).contains(&median) && dates.len() >= opts.min_annual {
        return Some(Cadence::Annual);
    }
    None
}

/// Run the full subscriptions audit over a transaction set within `filter`.
pub fn audit(txs: &[Transaction], filter: DateFilter, opts: DetectOptions) -> Audit {
    // Group debit charges by (merchant, exact amount). BTreeMap keeps output
    // deterministic without an extra sort pass on the keys.
    let mut groups: BTreeMap<(String, Decimal), Vec<NaiveDate>> = BTreeMap::new();

    for tx in txs {
        if !filter.matches(tx.date) {
            continue;
        }
        // Outgoing money only.
        if !tx.is_debit() {
            continue;
        }
        let merchant = normalise_description(&tx.description);
        // Skip rows that normalise to nothing (defensive — shouldn't happen).
        if merchant.is_empty() {
            continue;
        }
        let amount = tx.amount.abs();
        groups.entry((merchant, amount)).or_default().push(tx.date);
    }

    let mut subscriptions = Vec::new();
    for ((merchant, amount), mut dates) in groups {
        dates.sort_unstable();
        // Collapse same-day duplicates: two charges on one day don't establish
        // a cadence (could be a retry / split). Cadence is about *spacing*.
        dates.dedup();
        if let Some(cadence) = classify(&dates, &opts) {
            subscriptions.push(Subscription {
                merchant,
                amount,
                cadence,
                occurrences: dates.len(),
                first_seen: *dates.first().unwrap(),
                last_seen: *dates.last().unwrap(),
            });
        }
    }

    // Sort by annualised cost descending; tie-break by merchant for stability.
    subscriptions.sort_by(|a, b| {
        b.annualised()
            .cmp(&a.annualised())
            .then_with(|| a.merchant.cmp(&b.merchant))
    });

    let flags = build_flags(&subscriptions);

    Audit {
        subscriptions,
        flags,
    }
}

/// Build the "review these" flags from the detected subscriptions.
fn build_flags(subs: &[Subscription]) -> Vec<ReviewFlag> {
    let mut flags = Vec::new();

    // (a) Same amount, distinct merchants.
    let mut by_amount: BTreeMap<Decimal, Vec<String>> = BTreeMap::new();
    for s in subs {
        let entry = by_amount.entry(s.amount).or_default();
        if !entry.contains(&s.merchant) {
            entry.push(s.merchant.clone());
        }
    }
    for (amount, mut merchants) in by_amount {
        if merchants.len() >= 2 {
            merchants.sort();
            flags.push(ReviewFlag::SameAmountDifferentMerchants { amount, merchants });
        }
    }

    // (b) Same merchant, distinct amounts.
    let mut by_merchant: BTreeMap<String, Vec<Decimal>> = BTreeMap::new();
    for s in subs {
        let entry = by_merchant.entry(s.merchant.clone()).or_default();
        if !entry.contains(&s.amount) {
            entry.push(s.amount);
        }
    }
    for (merchant, mut amounts) in by_merchant {
        if amounts.len() >= 2 {
            amounts.sort();
            flags.push(ReviewFlag::SameMerchantDifferentAmounts { merchant, amounts });
        }
    }

    flags
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Format a (positive) amount as GBP: "£12.99". Mirrors `query::format_money`
/// but always positive (charges are stored as their absolute value here).
fn fmt_gbp(amount: Decimal) -> String {
    format!("£{:.2}", amount)
}

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_string()
    } else {
        s.chars().take(width).collect()
    }
}

/// Render the audit to a `String` (so it is testable). `main` prints this.
pub fn render(audit: &Audit) -> String {
    let mut out = String::new();

    if audit.subscriptions.is_empty() {
        out.push_str("No recurring subscriptions detected.\n");
    } else {
        out.push_str(&format!(
            "{:<32} {:>9}  {:<8} {:>4}  {:<11} {:<11} {:>11}\n",
            "Merchant", "Amount", "Cadence", "Occ", "First seen", "Last seen", "Annualised"
        ));
        out.push_str(&format!("{}\n", "-".repeat(92)));
        for s in &audit.subscriptions {
            out.push_str(&format!(
                "{:<32} {:>9}  {:<8} {:>4}  {:<11} {:<11} {:>11}\n",
                truncate(&s.merchant, 32),
                fmt_gbp(s.amount),
                s.cadence.as_str(),
                s.occurrences,
                s.first_seen,
                s.last_seen,
                fmt_gbp(s.annualised()),
            ));
        }
        out.push_str(&format!("{}\n", "-".repeat(92)));
        out.push_str(&format!(
            "{:<32} {:>9}  {:<8} {:>4}  {:<11} {:<11} {:>11}\n",
            "TOTAL annualised subscription spend",
            "",
            "",
            "",
            "",
            "",
            fmt_gbp(audit.total_annualised()),
        ));
    }

    if !audit.flags.is_empty() {
        out.push('\n');
        out.push_str("Review these (possible duplicates / price changes):\n");
        for flag in &audit.flags {
            match flag {
                ReviewFlag::SameAmountDifferentMerchants { amount, merchants } => {
                    out.push_str(&format!(
                        "  ! {} billed by {} distinct merchants: {}\n",
                        fmt_gbp(*amount),
                        merchants.len(),
                        merchants.join(", "),
                    ));
                }
                ReviewFlag::SameMerchantDifferentAmounts { merchant, amounts } => {
                    let priced: Vec<String> = amounts.iter().map(|a| fmt_gbp(*a)).collect();
                    out.push_str(&format!(
                        "  ! {} appears at {} prices: {}\n",
                        merchant,
                        amounts.len(),
                        priced.join(", "),
                    ));
                }
            }
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Tests (synthetic fixtures ONLY — fictional merchants, round amounts)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Account, TxType};
    use std::str::FromStr;

    /// Build a synthetic debit transaction. Amount given as a positive magnitude
    /// for readability; stored negative (a debit) per the data model.
    fn debit(date: &str, magnitude: &str, desc: &str) -> Transaction {
        Transaction {
            date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            account: Account::Current,
            tx_type: TxType::DirectDebit,
            amount: -Decimal::from_str(magnitude).unwrap(),
            description: desc.to_string(),
            raw_description: desc.to_string(),
            balance: None,
            tags: Vec::new(),
            import_id: format!("{date}-{desc}-{magnitude}"),
        }
    }

    fn credit(date: &str, magnitude: &str, desc: &str) -> Transaction {
        Transaction {
            date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            account: Account::Current,
            tx_type: TxType::BankPayment,
            amount: Decimal::from_str(magnitude).unwrap(),
            description: desc.to_string(),
            raw_description: desc.to_string(),
            balance: None,
            tags: Vec::new(),
            import_id: format!("{date}-{desc}-credit"),
        }
    }

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn detects_monthly_subscription() {
        // Five roughly-monthly charges -> one monthly subscription.
        let txs = vec![
            debit("2025-01-15", "9.99", "STREAMFLIX"),
            debit("2025-02-14", "9.99", "STREAMFLIX"),
            debit("2025-03-16", "9.99", "STREAMFLIX"),
            debit("2025-04-15", "9.99", "STREAMFLIX"),
            debit("2025-05-15", "9.99", "STREAMFLIX"),
        ];
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert_eq!(audit.subscriptions.len(), 1);
        let s = &audit.subscriptions[0];
        assert_eq!(s.merchant, "STREAMFLIX");
        assert_eq!(s.amount, dec("9.99"));
        assert_eq!(s.cadence, Cadence::Monthly);
        assert_eq!(s.occurrences, 5);
        assert_eq!(s.first_seen, NaiveDate::from_ymd_opt(2025, 1, 15).unwrap());
        assert_eq!(s.last_seen, NaiveDate::from_ymd_opt(2025, 5, 15).unwrap());
        // Annualised = 9.99 * 12.
        assert_eq!(s.annualised(), dec("119.88"));
        assert_eq!(audit.total_annualised(), dec("119.88"));
    }

    #[test]
    fn monthly_is_robust_to_one_missing_month() {
        // March is missing -> a ~59-day gap. With a median over the other gaps
        // still ~30, this should still classify as monthly (>= 3 occurrences).
        let txs = vec![
            debit("2025-01-10", "5.00", "CLOUDBOX"),
            debit("2025-02-10", "5.00", "CLOUDBOX"),
            // (March skipped)
            debit("2025-04-10", "5.00", "CLOUDBOX"),
            debit("2025-05-10", "5.00", "CLOUDBOX"),
        ];
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert_eq!(audit.subscriptions.len(), 1);
        assert_eq!(audit.subscriptions[0].cadence, Cadence::Monthly);
        assert_eq!(audit.subscriptions[0].occurrences, 4);
    }

    #[test]
    fn detects_annual_subscription() {
        // Two charges ~365 days apart -> annual (>= 2 occurrences).
        let txs = vec![
            debit("2024-03-01", "79.00", "DOMAIN REGISTRAR"),
            debit("2025-03-02", "79.00", "DOMAIN REGISTRAR"),
        ];
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert_eq!(audit.subscriptions.len(), 1);
        let s = &audit.subscriptions[0];
        assert_eq!(s.cadence, Cadence::Annual);
        assert_eq!(s.occurrences, 2);
        // Annualised = amount * 1.
        assert_eq!(s.annualised(), dec("79.00"));
    }

    #[test]
    fn same_amount_different_merchants_is_flagged() {
        // The real-world case: two DISTINCT £12.99 streams. Both are monthly,
        // and the review section must flag the shared amount.
        let mut txs = Vec::new();
        for (i, m) in [1u32, 2, 3, 4, 5].iter().enumerate() {
            let _ = i;
            txs.push(debit(&format!("2025-{:02}-05", m), "12.99", "VIDEOSTREAM"));
            txs.push(debit(&format!("2025-{:02}-20", m), "12.99", "MUSICPREMIUM"));
        }
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert_eq!(audit.subscriptions.len(), 2);

        let flagged: Vec<&ReviewFlag> = audit
            .flags
            .iter()
            .filter(|f| matches!(f, ReviewFlag::SameAmountDifferentMerchants { .. }))
            .collect();
        assert_eq!(flagged.len(), 1, "expected exactly one same-amount flag");
        if let ReviewFlag::SameAmountDifferentMerchants { amount, merchants } = flagged[0] {
            assert_eq!(*amount, dec("12.99"));
            assert_eq!(merchants.len(), 2);
            assert!(merchants.contains(&"VIDEOSTREAM".to_string()));
            assert!(merchants.contains(&"MUSICPREMIUM".to_string()));
        } else {
            unreachable!();
        }
    }

    #[test]
    fn non_recurring_merchant_is_not_flagged() {
        // Irregular one-off shopping at the same merchant, varying amounts and
        // erratic spacing -> NOT a subscription, and no flags.
        let txs = vec![
            debit("2025-01-03", "23.40", "CORNER SHOP"),
            debit("2025-01-19", "8.10", "CORNER SHOP"),
            debit("2025-03-02", "55.00", "CORNER SHOP"),
            debit("2025-03-04", "12.00", "CORNER SHOP"),
            debit("2025-06-30", "7.25", "CORNER SHOP"),
        ];
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert!(
            audit.subscriptions.is_empty(),
            "irregular merchant should not be detected as a subscription"
        );
        assert!(audit.flags.is_empty());
    }

    #[test]
    fn credits_are_ignored() {
        // A monthly *incoming* payment (e.g. salary) must never be a subscription.
        let txs = vec![
            credit("2025-01-28", "2000.00", "EMPLOYER PAYROLL"),
            credit("2025-02-28", "2000.00", "EMPLOYER PAYROLL"),
            credit("2025-03-28", "2000.00", "EMPLOYER PAYROLL"),
            credit("2025-04-28", "2000.00", "EMPLOYER PAYROLL"),
        ];
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert!(audit.subscriptions.is_empty());
    }

    #[test]
    fn same_merchant_two_prices_is_flagged() {
        // A price rise mid-stream: same merchant, two distinct prices. Both
        // sub-streams need >= 3 occurrences to detect; then the merchant is
        // flagged as appearing at two prices.
        let txs = vec![
            // Old price.
            debit("2024-07-12", "10.00", "NEWSDAILY"),
            debit("2024-08-12", "10.00", "NEWSDAILY"),
            debit("2024-09-12", "10.00", "NEWSDAILY"),
            // New price.
            debit("2025-01-12", "12.00", "NEWSDAILY"),
            debit("2025-02-12", "12.00", "NEWSDAILY"),
            debit("2025-03-12", "12.00", "NEWSDAILY"),
        ];
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        // Two detected sub-streams for the same merchant at two prices.
        assert_eq!(audit.subscriptions.len(), 2);
        let flagged: Vec<&ReviewFlag> = audit
            .flags
            .iter()
            .filter(|f| matches!(f, ReviewFlag::SameMerchantDifferentAmounts { .. }))
            .collect();
        assert_eq!(flagged.len(), 1);
        if let ReviewFlag::SameMerchantDifferentAmounts { merchant, amounts } = flagged[0] {
            assert_eq!(merchant, "NEWSDAILY");
            assert_eq!(amounts, &vec![dec("10.00"), dec("12.00")]);
        } else {
            unreachable!();
        }
    }

    #[test]
    fn min_occurrences_knob_raises_the_bar() {
        // Three monthly charges detect by default; raising min_monthly to 4
        // should reject them.
        let txs = vec![
            debit("2025-01-15", "9.99", "STREAMFLIX"),
            debit("2025-02-15", "9.99", "STREAMFLIX"),
            debit("2025-03-15", "9.99", "STREAMFLIX"),
        ];
        let default = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert_eq!(default.subscriptions.len(), 1);

        let stricter = audit(
            &txs,
            DateFilter::default(),
            DetectOptions {
                min_monthly: 4,
                min_annual: 2,
            },
        );
        assert!(stricter.subscriptions.is_empty());
    }

    #[test]
    fn sorted_by_annualised_cost_descending() {
        // A cheap monthly (annualised 60) vs an expensive annual (annualised
        // 200): annual sorts first.
        let mut txs = vec![
            debit("2024-06-01", "200.00", "INSURANCE CO"),
            debit("2025-06-01", "200.00", "INSURANCE CO"),
        ];
        for m in 1u32..=4 {
            txs.push(debit(&format!("2025-{:02}-10", m), "5.00", "GYMLITE"));
        }
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert_eq!(audit.subscriptions.len(), 2);
        assert_eq!(audit.subscriptions[0].merchant, "INSURANCE CO");
        assert_eq!(audit.subscriptions[0].annualised(), dec("200.00"));
        assert_eq!(audit.subscriptions[1].merchant, "GYMLITE");
        assert_eq!(audit.subscriptions[1].annualised(), dec("60.00"));
    }

    #[test]
    fn date_filter_since_excludes_earlier_charges() {
        // Two streams; --since drops the early-only stream below the occurrence
        // floor.
        let mut txs = Vec::new();
        for m in 1u32..=5 {
            txs.push(debit(&format!("2025-{:02}-15", m), "9.99", "STREAMFLIX"));
        }
        let filter = DateFilter {
            since: NaiveDate::from_ymd_opt(2025, 4, 1),
            until: None,
        };
        let audit = audit(&txs, filter, DetectOptions::default());
        // Only April + May remain -> 2 occurrences, below monthly floor of 3.
        assert!(audit.subscriptions.is_empty());
    }

    #[test]
    fn render_contains_subscription_and_flag() {
        let mut txs = Vec::new();
        for m in 1u32..=5 {
            txs.push(debit(&format!("2025-{:02}-05", m), "12.99", "VIDEOSTREAM"));
            txs.push(debit(&format!("2025-{:02}-20", m), "12.99", "MUSICPREMIUM"));
        }
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        let text = render(&audit);
        assert!(text.contains("VIDEOSTREAM"));
        assert!(text.contains("£12.99"));
        assert!(text.contains("monthly"));
        assert!(text.contains("TOTAL annualised"));
        assert!(text.contains("Review these"));
        assert!(text.contains("distinct merchants"));
    }
}
