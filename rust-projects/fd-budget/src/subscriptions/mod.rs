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
//!   into ONE, with a *representative* amount (the median charge) and an
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

/// A merchant's price has undergone a sustained LEVEL SHIFT — and therefore
/// warrants an informational price-change note — when the *directional* move
/// from the earliest to the latest charge, `|last - first|`, exceeds this
/// fraction of the representative amount. 0.10 = a >10% step. Deliberately a
/// *relative* threshold so a £2 service and a £200 service are judged on the
/// same footing. Being directional (first→last), not amplitude-only (max-min),
/// it distinguishes a genuine sustained rise/drop from symmetric FX wobble or
/// scatter that returns to its starting level — the latter no longer trips it.
const WIDE_RANGE_FRACTION: f64 = 0.10;

/// Within one merchant, charges whose amounts differ by MORE than this fraction
/// (relative to the smaller) start a new amount-cluster — i.e. a separate
/// candidate subscription. Chosen so two genuinely distinct streams from one
/// biller (e.g. `APPLE.COM/BILL` £2.99 and £9.99, >200% apart) are detected
/// separately, while FX-wobble and modest price drift of ONE subscription
/// (a few % on a £10 charge) stay in the same cluster. Grouping by merchant
/// alone used to interleave the two streams' dates, wrecking the cadence so
/// BOTH vanished; and it let two dissimilar one-offs a year apart masquerade as
/// an annual subscription.
const AMOUNT_CLUSTER_GAP: f64 = 0.25;

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
/// `***`) and mask-shaped tokens made up entirely of `*`, digits, and ASCII
/// punctuation (no letters), e.g. masked card tails. Such a token carries no
/// merchant identity, so when TRAILING it is dropped.
///
/// EXCEPTION: a token that is ENTIRELY digits is only noise when it is LONG
/// (>= 3 digits) — the masked card-tail shape. A short 1-2 digit trailing token
/// is a *distinguishing* part of the merchant name (`CHANNEL 4`, `CHANNEL 5`,
/// `RADIO 1`, `STUDIO 54`) and must be kept, or those merchants would collapse
/// together. Long digit runs and any `*`/punctuation-bearing masked tail still
/// strip as before.
fn is_mask_noise(tok: &str) -> bool {
    if tok.is_empty() {
        return false;
    }
    // Purely numeric: strip only the long (>= 3 digit) masked-tail shape; keep
    // short 1-2 digit tokens that distinguish a merchant name.
    if tok.chars().all(|c| c.is_ascii_digit()) {
        return tok.len() >= 3;
    }
    // Otherwise: mask / punctuation noise (runs of `*`, masked digits carrying
    // `*` or punctuation, e.g. `**********`, `***`, `****1234`).
    tok.chars()
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
    /// The (positive) *representative* per-charge amount: the MEDIAN charge in
    /// the group. Used for the annualised cost and the same-amount duplicate
    /// cross-check.
    pub amount: Decimal,
    /// Lowest charge seen in the group (== `amount` when the price is fixed).
    pub amount_min: Decimal,
    /// Highest charge seen in the group (== `amount` when the price is fixed).
    pub amount_max: Decimal,
    /// The earliest (date-ordered) charge's amount. With [`Self::last_amount`],
    /// lets the price-change note detect a *directional* level shift rather than
    /// mere amplitude — see [`WIDE_RANGE_FRACTION`].
    pub first_amount: Decimal,
    /// The latest (date-ordered) charge's amount.
    pub last_amount: Decimal,
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
    /// **Informational** — a single merchant whose price has undergone a
    /// sustained directional LEVEL SHIFT from its earliest to its latest charge
    /// (a likely real price change), not mere FX wobble/scatter. This is NOT a
    /// duplicate and NOT two subscriptions: it is ONE subscription spanning a
    /// price range. Surfaced as a note (with the min/max seen) so the change is
    /// visible, never double-counted.
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

/// LOWER median of a *non-empty* sorted slice of day-gaps.
///
/// Always returns a REAL observed gap (`sorted[(n-1)/2]`) — never an average of
/// two central gaps. Averaging (the old even-count behaviour) could fuse two
/// out-of-band gaps INTO the band (e.g. gaps [20, 40] average to a false
/// "monthly" 30) or pull a genuine cadence out of it. The lower median rejects
/// [20, 40] (→ 20, correctly not monthly) and keeps [31, 59] (→ 31, a real
/// monthly with one skipped month). For odd `n` this equals the ordinary median.
fn median_gap(sorted_gaps: &[i64]) -> i64 {
    sorted_gaps[(sorted_gaps.len() - 1) / 2]
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
    // Largest observed gap (gaps is sorted ascending). A single very large gap
    // must not be smuggled past the median: it means the cadence lapsed, so the
    // spacing is not really regular. Reject when the max gap exceeds 3x the
    // band's upper bound (monthly 3x35 = 105 days; annual 3x380 = 1140 days).
    // This rejects e.g. [30, 200] (200 > 105) while keeping a doubled/skipped
    // gap like [31, 59] (59 <= 105).
    let max_gap = *gaps.last().unwrap();

    // Monthly: ~28-31 day cadence. Allow 25..=35 to tolerate billing-day drift
    // and short/long months. Median absorbs one doubled gap (a missed month).
    if (25..=35).contains(&median) && max_gap <= 105 && dates.len() >= opts.min_monthly {
        return Some(Cadence::Monthly);
    }
    // Annual: ~365 day cadence. Allow 350..=380 for leap years / billing drift.
    if (350..=380).contains(&median) && max_gap <= 1140 && dates.len() >= opts.min_annual {
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

/// The *representative* amount of a group: the MEDIAN charge. `amounts` is
/// non-empty.
///
/// A mode (most-common charge) degenerates to the MINIMUM for FX-priced subs
/// where every charge is distinct — the old count-then-lowest tie-break returned
/// the cheapest of the (all equally-common, count 1) amounts, systematically
/// understating the annualised cost. The median is a real charge from the group,
/// resists both an outlier FX spike and a one-off partial charge, and is
/// deterministic. For an even count we take the UPPER median (`sorted[len/2]`),
/// so a genuine old→new price step reports at the newer (current) level.
fn representative_amount(amounts: &[Decimal]) -> Decimal {
    let mut sorted: Vec<Decimal> = amounts.to_vec();
    sorted.sort_unstable();
    sorted[sorted.len() / 2]
}

/// Split one merchant's charges into amount-clusters so two DISTINCT
/// subscriptions from the same biller are detected separately, while FX-wobble
/// / modest price drift of a single subscription stays together. Charges are
/// sorted by amount and split wherever consecutive amounts differ by more than
/// [`AMOUNT_CLUSTER_GAP`] relative to the smaller. Amounts are the pre-abs'd
/// positive magnitudes stored in the group.
fn cluster_by_amount(mut charges: Vec<(NaiveDate, Decimal)>) -> Vec<Vec<(NaiveDate, Decimal)>> {
    use rust_decimal::prelude::ToPrimitive;
    charges.sort_by(|a, b| a.1.cmp(&b.1));
    let mut clusters: Vec<Vec<(NaiveDate, Decimal)>> = Vec::new();
    for charge in charges {
        match clusters.last_mut() {
            Some(cluster) => {
                let prev = cluster.last().unwrap().1.to_f64().unwrap_or(0.0).abs();
                let cur = charge.1.to_f64().unwrap_or(0.0).abs();
                let base = prev.min(cur).max(f64::MIN_POSITIVE);
                if (cur - prev).abs() / base > AMOUNT_CLUSTER_GAP {
                    clusters.push(vec![charge]);
                } else {
                    cluster.push(charge);
                }
            }
            None => clusters.push(vec![charge]),
        }
    }
    clusters
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
    for (merchant, charges) in groups {
        // Split a merchant's charges into amount-clusters first, so two distinct
        // subscriptions from one biller are detected separately (and two
        // dissimilar one-offs a year apart don't fabricate an annual sub), while
        // FX-wobble of one subscription stays as a single cluster.
        for mut cluster in cluster_by_amount(charges) {
            // Sort by date so cadence spacing is computed in order.
            cluster.sort_by(|a, b| a.0.cmp(&b.0));

            // Cadence is about *spacing*: collapse same-day charges to one date
            // for the gap analysis (two charges on one day could be a retry /
            // split). But the amount STATS must keep every charge — a second
            // same-day charge of a DIFFERENT amount (within the cluster) is a
            // real charge and must feed the min/max/representative, not be
            // dropped. So `amounts` sees all charges; `dates` is deduped by day.
            let mut dates: Vec<NaiveDate> = Vec::with_capacity(cluster.len());
            let mut amounts: Vec<Decimal> = Vec::with_capacity(cluster.len());
            for (date, amount) in &cluster {
                amounts.push(*amount);
                if dates.last() != Some(date) {
                    dates.push(*date);
                }
            }

            if let Some(cadence) = classify(&dates, &opts) {
                let amount = representative_amount(&amounts);
                let amount_min = amounts.iter().copied().min().unwrap();
                let amount_max = amounts.iter().copied().max().unwrap();
                // `cluster` is date-sorted, so its first/last charge give the
                // earliest and latest amount for the directional price-change
                // check. (Same-day ordering is the stable input order.)
                let first_amount = cluster.first().unwrap().1;
                let last_amount = cluster.last().unwrap().1;
                subscriptions.push(Subscription {
                    merchant: merchant.clone(),
                    amount,
                    amount_min,
                    amount_max,
                    first_amount,
                    last_amount,
                    cadence,
                    occurrences: dates.len(),
                    first_seen: *dates.first().unwrap(),
                    last_seen: *dates.last().unwrap(),
                });
            }
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

/// Has the price undergone a sustained directional LEVEL SHIFT? True when the
/// move from the earliest charge (`first`) to the latest (`last`) exceeds
/// [`WIDE_RANGE_FRACTION`] of `representative`. Unlike an amplitude (max-min)
/// test this fires on a genuine step (e.g. £10.99 → £11.99) but stays quiet for
/// FX wobble/scatter that returns near its starting level. A zero/negative
/// representative (defensive) never counts as a shift.
fn is_level_shift(first: Decimal, last: Decimal, representative: Decimal) -> bool {
    use rust_decimal::prelude::ToPrimitive;
    if representative <= Decimal::ZERO {
        return false;
    }
    let step = (last - first).abs().to_f64().unwrap_or(0.0);
    let base = representative.to_f64().unwrap_or(0.0);
    base > 0.0 && (step / base) > WIDE_RANGE_FRACTION
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

    // (b) INFORMATIONAL: one merchant, one subscription, whose price stepped to a
    // new sustained LEVEL between its earliest and latest charge (a likely real
    // price change) — not mere FX wobble/scatter. This is no longer a duplicate —
    // the merchant is a single subscription with a price range — so we note it
    // once, never splitting it into two. Subscriptions are pre-sorted, so
    // iterating in order keeps the notes deterministic.
    for s in subs {
        if s.amount_varies() && is_level_shift(s.first_amount, s.last_amount, s.amount) {
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
    fn two_distinct_amount_subs_from_one_merchant_both_detected() {
        // Two streams under ONE descriptor (Apple £2.99 on the 5th, £9.99 on the
        // 19th), interleaved. Grouping by merchant alone wrecked the cadence so
        // BOTH vanished; amount-clustering now detects each.
        let txs = vec![
            debit("2025-01-05", "2.99", "APPLE.COM/BILL"),
            debit("2025-01-19", "9.99", "APPLE.COM/BILL"),
            debit("2025-02-05", "2.99", "APPLE.COM/BILL"),
            debit("2025-02-19", "9.99", "APPLE.COM/BILL"),
            debit("2025-03-05", "2.99", "APPLE.COM/BILL"),
            debit("2025-03-19", "9.99", "APPLE.COM/BILL"),
        ];
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert_eq!(audit.subscriptions.len(), 2, "both Apple streams should be detected");
        let mut amts: Vec<Decimal> = audit.subscriptions.iter().map(|s| s.amount).collect();
        amts.sort();
        assert_eq!(amts, vec![dec("2.99"), dec("9.99")]);
    }

    #[test]
    fn fx_wobble_stays_one_subscription() {
        // One FX-priced sub whose GBP wobbles a few % must NOT split into several.
        let txs = vec![
            debit("2025-01-10", "9.20", "FOREIGN SAAS"),
            debit("2025-02-10", "9.71", "FOREIGN SAAS"),
            debit("2025-03-10", "9.48", "FOREIGN SAAS"),
            debit("2025-04-10", "9.55", "FOREIGN SAAS"),
        ];
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert_eq!(audit.subscriptions.len(), 1, "FX wobble must not split");
        assert_eq!(audit.subscriptions[0].occurrences, 4);
    }

    #[test]
    fn two_dissimilar_oneoffs_a_year_apart_are_not_an_annual_sub() {
        // Seasonal shopping: £85 last December, £12.50 this December. Different
        // amounts -> different single-charge clusters -> no phantom annual sub.
        let txs = vec![
            debit("2024-12-20", "85.00", "GARDEN CENTRE"),
            debit("2025-12-18", "12.50", "GARDEN CENTRE"),
        ];
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert_eq!(audit.subscriptions.len(), 0, "seasonal one-offs must not be an annual sub");
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
        // A price rise mid-stream: same merchant, two distinct prices, billed
        // continuously each month. Under the new merchant-only grouping this is
        // ONE subscription spanning a price range, NOT two — and the sustained
        // directional shift (10->12 = 20% > 10%) is surfaced as an INFORMATIONAL
        // price-change note, never double-counted. (Billing is continuous: the
        // M6 max-gap guard now rejects a multi-month lapse, so the old->new
        // change happens between consecutive months.)
        let txs = vec![
            // Old price.
            debit("2024-10-12", "10.00", "NEWSDAILY"),
            debit("2024-11-12", "10.00", "NEWSDAILY"),
            debit("2024-12-12", "10.00", "NEWSDAILY"),
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
        // Representative is the MEDIAN charge. Six charges [10,10,10,12,12,12];
        // the upper median (sorted[6/2]) is £12.00 — the newer, current level —
        // and annualised uses it.
        assert_eq!(s.amount, dec("12.00"));
        assert_eq!(s.annualised(), dec("144.00"));

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

    #[test]
    fn canonical_keeps_short_trailing_digit_tokens_but_strips_masked_tail() {
        // L7: a short (1-2 digit) trailing number DISTINGUISHES the merchant and
        // must be kept — `CHANNEL 4` and `CHANNEL 5` are different channels and
        // must NOT collapse to a bare `CHANNEL`.
        let c4 = canonical_merchant("CHANNEL 4");
        let c5 = canonical_merchant("CHANNEL 5");
        assert_eq!(c4, "CHANNEL 4");
        assert_eq!(c5, "CHANNEL 5");
        assert_ne!(c4, c5);
        // Two-digit trailing numbers survive too (STUDIO 54, RADIO on 1).
        assert_eq!(canonical_merchant("STUDIO 54"), "STUDIO 54");
        assert_eq!(canonical_merchant("RADIO 1"), "RADIO 1");
        // But a long (>= 3 digit) masked card tail is STILL stripped as noise.
        assert_eq!(canonical_merchant("MERCHANT 1234"), "MERCHANT");
        assert_eq!(canonical_merchant("MERCHANT 999"), "MERCHANT");
        // And a `*` mask tail still strips regardless of length.
        assert_eq!(canonical_merchant("MERCHANT **"), "MERCHANT");
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

    // -----------------------------------------------------------------------
    // M4: representative amount is the MEDIAN, not the minimum. For an FX-priced
    // sub where every charge is distinct, the old mode+lowest tie-break returned
    // the MINIMUM, understating the annualised cost.
    // -----------------------------------------------------------------------

    #[test]
    fn representative_is_median_not_min_for_fx_sub() {
        // Five all-distinct monthly charges (FX wobble). Median = £9.50; the old
        // behaviour would have picked the minimum £8.50 and understated the year.
        let txs = vec![
            debit("2025-01-10", "8.50", "FXSTREAM"),
            debit("2025-02-10", "9.00", "FXSTREAM"),
            debit("2025-03-10", "9.50", "FXSTREAM"),
            debit("2025-04-10", "10.00", "FXSTREAM"),
            debit("2025-05-10", "10.50", "FXSTREAM"),
        ];
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert_eq!(audit.subscriptions.len(), 1);
        let s = &audit.subscriptions[0];
        assert_eq!(s.occurrences, 5);
        // Median of [8.50, 9.00, 9.50, 10.00, 10.50] is 9.50 — NOT the min 8.50.
        assert_eq!(s.amount, dec("9.50"));
        assert_ne!(s.amount, s.amount_min);
        assert_eq!(s.annualised(), dec("114.00"));
    }

    // -----------------------------------------------------------------------
    // M6: lower-median gap + max-gap guard. [20,40] must NOT average into a
    // false monthly; [31,59] (a skipped month) must stay monthly; [30,200]
    // (a lapsed cadence) must be rejected by the max-gap guard.
    // -----------------------------------------------------------------------

    #[test]
    fn even_gap_count_uses_lower_median_not_average() {
        // Gaps [20, 40]: the old even-count median averaged to 30 (a false
        // monthly). The lower median is 20 -> outside the 25..=35 band -> rejected.
        let txs = vec![
            debit("2025-01-01", "5.00", "GAP2040"),
            debit("2025-01-21", "5.00", "GAP2040"), // +20
            debit("2025-03-02", "5.00", "GAP2040"), // +40
        ];
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert!(
            audit.subscriptions.is_empty(),
            "gaps [20,40] must not average into a false monthly"
        );
    }

    #[test]
    fn skipped_month_gaps_31_59_still_monthly() {
        // Gaps [31, 59] (one skipped month): lower median 31 is in-band and the
        // max gap 59 <= 105, so this stays a real monthly.
        let txs = vec![
            debit("2025-01-01", "5.00", "SKIP3159"),
            debit("2025-02-01", "5.00", "SKIP3159"), // +31
            debit("2025-04-01", "5.00", "SKIP3159"), // +59
        ];
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert_eq!(audit.subscriptions.len(), 1);
        assert_eq!(audit.subscriptions[0].cadence, Cadence::Monthly);
    }

    #[test]
    fn large_irregular_gap_30_200_rejected_by_max_gap_guard() {
        // Gaps [30, 200]: lower median 30 is in-band, but the 200-day gap means
        // the cadence lapsed. The max-gap guard (>105) rejects it.
        let txs = vec![
            debit("2025-01-01", "5.00", "LAPSE30200"),
            debit("2025-01-31", "5.00", "LAPSE30200"), // +30
            debit("2025-08-19", "5.00", "LAPSE30200"), // +200
        ];
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert!(
            audit.subscriptions.is_empty(),
            "a 200-day gap must be rejected by the max-gap guard"
        );
    }

    // -----------------------------------------------------------------------
    // M7: price-change flag is DIRECTIONAL. A sustained rise is flagged; FX
    // scatter that returns near its start is NOT (even though its amplitude
    // max-min exceeds 10%, which the old amplitude-only flag would have tripped).
    // -----------------------------------------------------------------------

    #[test]
    fn sustained_price_rise_is_flagged() {
        // A sustained rise £9.99 -> £11.99 (a real step > 10%).
        let txs = vec![
            debit("2024-10-12", "9.99", "STREAMRISE"),
            debit("2024-11-12", "9.99", "STREAMRISE"),
            debit("2024-12-12", "9.99", "STREAMRISE"),
            debit("2025-01-12", "11.99", "STREAMRISE"),
            debit("2025-02-12", "11.99", "STREAMRISE"),
            debit("2025-03-12", "11.99", "STREAMRISE"),
        ];
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert_eq!(audit.subscriptions.len(), 1);
        let s = &audit.subscriptions[0];
        assert_eq!(s.first_amount, dec("9.99"));
        assert_eq!(s.last_amount, dec("11.99"));
        let flagged = audit
            .flags
            .iter()
            .filter(|f| matches!(f, ReviewFlag::WidePriceRange { .. }))
            .count();
        assert_eq!(flagged, 1, "a sustained level shift should be flagged");
    }

    #[test]
    fn fx_scatter_returning_to_start_is_not_flagged() {
        // Amounts scatter (max-min = 11.50-9.20 = 2.30, a 23% amplitude that the
        // OLD amplitude-only flag would trip) but the earliest (10.00) and latest
        // (10.10) charges are essentially level -> no directional shift -> quiet.
        let txs = vec![
            debit("2025-01-10", "10.00", "FXSCATTER"),
            debit("2025-02-10", "11.50", "FXSCATTER"),
            debit("2025-03-10", "9.20", "FXSCATTER"),
            debit("2025-04-10", "10.80", "FXSCATTER"),
            debit("2025-05-10", "10.10", "FXSCATTER"),
        ];
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert_eq!(audit.subscriptions.len(), 1);
        let s = &audit.subscriptions[0];
        assert!(s.amount_varies(), "amounts do vary (wide amplitude)");
        assert_eq!(s.first_amount, dec("10.00"));
        assert_eq!(s.last_amount, dec("10.10"));
        // Amplitude is wide, but the directional first->last move is < 10%.
        assert!(
            !audit
                .flags
                .iter()
                .any(|f| matches!(f, ReviewFlag::WidePriceRange { .. })),
            "FX scatter that returns to its start must NOT be flagged"
        );
    }

    // -----------------------------------------------------------------------
    // M5: a same-day second charge of a DIFFERENT amount (within one cluster)
    // must still feed the amount stats (min/max/representative), not be dropped,
    // while the dates are collapsed for cadence.
    // -----------------------------------------------------------------------

    #[test]
    fn same_day_different_amount_feeds_amount_stats() {
        // Five monthly £10.00 charges, plus an extra £11.00 charge on the SAME day
        // as the March charge. Cadence sees 5 dates (monthly); the amount stats
        // must include the £11.00 -> amount_max = 11.00.
        let txs = vec![
            debit("2025-01-15", "10.00", "RETRYSUB"),
            debit("2025-02-15", "10.00", "RETRYSUB"),
            debit("2025-03-15", "10.00", "RETRYSUB"),
            debit("2025-04-15", "10.00", "RETRYSUB"),
            debit("2025-05-15", "10.00", "RETRYSUB"),
            // Same-day, different amount (within the 25% cluster).
            debit("2025-03-15", "11.00", "RETRYSUB"),
        ];
        let audit = audit(&txs, DateFilter::default(), DetectOptions::default());
        assert_eq!(audit.subscriptions.len(), 1);
        let s = &audit.subscriptions[0];
        assert_eq!(s.cadence, Cadence::Monthly);
        // Dates collapsed to 5 for cadence...
        assert_eq!(s.occurrences, 5);
        // ...but the same-day £11.00 is NOT dropped from the stats.
        assert_eq!(s.amount_min, dec("10.00"));
        assert_eq!(s.amount_max, dec("11.00"));
        // Median of [10,10,10,10,10,11] is £10.00.
        assert_eq!(s.amount, dec("10.00"));
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
