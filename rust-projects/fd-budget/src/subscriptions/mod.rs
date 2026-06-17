//! Subscriptions audit (feature P3).
//!
//! Detects recurring same-merchant / same-amount outgoings and reports them as
//! subscriptions, with an annualised cost so the total drag of recurring
//! commitments is visible at a glance. The real value: surfacing *forgotten* or
//! *duplicate* subscriptions — e.g. two distinct £12.99 streams (a video service
//! and a music service) that are easy to miss in the spending tail.
//!
//! Self-contained: loads via [`crate::store::CsvStore`], derives the merchant
//! grouping key with [`canonical_merchant`] (a conservative normaliser that
//! collapses cosmetic variants of one merchant — branch town, masked card tail,
//! payment-gateway stub — without merging distinct merchants), optionally
//! resolves a bare `PAYPAL PAYMENT` row to its recovered real merchant
//! (via [`crate::paypal::RecoveryIndex`]), and reuses [`crate::query::DateFilter`]
//! for the standard `--year`/`--month`/`--since` window. No mutation of
//! `transactions.csv`.
//!
//! ## Heuristic (in brief)
//! - Consider **debits** only (outgoing money). Group by the **canonical
//!   merchant alone** — NOT `(merchant, exact amount)`. A subscription whose GBP
//!   price wobbles month-to-month (FX-priced services) would otherwise split
//!   into several phantom "subscriptions"; grouping by merchant collapses those
//!   into ONE, with a *representative* amount (the most common charge) and an
//!   amount *range* (min–max) when the price varies.
//! - **PayPal recovery**: a bare bank `PAYPAL PAYMENT` row carries no merchant.
//!   When a [`crate::paypal::RecoveryIndex`] is supplied and has a recovered
//!   merchant for the row's `import_id`, that recovered name (canonicalised) is
//!   used as the grouping key instead of the literal "PAYPAL PAYMENT" — so
//!   recovered PayPal subscriptions become visible. Rows with no recovery keep
//!   the raw key.
//! - Sort each merchant group's dates, compute consecutive day-gaps, take the
//!   **median** gap. Classify monthly (median 25..=35 days, >= 3 occurrences) or
//!   annual (median 350..=380 days, >= 2 occurrences). The median absorbs a
//!   single doubled gap, so detection survives one missing month.
//! - **Duplicate review**: after detection, flag (a) the same *representative*
//!   amount billed by two *distinct* merchants (the two-£12.99 case — still a
//!   useful cross-check, since distinct merchants are now distinct keys) and (b)
//!   — purely **informational** — a single merchant whose amount range is *wide*
//!   (a likely real price change). Case (b) is no longer two subscriptions: it
//!   is ONE subscription with a price range, noted but never double-counted.

use crate::paypal::RecoveryIndex;
use crate::query::DateFilter;
use crate::Transaction;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use std::collections::BTreeMap;

/// A merchant's amount range is "wide" — and therefore worth an informational
/// price-change note — when `max - min` exceeds this fraction of the
/// representative amount. 0.10 = a >10% spread. Deliberately a *relative*
/// threshold so a £2 service and a £200 service are judged on the same footing,
/// and small FX wobble (a few pence on a £10 charge) does NOT trip the note.
const WIDE_RANGE_FRACTION: f64 = 0.10;

// ---------------------------------------------------------------------------
// Conservative merchant-name canonicalisation
// ---------------------------------------------------------------------------

/// Trailing location tokens seen in the real data that are pure branch/town
/// noise appended after a merchant name (e.g. `AUDIBLE UK LONDON`). Kept
/// deliberately SMALL and only matched as a *whole trailing token*, never as a
/// substring and never mid-name — so `GOOGLE YOUTUBE MEMLONDON` (which embeds
/// "LONDON" inside `MEMLONDON`) and `GOOGLE GSUITE_WILLDUBLIN` are NOT touched.
///
/// Refinable: add a town here only after confirming it is genuine trailing
/// branch noise on more than one real merchant, never a merchant name itself.
const TRAILING_LOCATIONS: &[&str] = &["LONDON", "CORK", "DUBLIN", "SWINDON", "LUTON", "CHARD"];

/// Is `tok` pure mask / numeric noise? True for runs of `*` (`**********`,
/// `***`) and tokens made up entirely of `*`, digits, and ASCII punctuation
/// (no letters), e.g. masked card tails. Such a token carries no merchant
/// identity, so when TRAILING it is dropped.
fn is_mask_noise(tok: &str) -> bool {
    !tok.is_empty()
        && tok
            .chars()
            .all(|c| c == '*' || c.is_ascii_digit() || c.is_ascii_punctuation())
}

/// Is `tok` a trailing payment-gateway / processor fragment? These are the
/// "how it was paid" tails appended after the merchant, e.g. `APPLE.COM/`,
/// `ADBL.CO/PYMT`, `.CO/PYMT`, `/PYMT`. Heuristic, intentionally narrow:
/// the token must contain a `/` AND either end in `/` (a bare gateway stub like
/// `APPLE.COM/`) or carry a known processor marker (`PYMT`/`PAYM`/`BILL/` style
/// tails). A token that merely contains `/` mid-name is NOT matched here — the
/// real core `APPLE.COM/BILL` is protected both by this narrowness and by the
/// "never peel the last remaining token" rule below.
///
/// Refinable: extend the marker set as new processor tails appear in the data.
fn is_gateway_tail(tok: &str) -> bool {
    if !tok.contains('/') {
        return false;
    }
    // Bare gateway stub: ends with a slash, e.g. `APPLE.COM/`.
    if tok.ends_with('/') {
        return true;
    }
    // Known processor markers in the slash-bearing tail.
    const MARKERS: &[&str] = &["PYMT", "PYMNT", "PAYM", "/PMT"];
    MARKERS.iter().any(|m| tok.contains(m))
}

/// Conservative canonical merchant key for grouping subscription variants.
///
/// Same merchant filed under cosmetically-different bank descriptors (branch
/// town, masked card tail, payment-gateway stub) should collapse to ONE key;
/// genuinely distinct merchants (different products, different PayPal payees)
/// must NOT. When unsure, we UNDER-merge: a missed merge is a minor nuisance,
/// a wrong merge corrupts the report.
///
/// Rules, applied in order:
/// 1. Uppercase and collapse internal whitespace to single spaces.
/// 2. Peel *trailing* noise tokens, right to left, stopping at the first token
///    that is real and never removing the final remaining token (so the core
///    name — even one containing `/`, like `APPLE.COM/BILL` — is always kept):
///    - pure mask / numeric tokens (`**********`, `***`, masked digits);
///    - payment-gateway / processor tails (`APPLE.COM/`, `ADBL.CO/PYMT`,
///      `/PYMT`);
///    - a SMALL curated set of trailing location tokens ([`TRAILING_LOCATIONS`]),
///      matched only as a whole trailing token.
/// 3. Re-join the surviving core tokens with single spaces and return.
///
/// Note: unlike [`crate::query::normalise_description`] this does NOT truncate
/// to 25 chars — truncation can fuse distinct merchants and is not wanted for a
/// grouping identity. Behaviour of `normalise_description` is unchanged.
pub fn canonical_merchant(raw: &str) -> String {
    // Rule 1: uppercase + collapse whitespace into discrete tokens.
    let mut tokens: Vec<String> = raw
        .to_uppercase()
        .split_whitespace()
        .map(String::from)
        .collect();

    // Rule 2: peel trailing noise, never below one token.
    while tokens.len() > 1 {
        let last = tokens.last().unwrap();
        let is_location = TRAILING_LOCATIONS.contains(&last.as_str());
        if is_mask_noise(last) || is_gateway_tail(last) || is_location {
            tokens.pop();
        } else {
            // First real (non-noise) trailing token: stop. We never reorder or
            // touch interior tokens, so mid-name occurrences are preserved.
            break;
        }
    }

    // Rule 3: collapse the surviving core.
    tokens.join(" ")
}

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

/// One detected subscription (one per canonical merchant).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    pub merchant: String,
    /// The (positive) *representative* per-charge amount: the most common charge
    /// in the group (ties broken by the lower amount). Used for the annualised
    /// cost and the same-amount duplicate cross-check.
    pub amount: Decimal,
    /// Lowest charge seen in the group (== `amount` when the price is fixed).
    pub amount_min: Decimal,
    /// Highest charge seen in the group (== `amount` when the price is fixed).
    pub amount_max: Decimal,
    pub cadence: Cadence,
    pub occurrences: usize,
    pub first_seen: NaiveDate,
    pub last_seen: NaiveDate,
}

impl Subscription {
    /// Annualised cost: monthly charge x12, annual charge x1. Uses the
    /// representative amount.
    pub fn annualised(&self) -> Decimal {
        self.amount * self.cadence.annual_multiplier()
    }

    /// Does the per-charge amount vary across the group (a price range)?
    pub fn amount_varies(&self) -> bool {
        self.amount_min != self.amount_max
    }
}

/// A flagged review item: a cluster worth a human glance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewFlag {
    /// Two or more *distinct* merchants billing the same *representative* amount
    /// (e.g. two separate £12.99 streams). The classic forgotten-duplicate
    /// signal. Distinct merchants are now distinct grouping keys, so two genuine
    /// £12.99 subscriptions remain two subscriptions and this cross-check still
    /// fires.
    SameAmountDifferentMerchants {
        amount: Decimal,
        merchants: Vec<String>,
    },
    /// **Informational** — a single merchant whose per-charge amount range is
    /// *wide* (a likely real price change). This is NOT a duplicate and NOT two
    /// subscriptions: it is ONE subscription spanning a price range. Surfaced as
    /// a note so the spread is visible, never double-counted.
    WidePriceRange {
        merchant: String,
        amount_min: Decimal,
        amount_max: Decimal,
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

/// Resolve the grouping key for a bank row.
///
/// A bare `PAYPAL PAYMENT` row carries no merchant. When `recoveries` is
/// supplied AND the row is a bare PayPal payment AND a recovery exists for its
/// `import_id`, the recovered (real) merchant — canonicalised — becomes the key,
/// so recovered PayPal subscriptions are visible. Otherwise the canonicalised
/// bank `description` is used (the raw key — what an unrecovered row keeps).
fn grouping_key(tx: &Transaction, recoveries: Option<&RecoveryIndex>) -> String {
    if let Some(idx) = recoveries {
        if crate::paypal::is_bare_paypal_payment(tx) {
            if let Some(m) = idx.recovered_merchant_for(&tx.import_id) {
                let canon = canonical_merchant(m);
                if !canon.is_empty() {
                    return canon;
                }
            }
        }
    }
    canonical_merchant(&tx.description)
}

/// The *representative* amount of a group: the most common charge, ties broken
/// by the LOWER amount (deterministic, and conservatively reports the cheaper of
/// two equally-common prices). `amounts` is non-empty.
fn representative_amount(amounts: &[Decimal]) -> Decimal {
    let mut counts: BTreeMap<Decimal, usize> = BTreeMap::new();
    for a in amounts {
        *counts.entry(*a).or_default() += 1;
    }
    // BTreeMap iterates ascending by amount, so the first max-count entry seen is
    // the lowest amount among the most-common — exactly the tie-break we want.
    counts
        .into_iter()
        .max_by_key(|&(amount, count)| (count, std::cmp::Reverse(amount)))
        .map(|(amount, _)| amount)
        .unwrap_or(Decimal::ZERO)
}

/// Run the full subscriptions audit over a transaction set within `filter`.
///
/// PayPal recovery is NOT applied — bare `PAYPAL PAYMENT` rows group under their
/// literal description. Use [`audit_with_recovery`] to resolve them to their
/// recovered merchant.
pub fn audit(txs: &[Transaction], filter: DateFilter, opts: DetectOptions) -> Audit {
    audit_inner(txs, filter, opts, None)
}

/// Run the audit, additionally resolving bare `PAYPAL PAYMENT` rows to their
/// recovered merchant via `recoveries` (from `paypal_matches.jsonl`).
pub fn audit_with_recovery(
    txs: &[Transaction],
    filter: DateFilter,
    opts: DetectOptions,
    recoveries: &RecoveryIndex,
) -> Audit {
    audit_inner(txs, filter, opts, Some(recoveries))
}

fn audit_inner(
    txs: &[Transaction],
    filter: DateFilter,
    opts: DetectOptions,
    recoveries: Option<&RecoveryIndex>,
) -> Audit {
    // Group debit charges by canonical MERCHANT alone (no amount in the key), so
    // a merchant whose price wobbles month-to-month stays ONE subscription. Each
    // charge keeps its amount so we can derive a representative amount + range.
    // BTreeMap keeps output deterministic without an extra sort pass on the keys.
    let mut groups: BTreeMap<String, Vec<(NaiveDate, Decimal)>> = BTreeMap::new();

    for tx in txs {
        if !filter.matches(tx.date) {
            continue;
        }
        // Outgoing money only.
        if !tx.is_debit() {
            continue;
        }
        // Grouping key: recovered PayPal merchant (when available) else the
        // conservative canonicalisation of the bank description. Cosmetic
        // variants of the SAME merchant (branch town, masked card tail, gateway
        // stub) collapse to one key without merging genuinely distinct merchants.
        let merchant = grouping_key(tx, recoveries);
        // Skip rows that normalise to nothing (defensive — shouldn't happen).
        if merchant.is_empty() {
            continue;
        }
        let amount = tx.amount.abs();
        groups.entry(merchant).or_default().push((tx.date, amount));
    }

    let mut subscriptions = Vec::new();
    for (merchant, mut charges) in groups {
        // Sort charges by date so cadence spacing is computed in order.
        charges.sort_by(|a, b| a.0.cmp(&b.0));

        // Cadence is about *spacing*: collapse same-day charges to one date (two
        // charges on one day could be a retry / split). We keep the amount of
        // the FIRST charge on a day for the representative/range stats — the
        // amount picture is over distinct billing events.
        let mut dates: Vec<NaiveDate> = Vec::with_capacity(charges.len());
        let mut day_amounts: Vec<Decimal> = Vec::with_capacity(charges.len());
        for (date, amount) in &charges {
            if dates.last() != Some(date) {
                dates.push(*date);
                day_amounts.push(*amount);
            }
        }

        if let Some(cadence) = classify(&dates, &opts) {
            let amount = representative_amount(&day_amounts);
            let amount_min = day_amounts.iter().copied().min().unwrap();
            let amount_max = day_amounts.iter().copied().max().unwrap();
            subscriptions.push(Subscription {
                merchant,
                amount,
                amount_min,
                amount_max,
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

/// Is `[min, max]` a *wide* range relative to `representative`? Used to decide
/// whether a merchant's price spread is worth an informational note (a real
/// price change) rather than mere FX wobble. A zero/negative representative
/// (defensive) is never "wide".
fn is_wide_range(min: Decimal, max: Decimal, representative: Decimal) -> bool {
    use rust_decimal::prelude::ToPrimitive;
    if representative <= Decimal::ZERO {
        return false;
    }
    let spread = (max - min).to_f64().unwrap_or(0.0);
    let base = representative.to_f64().unwrap_or(0.0);
    base > 0.0 && (spread / base) > WIDE_RANGE_FRACTION
}

/// Build the "review these" flags from the detected subscriptions.
fn build_flags(subs: &[Subscription]) -> Vec<ReviewFlag> {
    let mut flags = Vec::new();

    // (a) Same *representative* amount, distinct merchants. Distinct merchants
    // are distinct grouping keys, so two genuine £12.99 subscriptions stay two
    // subscriptions and still trip this cross-check.
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

    // (b) INFORMATIONAL: one merchant, one subscription, but a WIDE price range
    // (a likely real price change). This is no longer a duplicate — the merchant
    // is a single subscription with a price range — so we note it once, never
    // splitting it into two. Subscriptions are pre-sorted, so iterating in order
    // keeps the notes deterministic.
    for s in subs {
        if s.amount_varies() && is_wide_range(s.amount_min, s.amount_max, s.amount) {
            flags.push(ReviewFlag::WidePriceRange {
                merchant: s.merchant.clone(),
                amount_min: s.amount_min,
                amount_max: s.amount_max,
            });
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

/// The Amount cell for a subscription: the representative amount, plus a compact
/// "(£min-£max)" suffix when the per-charge amount varies (an FX-priced or
/// price-changed merchant). A fixed-price subscription shows just the amount.
fn fmt_amount_cell(s: &Subscription) -> String {
    if s.amount_varies() {
        format!(
            "{} ({}-{})",
            fmt_gbp(s.amount),
            fmt_gbp(s.amount_min),
            fmt_gbp(s.amount_max)
        )
    } else {
        fmt_gbp(s.amount)
    }
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
            "{:<32} {:>17}  {:<8} {:>4}  {:<11} {:<11} {:>11}\n",
            "Merchant", "Amount", "Cadence", "Occ", "First seen", "Last seen", "Annualised"
        ));
        out.push_str(&format!("{}\n", "-".repeat(100)));
        for s in &audit.subscriptions {
            out.push_str(&format!(
                "{:<32} {:>17}  {:<8} {:>4}  {:<11} {:<11} {:>11}\n",
                truncate(&s.merchant, 32),
                fmt_amount_cell(s),
                s.cadence.as_str(),
                s.occurrences,
                s.first_seen,
                s.last_seen,
                fmt_gbp(s.annualised()),
            ));
        }
        out.push_str(&format!("{}\n", "-".repeat(100)));
        out.push_str(&format!(
            "{:<32} {:>17}  {:<8} {:>4}  {:<11} {:<11} {:>11}\n",
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
                ReviewFlag::WidePriceRange {
                    merchant,
                    amount_min,
                    amount_max,
                } => {
                    // Informational: ONE subscription, wide price range (likely a
                    // real price change). Not a duplicate, not double-counted.
                    out.push_str(&format!(
                        "  i {} price ranges {}-{} (one subscription, likely a price change)\n",
                        merchant,
                        fmt_gbp(*amount_min),
                        fmt_gbp(*amount_max),
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
        // Fixed price -> min == max == representative, and the range is flat.
        assert_eq!(s.amount_min, dec("9.99"));
        assert_eq!(s.amount_max, dec("9.99"));
        assert!(!s.amount_varies());
        assert_eq!(s.cadence, Cadence::Monthly);
        assert_eq!(s.occurrences, 5);
        assert_eq!(s.first_seen, NaiveDate::from_ymd_opt(2025, 1, 15).unwrap());
        assert_eq!(s.last_seen, NaiveDate::from_ymd_opt(2025, 5, 15).unwrap());
        // Annualised = 9.99 * 12.
        assert_eq!(s.annualised(), dec("119.88"));
        assert_eq!(audit.total_annualised(), dec("119.88"));
        // A fixed-price subscription produces no flags.
        assert!(audit.flags.is_empty());
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
        // genuinely ERRATIC spacing -> NOT a subscription, and no flags. (Under
        // merchant-only grouping all five rows form one group, so the spacing
        // must be irregular enough that the median day-gap falls outside both
        // the monthly and annual bands: clustered early visits then a long gap
        // gives a median of ~3 days, far below the 25-day monthly floor.)
        let txs = vec![
            debit("2025-01-03", "23.40", "CORNER SHOP"),
            debit("2025-01-05", "8.10", "CORNER SHOP"),
            debit("2025-01-08", "55.00", "CORNER SHOP"),
            debit("2025-01-12", "12.00", "CORNER SHOP"),
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
    fn same_merchant_price_rise_is_one_subscription_with_range() {
        // A price rise mid-stream: same merchant, two distinct prices. Under the
        // new merchant-only grouping this is ONE subscription spanning a price
        // range, NOT two — and the wide spread (10->12 = 20% > 10%) is surfaced
        // as an INFORMATIONAL price-change note, never double-counted.
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
        // ONE subscription for the merchant, spanning a price range.
        assert_eq!(audit.subscriptions.len(), 1);
        let s = &audit.subscriptions[0];
        assert_eq!(s.merchant, "NEWSDAILY");
        assert_eq!(s.amount_min, dec("10.00"));
        assert_eq!(s.amount_max, dec("12.00"));
        assert!(s.amount_varies());
        assert_eq!(s.occurrences, 6);
        // Three charges at each price; tie on count -> representative is the
        // LOWER amount (£10.00), and annualised uses it.
        assert_eq!(s.amount, dec("10.00"));
        assert_eq!(s.annualised(), dec("120.00"));

        // The wide spread is flagged as an informational price-change note.
        let flagged: Vec<&ReviewFlag> = audit
            .flags
            .iter()
            .filter(|f| matches!(f, ReviewFlag::WidePriceRange { .. }))
            .collect();
        assert_eq!(flagged.len(), 1);
        if let ReviewFlag::WidePriceRange {
            merchant,
            amount_min,
            amount_max,
        } = flagged[0]
        {
            assert_eq!(merchant, "NEWSDAILY");
            assert_eq!(*amount_min, dec("10.00"));
            assert_eq!(*amount_max, dec("12.00"));
        } else {
            unreachable!();
        }
        // And NO same-amount-distinct-merchants flag (only one merchant).
        assert!(!audit
            .flags
            .iter()
            .any(|f| matches!(f, ReviewFlag::SameAmountDifferentMerchants { .. })));
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

    // -----------------------------------------------------------------------
    // canonical_merchant — MERGE direction (cosmetic variants of ONE merchant)
    // -----------------------------------------------------------------------

    #[test]
    fn canonical_merges_apple_bill_variants() {
        // The real Apple case: masked tail, branch town, and gateway stub are
        // all the SAME merchant and must collapse to one key.
        let a = canonical_merchant("APPLE.COM/BILL **********");
        let b = canonical_merchant("APPLE.COM/BILL CORK");
        let c = canonical_merchant("APPLE.COM/BILL APPLE.COM/");
        assert_eq!(a, "APPLE.COM/BILL");
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    #[test]
    fn canonical_merges_audible_variants() {
        // Audible: a processor tail (ADBL.CO/PYMT) and a branch town (LONDON)
        // are the same merchant.
        let a = canonical_merchant("AUDIBLE UK ADBL.CO/PYMT");
        let b = canonical_merchant("AUDIBLE UK LONDON");
        assert_eq!(a, "AUDIBLE UK");
        assert_eq!(a, b);
    }

    #[test]
    fn canonical_drops_only_trailing_noise_not_core_slash() {
        // The core token itself contains a slash; it must survive (never peel
        // the last remaining token).
        assert_eq!(canonical_merchant("APPLE.COM/BILL"), "APPLE.COM/BILL");
        // Mask-only or gateway-only inputs collapse to the surviving core.
        assert_eq!(canonical_merchant("AUDIBLE UK *** 1234"), "AUDIBLE UK");
    }

    // -----------------------------------------------------------------------
    // canonical_merchant — DO-NOT-MERGE direction (distinct merchants)
    // -----------------------------------------------------------------------

    #[test]
    fn canonical_keeps_google_products_distinct() {
        // Three different Google products / payment routes must stay THREE keys.
        // "MEMLONDON"/"GSUITE_WILLDUBLIN" embed a town as a SUBSTRING, not a bare
        // trailing token, so they are preserved; "(VIA PAYPAL)" is its own route.
        let yt = canonical_merchant("GOOGLE YOUTUBE MEMLONDON");
        let gs = canonical_merchant("GOOGLE GSUITE_WILLDUBLIN");
        let pp = canonical_merchant("GOOGLE (VIA PAYPAL)");
        assert_ne!(yt, gs);
        assert_ne!(yt, pp);
        assert_ne!(gs, pp);
        // And none of them collapsed to a bare "GOOGLE".
        assert_ne!(yt, "GOOGLE");
        assert_ne!(gs, "GOOGLE");
        assert_ne!(pp, "GOOGLE");
    }

    #[test]
    fn canonical_keeps_netflix_distinct() {
        // A long descriptive name must not collapse into or merge with anything.
        let n = canonical_merchant("NETFLIX SERVICES UK LIMIT");
        assert_eq!(n, "NETFLIX SERVICES UK LIMIT");
        assert_ne!(n, canonical_merchant("NETFLIX"));
    }

    #[test]
    fn canonical_does_not_merge_paypal_payee_with_bare_paypal() {
        // A recovered PayPal payee must stay distinct from an unrecovered bare
        // PAYPAL PAYMENT row.
        let payee = canonical_merchant("DHARMACHAKRA (VIA PAYPAL)");
        let bare = canonical_merchant("PAYPAL PAYMENT");
        assert_ne!(payee, bare);
        assert_eq!(bare, "PAYPAL PAYMENT");
        // Bare PayPal stays its own thing — no noise rule touches it.
        assert_eq!(payee, "DHARMACHAKRA (VIA PAYPAL)");
    }

    #[test]
    fn canonical_does_not_drop_midname_location() {
        // A location token in the MIDDLE of a name must never be stripped.
        assert_eq!(
            canonical_merchant("LONDON TRANSPORT TFL"),
            "LONDON TRANSPORT TFL"
        );
    }

    // -----------------------------------------------------------------------
    // End-to-end: APPLE.COM/BILL variants collapse in the audit, while two
    // genuinely distinct same-amount merchants are still flagged.
    // -----------------------------------------------------------------------

    #[test]
    fn audit_collapses_apple_variants_but_still_flags_distinct_pair() {
        let mut txs = Vec::new();
        // ONE Apple subscription whose monthly charges arrive under three
        // cosmetically-different descriptors — must become a SINGLE merchant.
        let apple_variants = [
            "APPLE.COM/BILL **********",
            "APPLE.COM/BILL CORK",
            "APPLE.COM/BILL APPLE.COM/",
            "APPLE.COM/BILL **********",
            "APPLE.COM/BILL CORK",
        ];
        for (i, desc) in apple_variants.iter().enumerate() {
            let m = (i + 1) as u32;
            txs.push(debit(&format!("2025-{:02}-08", m), "0.99", desc));
        }
        // Two GENUINELY distinct merchants at the same amount (£0.99) — these
        // must remain two merchants and still trip the duplicate flag.
        for m in 1u32..=4 {
            txs.push(debit(&format!("2025-{:02}-12", m), "0.99", "CLOUDPHOTOS"));
            txs.push(debit(&format!("2025-{:02}-22", m), "0.99", "PODCASTPLUS"));
        }

        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());

        // Exactly THREE detected merchants: one canonical Apple, plus the two
        // distinct £0.99 streams. (Without canonicalisation Apple would be 3.)
        assert_eq!(audit.subscriptions.len(), 3);
        let merchants: Vec<&str> = audit
            .subscriptions
            .iter()
            .map(|s| s.merchant.as_str())
            .collect();
        assert!(merchants.contains(&"APPLE.COM/BILL"));
        assert!(merchants.contains(&"CLOUDPHOTOS"));
        assert!(merchants.contains(&"PODCASTPLUS"));

        // The Apple subscription has all five occurrences merged into one group.
        let apple = audit
            .subscriptions
            .iter()
            .find(|s| s.merchant == "APPLE.COM/BILL")
            .expect("apple subscription present");
        assert_eq!(apple.occurrences, 5);

        // The same-amount flag lists exactly the THREE distinct merchants at
        // £0.99 (Apple counted once, not three times).
        let flagged: Vec<&ReviewFlag> = audit
            .flags
            .iter()
            .filter(|f| matches!(f, ReviewFlag::SameAmountDifferentMerchants { .. }))
            .collect();
        assert_eq!(flagged.len(), 1);
        if let ReviewFlag::SameAmountDifferentMerchants { amount, merchants } = flagged[0] {
            assert_eq!(*amount, dec("0.99"));
            assert_eq!(
                merchants,
                &vec![
                    "APPLE.COM/BILL".to_string(),
                    "CLOUDPHOTOS".to_string(),
                    "PODCASTPLUS".to_string(),
                ]
            );
        } else {
            unreachable!();
        }
    }

    // -----------------------------------------------------------------------
    // Change 1: merchant-only grouping — an FX-varying merchant collapses to ONE
    // subscription with a price range.
    // -----------------------------------------------------------------------

    #[test]
    fn fx_varying_merchant_collapses_to_one_subscription_with_range() {
        // A foreign-priced subscription whose GBP charge wobbles every month
        // (FX drift). Under (merchant, exact-amount) grouping this would split
        // into several phantom subscriptions; under merchant-only grouping it is
        // ONE subscription with a representative amount and a min-max range.
        let txs = vec![
            debit("2025-01-10", "9.41", "FOREIGNSAAS"),
            debit("2025-02-10", "9.62", "FOREIGNSAAS"),
            debit("2025-03-10", "9.20", "FOREIGNSAAS"),
            debit("2025-04-10", "9.41", "FOREIGNSAAS"),
            debit("2025-05-10", "9.55", "FOREIGNSAAS"),
        ];
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        // Exactly ONE subscription despite five different prices.
        assert_eq!(audit.subscriptions.len(), 1);
        let s = &audit.subscriptions[0];
        assert_eq!(s.merchant, "FOREIGNSAAS");
        assert_eq!(s.cadence, Cadence::Monthly);
        assert_eq!(s.occurrences, 5);
        // Representative = most common (9.41 appears twice); range spans min-max.
        assert_eq!(s.amount, dec("9.41"));
        assert_eq!(s.amount_min, dec("9.20"));
        assert_eq!(s.amount_max, dec("9.62"));
        assert!(s.amount_varies());
        // Annualised uses the representative amount.
        assert_eq!(s.annualised(), dec("112.92"));
        // Range here is narrow (9.20..9.62 = 0.42, < 10% of 9.41), so it does
        // NOT trip the informational wide-range note — small FX wobble is quiet.
        assert!(audit.flags.is_empty());
        // The rendered Amount cell shows the representative plus the range.
        let text = render(&audit);
        assert!(text.contains("£9.41 (£9.20-£9.62)"), "rendered: {text}");
    }

    // -----------------------------------------------------------------------
    // Change 1: two DISTINCT merchants at the same amount stay separate and are
    // still flagged (the duplicate cross-check survives merchant-only grouping).
    // -----------------------------------------------------------------------

    #[test]
    fn distinct_merchants_same_amount_stay_separate_and_flagged() {
        // Two genuinely distinct £12.99 monthly streams. Distinct merchants are
        // distinct grouping keys, so they remain TWO subscriptions, and the
        // same-amount cross-check still fires.
        let mut txs = Vec::new();
        for m in 1u32..=5 {
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

    // -----------------------------------------------------------------------
    // Change 2: bare PAYPAL PAYMENT rows resolve to their recovered merchant.
    // -----------------------------------------------------------------------

    /// A bare `PAYPAL PAYMENT` debit with a given `import_id`, so a recovery
    /// index can key off it.
    fn paypal_debit(date: &str, magnitude: &str, import_id: &str) -> Transaction {
        Transaction {
            date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            account: Account::Current,
            tx_type: TxType::DirectDebit,
            amount: -Decimal::from_str(magnitude).unwrap(),
            description: "PAYPAL PAYMENT".to_string(),
            raw_description: "PAYPAL PAYMENT".to_string(),
            balance: None,
            tags: Vec::new(),
            import_id: import_id.to_string(),
        }
    }

    fn recovery_index(pairs: &[(&str, &str)]) -> RecoveryIndex {
        let rows = pairs
            .iter()
            .map(|(id, merchant)| crate::paypal::RecoveryRow {
                bank_import_id: id.to_string(),
                recovered_merchant: merchant.to_string(),
                currency: "GBP".to_string(),
                leg: "direct-gbp".to_string(),
            })
            .collect();
        RecoveryIndex::from_rows(rows)
    }

    #[test]
    fn bare_paypal_row_with_recovery_groups_under_recovered_merchant() {
        // Five monthly bare PAYPAL PAYMENT rows, each recovered to the same real
        // merchant. They must group under the RECOVERED merchant, not the literal
        // "PAYPAL PAYMENT" key — so the subscription becomes visible.
        let txs = vec![
            paypal_debit("2025-01-15", "7.99", "pp-1"),
            paypal_debit("2025-02-15", "7.99", "pp-2"),
            paypal_debit("2025-03-15", "7.99", "pp-3"),
            paypal_debit("2025-04-15", "7.99", "pp-4"),
            paypal_debit("2025-05-15", "7.99", "pp-5"),
        ];
        let idx = recovery_index(&[
            ("pp-1", "Dharmachakra"),
            ("pp-2", "Dharmachakra"),
            ("pp-3", "Dharmachakra"),
            ("pp-4", "Dharmachakra"),
            ("pp-5", "Dharmachakra"),
        ]);
        let audit =
            audit_with_recovery(&txs, DateFilter::default(), DetectOptions::default(), &idx);
        assert_eq!(audit.subscriptions.len(), 1);
        let s = &audit.subscriptions[0];
        // Recovered merchant is canonicalised ("Dharmachakra" -> uppercase).
        assert_eq!(s.merchant, "DHARMACHAKRA");
        assert_eq!(s.cadence, Cadence::Monthly);
        assert_eq!(s.occurrences, 5);
        assert_eq!(s.amount, dec("7.99"));
        // It did NOT group under the literal PayPal key.
        assert!(audit
            .subscriptions
            .iter()
            .all(|s| s.merchant != "PAYPAL PAYMENT"));
    }

    #[test]
    fn bare_paypal_row_without_recovery_falls_back_to_raw_key() {
        // Bare PAYPAL PAYMENT rows with NO recovery for their ids keep the raw
        // "PAYPAL PAYMENT" key (the index is empty / lacks these ids).
        let txs = vec![
            paypal_debit("2025-01-15", "5.00", "pp-x1"),
            paypal_debit("2025-02-15", "5.00", "pp-x2"),
            paypal_debit("2025-03-15", "5.00", "pp-x3"),
        ];
        // Recovery index knows about an UNRELATED id only.
        let idx = recovery_index(&[("some-other-id", "Streamflix")]);
        let with_idx =
            audit_with_recovery(&txs, DateFilter::default(), DetectOptions::default(), &idx);
        assert_eq!(with_idx.subscriptions.len(), 1);
        assert_eq!(with_idx.subscriptions[0].merchant, "PAYPAL PAYMENT");

        // And the plain `audit` (no index at all) behaves identically.
        let no_idx = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert_eq!(no_idx.subscriptions.len(), 1);
        assert_eq!(no_idx.subscriptions[0].merchant, "PAYPAL PAYMENT");
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
