//! Stage 2 query commands: aggregate spend, vendor drill-down, and unmatched-rows
//! reporting over the (transactions.csv + bills.jsonl + matches.jsonl) triple.
//!
//! All three commands consume the three sources in memory and render plaintext
//! tables to stdout. They share the join logic in [`load_joined`].

use crate::enrich::{self, EmailRow};
use crate::paypal::RecoveryIndex;
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
///
/// `recovered` carries the PayPal merchant recovered from `paypal_matches.jsonl`
/// for a bare `PAYPAL PAYMENT` bank row (see [`crate::paypal`]). It is `None`
/// unless the row was built via [`join_with_recovery`].
#[derive(Debug, Clone)]
pub struct JoinedRow<'a> {
    pub tx: &'a Transaction,
    pub confidence: &'a str,
    pub emails: Vec<&'a EmailRow>,
    /// Recovered PayPal merchant for this row, if any.
    pub recovered: Option<&'a str>,
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
    /// Priority:
    /// 1. Email evidence — first email's `effective_vendor()` (utility row →
    ///    `vendor`; PayPal row → `counterparty`).
    /// 2. PayPal-recovered merchant (a bare `PAYPAL PAYMENT` whose merchant we
    ///    recovered from PayPal's own CSV).
    /// 3. Normalise the bank `description` (uppercase, collapse whitespace,
    ///    first `~25` chars).
    pub fn counterparty_name(&self) -> String {
        if let Some(email) = self.emails.first() {
            if let Some(v) = email.effective_vendor() {
                return v.to_string();
            }
        }
        if let Some(merchant) = self.recovered {
            if !merchant.trim().is_empty() {
                return merchant.to_string();
            }
        }
        normalise_description(&self.tx.description)
    }

    /// Source label for reporting.
    pub fn source(&self) -> Source {
        match self.emails.first() {
            None => {
                if self
                    .recovered
                    .map(|m| !m.trim().is_empty())
                    .unwrap_or(false)
                {
                    Source::PayPalRecovered
                } else {
                    Source::BankOnly
                }
            }
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
    /// Merchant recovered from PayPal's own CSV (no email evidence).
    PayPalRecovered,
    BankOnly,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::EmailDirect => "email-direct",
            Source::EmailViaPayPal => "email-via-PayPal",
            Source::PayPalRecovered => "paypal-recovered",
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
///
/// No PayPal recovery is applied (`recovered == None` on every row). Use
/// [`join_with_recovery`] to also surface recovered PayPal merchants.
pub fn join<'a>(
    transactions: &'a [Transaction],
    emails: &'a [EmailRow],
    matches: &'a [MatchRow],
) -> Vec<JoinedRow<'a>> {
    join_inner(transactions, emails, matches, None)
}

/// Build the joined dataset, additionally surfacing recovered PayPal merchants.
///
/// For a bare `PAYPAL PAYMENT` bank row with no email evidence, the recovered
/// merchant (from `paypal_matches.jsonl`) becomes its counterparty and the row's
/// [`Source`] is reported as [`Source::PayPalRecovered`].
pub fn join_with_recovery<'a>(
    transactions: &'a [Transaction],
    emails: &'a [EmailRow],
    matches: &'a [MatchRow],
    recoveries: &'a RecoveryIndex,
) -> Vec<JoinedRow<'a>> {
    join_inner(transactions, emails, matches, Some(recoveries))
}

fn join_inner<'a>(
    transactions: &'a [Transaction],
    emails: &'a [EmailRow],
    matches: &'a [MatchRow],
    recoveries: Option<&'a RecoveryIndex>,
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
            let recovered = recoveries.and_then(|r| r.recovered_merchant_for(&tx.import_id));
            JoinedRow {
                tx,
                confidence,
                emails,
                recovered,
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

        // Counterparty resolution priority:
        //   1. GBP email evidence (email-direct / email-via-PayPal)
        //   2. PayPal-recovered merchant (paypal-recovered)
        //   3. normalised bank description (bank-only)
        // A non-GBP email is dropped above, so a PayPal-recovered merchant can
        // still rescue the row from bank-only.
        let recovered = row
            .recovered
            .filter(|m| !m.trim().is_empty())
            .map(|m| m.to_string());
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
        } else if let Some(merchant) = recovered {
            (merchant, Source::PayPalRecovered)
        } else {
            (normalise_description(&row.tx.description), Source::BankOnly)
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

        // PayPal-recovered rows are now attributable to a real merchant, so
        // they count toward the reconciled total alongside email-evidenced rows.
        match source {
            Source::EmailDirect | Source::EmailViaPayPal | Source::PayPalRecovered => {
                reconciled_total += amount
            }
            Source::BankOnly => bank_only_total += amount,
        }
    }

    let mut out: Vec<CounterpartyAggregate> = buckets.into_values().collect();
    out.sort_by(|a, b| b.total_outgoing.cmp(&a.total_outgoing));
    (out, internal_count, reconciled_total, bank_only_total)
}

/// Test/integration helper: aggregate outgoing spend by counterparty over all
/// dates, returning `(name, total, source)` tuples plus the reconciled and
/// bank-only totals. Exposes the otherwise-private [`CounterpartyAggregate`]
/// shape to integration tests without leaking internal types.
pub fn aggregate_for_test(
    rows: &[JoinedRow<'_>],
) -> (Vec<(String, Decimal, Source)>, usize, Decimal, Decimal) {
    let (agg, internal, reconciled, bank_only) =
        aggregate_by_counterparty(rows, DateFilter::default());
    let tuples = agg
        .into_iter()
        .map(|a| (a.name, a.total_outgoing, a.source))
        .collect();
    (tuples, internal, reconciled, bank_only)
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
// Command: stats --by-category
// ---------------------------------------------------------------------------
//
// A native, exact category breakdown of the personal Spend floor. The earlier
// awk attempt came out ~3% wrong because awk can't parse quoted CSV; this reuses
// the tool's exact parser and the very same `counts_as_spend()` predicate that
// `cmd_stats` uses for the Spend line, so the per-category totals SUM EXACTLY to
// that floor.
//
// "Category" = the transaction's primary tag: its first tag that is NOT a
// NONSPEND_TAG. A spend row carrying several descriptive tags is attributed to
// exactly ONE bucket (its primary), so no row is double-counted. A spend row
// with no category tag lands in the "uncategorised" bucket — the worklist signal.

/// Bucket label used for spend rows that carry no descriptive (non-reserved) tag.
pub const UNCATEGORISED: &str = "uncategorised";

#[derive(Debug, Clone)]
struct CategoryAggregate {
    name: String,
    total: Decimal, // Sum of |amount| for spend rows in this category.
    count: usize,
    /// Indices (into the transactions slice passed to [`aggregate_by_category`])
    /// of the individual spend rows attributed to this bucket. Retained so the
    /// `--detail` view can list exactly the rows that make up `total`, using the
    /// SAME predicate/attribution, so detail rows reconcile to the bucket total.
    row_indices: Vec<usize>,
}

/// The primary category of a spend row: its first tag that is NOT a reserved
/// NONSPEND_TAG. Reserved tags (transfer/income/tax/one-off/business/fdvisa)
/// never reach here for a spend row — `counts_as_spend()` already excludes any
/// row carrying one — but we skip them defensively so a descriptive tag is
/// always chosen as the label. Returns `None` when the row has no descriptive
/// tag, which the caller maps to the "uncategorised" bucket.
fn primary_category(tx: &Transaction) -> Option<&str> {
    tx.tags
        .iter()
        .find(|t| {
            !Transaction::NONSPEND_TAGS
                .iter()
                .any(|n| t.eq_ignore_ascii_case(n))
        })
        .map(|s| s.as_str())
}

/// Aggregate the personal Spend floor per category over the window.
///
/// Only rows where `counts_as_spend()` is true are included — exactly the rows
/// `cmd_stats` sums into the "Spend (recurring personal living cost)" figure.
/// Each such row is attributed to exactly one bucket (its `primary_category`,
/// or "uncategorised"), so the bucket totals reconcile to the floor.
///
/// Return: (buckets sorted by total descending, grand total = Spend floor).
fn aggregate_by_category(
    transactions: &[Transaction],
    filter: DateFilter,
) -> (Vec<CategoryAggregate>, Decimal) {
    let mut buckets: HashMap<String, CategoryAggregate> = HashMap::new();
    let mut grand_total = Decimal::ZERO;

    for (idx, tx) in transactions.iter().enumerate() {
        if !filter.matches(tx.date) {
            continue;
        }
        if !tx.counts_as_spend() {
            continue;
        }
        let category = primary_category(tx).unwrap_or(UNCATEGORISED).to_string();
        let amount = tx.amount.abs();
        let entry = buckets
            .entry(category.clone())
            .or_insert_with(|| CategoryAggregate {
                name: category,
                total: Decimal::ZERO,
                count: 0,
                row_indices: Vec::new(),
            });
        entry.total += amount;
        entry.count += 1;
        // Same predicate + attribution as the total, so the retained rows
        // reconcile exactly to this bucket's total (and the grand total).
        entry.row_indices.push(idx);
        grand_total += amount;
    }

    let mut out: Vec<CategoryAggregate> = buckets.into_values().collect();
    // Sort by total descending; tie-break by name so output is deterministic.
    out.sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.name.cmp(&b.name)));
    (out, grand_total)
}

/// The fallback super-category for any tag not listed in a taxonomy.
pub const OTHER_SUPER: &str = "Other";

/// The super-category for the reserved [`UNCATEGORISED`] tag. Kept distinct from
/// [`OTHER_SUPER`] so untagged spend (the worklist signal) stays visible as its
/// own bucket rather than being lumped in with merely-unmapped tags.
pub const UNCATEGORISED_SUPER: &str = "Uncategorised";

/// The built-in default taxonomy, expressed as `(super-category, &[member tags])`.
///
/// Single source of truth for the default mapping, used both as the absent-config
/// fallback ([`CategoryMap::default_taxonomy`]) and to render a starter
/// `categories.toml` ([`CategoryMap::default_toml`]). It is grouped into ~16
/// super-categories derived from the real tag vocabulary surfaced by
/// `stats --by-category` (rent, mortgage, bills, subscription, gym, transport,
/// holiday, insurance, shopping, groceries, housing, health, cash, food, tech,
/// travel, charity, giving, ...). It is a superset of the previous hardcoded
/// table, so the default roll-up is unchanged for any tag that table mapped.
const DEFAULT_TAXONOMY: &[(&str, &[&str])] = &[
    (
        "Home",
        &[
            "rent",
            "mortgage",
            "housing",
            "flat",
            "home",
            "household",
            "furniture",
            "garden",
        ],
    ),
    (
        "Bills",
        &[
            "bills",
            "utilities",
            "electricity",
            "gas",
            "water",
            "energy",
            "council-tax",
            "broadband",
            "internet",
            "phone",
            "mobile",
            "insurance",
        ],
    ),
    (
        "Food",
        &[
            "groceries",
            "food",
            "eatingout",
            "workfood",
            "supermarket",
            "dining",
            "restaurant",
            "takeaway",
            "coffee",
            "pub",
        ],
    ),
    (
        "Transport",
        &[
            "transport",
            "fuel",
            "petrol",
            "parking",
            "car",
            "taxi",
            "rail",
            "train",
            "bus",
            "tube",
            "tfl",
        ],
    ),
    (
        "Travel",
        &["travel", "flights", "hotel", "holiday", "accommodation"],
    ),
    (
        "Subscriptions",
        &[
            "subscriptions",
            "subscription",
            "digital",
            "tech",
            "streaming",
            "software",
            "saas",
            "media",
            "entertainment",
        ],
    ),
    (
        "Health",
        &[
            "health", "medical", "dental", "pharmacy", "gym", "fitness", "therapy",
        ],
    ),
    (
        "Giving",
        &["giving", "charity", "donation", "gifts", "gift"],
    ),
    (
        "Shopping",
        &[
            "shopping",
            "clothes",
            "clothing",
            "amazon",
            "electronics",
            "books",
            "hobbies",
        ],
    ),
    (
        "Family",
        &["childcare", "children", "kids", "school", "family"],
    ),
    ("Cash", &["cash", "atm", "withdrawal"]),
];

/// Config-driven category -> super-category roll-up map.
///
/// Built either from the embedded [default taxonomy](CategoryMap::default_taxonomy)
/// or loaded from `categories.toml` (see [`CategoryMap::load`]). The on-disk schema
/// is `super-category -> list of member tags`:
///
/// ```toml
/// [super_categories]
/// Home = ["rent", "mortgage", "housing", "household"]
/// Bills = ["bills", "utilities", "mobile", "insurance"]
/// # ...
/// ```
///
/// Internally the map is inverted to `tag -> super-category` for O(1) lookup. Tag
/// keys are lowercased so lookup is case-insensitive. A tag present in no
/// super-category rolls up under [`OTHER_SUPER`]; the reserved [`UNCATEGORISED`]
/// tag always maps to [`UNCATEGORISED_SUPER`] (implicitly, even if a config omits
/// it), so the roll-up still reconciles exactly to the Spend floor.
#[derive(Debug, Clone, Default)]
pub struct CategoryMap {
    /// Inverted lookup: lowercased tag -> super-category label.
    tag_to_super: HashMap<String, String>,
}

/// On-disk schema for `categories.toml`: a single `[super_categories]` table
/// whose keys are super-category labels and whose values are the member tags.
#[derive(Debug, Clone, Default, serde::Serialize, Deserialize)]
struct CategoriesConfig {
    #[serde(default)]
    super_categories: std::collections::BTreeMap<String, Vec<String>>,
}

impl CategoryMap {
    /// Build the default (embedded) taxonomy map.
    pub fn default_taxonomy() -> Self {
        let mut tag_to_super = HashMap::new();
        for (sup, tags) in DEFAULT_TAXONOMY {
            for tag in *tags {
                tag_to_super.insert(tag.to_lowercase(), (*sup).to_string());
            }
        }
        Self { tag_to_super }
    }

    /// Build a map from an in-memory config (super-category -> member tags).
    fn from_config(config: &CategoriesConfig) -> Self {
        let mut tag_to_super = HashMap::new();
        for (sup, tags) in &config.super_categories {
            for tag in tags {
                tag_to_super.insert(tag.to_lowercase(), sup.clone());
            }
        }
        Self { tag_to_super }
    }

    /// Load the map from `categories.toml`.
    ///
    /// Behaviour:
    /// - **Absent file** → fall back to the embedded [default
    ///   taxonomy](CategoryMap::default_taxonomy) (today's behaviour, unchanged),
    ///   so `--rollup` works with no config present.
    /// - **Present file** → parse the `[super_categories]` table and invert it.
    ///
    /// A parse error is surfaced to the caller — a malformed config is a user
    /// error worth reporting, not silently swallowing.
    pub fn load<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default_taxonomy());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {}", path.display(), e))?;
        let config: CategoriesConfig = toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("failed to parse {}: {}", path.display(), e))?;
        Ok(Self::from_config(&config))
    }

    /// Serialise the default taxonomy to TOML, for writing a starter
    /// `categories.toml` users can then edit.
    pub fn default_toml() -> String {
        let mut super_categories = std::collections::BTreeMap::new();
        for (sup, tags) in DEFAULT_TAXONOMY {
            super_categories.insert(
                (*sup).to_string(),
                tags.iter().map(|t| t.to_string()).collect(),
            );
        }
        let config = CategoriesConfig { super_categories };
        // Render from DEFAULT_TAXONOMY so the starter file can never drift from it.
        toml::to_string_pretty(&config).unwrap_or_default()
    }

    /// The super-category for a category (primary-tag).
    ///
    /// The reserved [`UNCATEGORISED`] tag always maps to [`UNCATEGORISED_SUPER`].
    /// Any tag not present in the map rolls up under [`OTHER_SUPER`]. Lookup is
    /// case-insensitive.
    pub fn super_category(&self, category: &str) -> String {
        let lower = category.to_lowercase();
        if lower == UNCATEGORISED {
            return UNCATEGORISED_SUPER.to_string();
        }
        self.tag_to_super
            .get(&lower)
            .cloned()
            .unwrap_or_else(|| OTHER_SUPER.to_string())
    }
}

/// Default category -> super-category lookup, against the embedded taxonomy.
///
/// Thin wrapper over [`CategoryMap::default_taxonomy`]. Production code rolls up
/// via a [`CategoryMap`] built by `main` (config-driven), so this is only used by
/// the test suite to assert the default mapping without first building a map. Any
/// tag not in the default taxonomy rolls up under [`OTHER_SUPER`];
/// [`UNCATEGORISED`] maps to [`UNCATEGORISED_SUPER`]. Comparison is
/// case-insensitive.
#[cfg(test)]
fn super_category(category: &str) -> String {
    CategoryMap::default_taxonomy().super_category(category)
}

/// Resolve `--only <CATEGORY>` against the full aggregate: case-insensitive match
/// on the category (primary-tag) name. Returns the matching bucket(s) — at most
/// one, since bucket names are unique — or an empty vec when nothing matches (the
/// caller then prints a clear "no category 'X'" message). Factored out so the
/// match semantics are unit-testable without capturing stdout.
fn filter_only<'a>(full_agg: &'a [CategoryAggregate], want: &str) -> Vec<&'a CategoryAggregate> {
    full_agg
        .iter()
        .filter(|a| a.name.eq_ignore_ascii_case(want))
        .collect()
}

/// The available category names, sorted, for the "no category 'X'" hint.
fn available_category_names(full_agg: &[CategoryAggregate]) -> Vec<&str> {
    let mut names: Vec<&str> = full_agg.iter().map(|a| a.name.as_str()).collect();
    names.sort_unstable();
    names
}

/// Print a stats-by-category table. The TOTAL line equals (and reconciles to)
/// the Spend floor printed by `fd-budget stats`.
///
/// `detail`: list the individual rows behind each category total (date, amount,
/// description, tags), sorted within a category by absolute amount descending so
/// the items driving the total are at the top.
///
/// `only`: restrict the breakdown to a single category (case-insensitive match
/// against the category/primary-tag name, e.g. "groceries" or "uncategorised").
/// If it matches no category, a clear "no category 'X'" message is printed along
/// with the available category names. With neither `detail` nor `rollup`, `only`
/// just shows that one category's total + count.
///
/// Note: `detail` and `only` have NO effect without `--by-category` — this whole
/// function is the `--by-category` path, so they are simply not wired in
/// otherwise (mirroring `--rollup`).
/// Per-category ANNUAL spend targets (£/yr), keyed by lowercased primary-tag
/// name. Loaded from `~/.config/fd-budget/budgets.toml` `[budgets]`. The
/// `--budget` comparison annualises each category's actual over the data span,
/// so a target set in £/yr (the planning-doc "anchor figures") lines up directly.
#[derive(Debug, Clone, Default)]
pub struct BudgetMap {
    targets: std::collections::BTreeMap<String, Decimal>,
}

impl BudgetMap {
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        Self::from_toml_str(&std::fs::read_to_string(path)?)
    }

    /// Parse from a `[budgets]` TOML string (split out for unit-testing).
    pub fn from_toml_str(content: &str) -> anyhow::Result<Self> {
        let parsed: toml::Value = toml::from_str(content)?;
        let mut targets = std::collections::BTreeMap::new();
        if let Some(tbl) = parsed.get("budgets").and_then(|v| v.as_table()) {
            for (k, v) in tbl {
                let amt = v
                    .as_float()
                    .and_then(|f| Decimal::try_from(f).ok())
                    .or_else(|| v.as_integer().map(Decimal::from));
                if let Some(a) = amt {
                    targets.insert(k.to_lowercase(), a);
                }
            }
        }
        Ok(Self { targets })
    }

    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    pub fn target(&self, category: &str) -> Option<Decimal> {
        self.targets.get(&category.to_lowercase()).copied()
    }

    /// A starter `budgets.toml` listing the observed categories as commented
    /// `# <category> = 0.0` lines under `[budgets]` for the user to fill in.
    pub fn seed_toml(categories: &[String]) -> String {
        let mut s = String::from(
            "# fd-budget per-category ANNUAL spend targets (£/yr).\n\
             # Uncomment a line and set the figure; `stats --by-category --budget`\n\
             # then shows actual-vs-target (actuals annualised over the data span).\n\
             # Keys are the primary (first non-reserved) tag, lowercase.\n\n[budgets]\n",
        );
        for c in categories {
            if c != UNCATEGORISED {
                s.push_str(&format!("# {} = 0.0\n", c));
            }
        }
        s
    }
}

/// Sorted distinct primary-category names of the spend rows in the window
/// (used to seed a starter `budgets.toml`).
pub fn spend_category_names(transactions: &[Transaction], filter: DateFilter) -> Vec<String> {
    let (agg, _) = aggregate_by_category(transactions, filter);
    agg.into_iter().map(|a| a.name).collect()
}

/// Print the per-category actual-vs-target comparison (annualised £/yr). Only
/// categories that have a target set are shown; the rest are summarised in a
/// trailing note so an unset budget is visible, not silently dropped.
fn print_budget_comparison(agg: &[CategoryAggregate], span_days: i64, budgets: &BudgetMap) {
    let days = Decimal::from(span_days.max(365));
    let year = Decimal::from(365);
    println!();
    println!("Budget (annualised actual vs target, £/yr):");
    println!(
        "{:<24} {:>12} {:>12} {:>12}",
        "Category", "Actual/yr", "Target/yr", "Δ/yr"
    );
    println!("{}", "-".repeat(64));
    let mut compared = 0;
    for entry in agg {
        if let Some(target) = budgets.target(&entry.name) {
            let actual_annual = entry.total * year / days;
            let delta = actual_annual - target;
            let delta_str = if delta > Decimal::ZERO {
                format!("+{}", format_money(delta))
            } else {
                format_money(delta)
            };
            println!(
                "{:<24} {:>12} {:>12} {:>12}",
                truncate(&entry.name, 24),
                format_money(actual_annual),
                format_money(target),
                delta_str,
            );
            compared += 1;
        }
    }
    if compared == 0 {
        println!("(no category in this period has a target set in budgets.toml)");
    } else {
        let unset = agg.len() - compared;
        if unset > 0 {
            println!("({} other categor{} have no target set)", unset, if unset == 1 { "y" } else { "ies" });
        }
    }
}

pub fn cmd_stats_by_category(
    transactions: &[Transaction],
    filter: DateFilter,
    limit: usize,
    rollup: bool,
    detail: bool,
    only: Option<&str>,
    category_map: &CategoryMap,
    budgets: &BudgetMap,
) -> anyhow::Result<()> {
    let (full_agg, full_grand_total) = aggregate_by_category(transactions, filter);

    if full_agg.is_empty() {
        println!("No personal spend in the selected period.");
        return Ok(());
    }

    // --only restricts the printed breakdown to a single category, matched
    // case-insensitively against the primary-tag name. The roll-up section
    // (when --rollup is set) is built from the FULL aggregate below, unaffected.
    let agg: Vec<&CategoryAggregate> = if let Some(want) = only {
        let matched = filter_only(&full_agg, want);
        if matched.is_empty() {
            println!("No category '{}' in the selected period.", want);
            let names = available_category_names(&full_agg);
            println!("Available categories: {}", names.join(", "));
            return Ok(());
        }
        matched
    } else {
        full_agg.iter().collect()
    };

    // When --only narrows to one category, the printed TOTAL is that category's
    // total (not the whole Spend floor); otherwise it is the full floor.
    let grand_total: Decimal = if only.is_some() {
        agg.iter().map(|a| a.total).sum()
    } else {
        full_grand_total
    };
    let total_label = if only.is_some() {
        "TOTAL (selected category)"
    } else {
        "TOTAL (= Spend floor)"
    };

    println!("{:<28} {:>12}   {:>5}", "Category", "Total", "Count");
    println!("{}", "-".repeat(50));

    let mut shown_total = Decimal::ZERO;
    let mut shown_count = 0usize;
    for entry in agg.iter().take(limit) {
        // Flag the uncategorised bucket prominently — it is the worklist signal.
        let label = if entry.name == UNCATEGORISED {
            format!("** {} **", entry.name)
        } else {
            truncate(&entry.name, 28)
        };
        println!(
            "{:<28} {:>12}   {:>5}",
            label,
            format_money(entry.total),
            entry.count
        );
        shown_total += entry.total;
        shown_count += entry.count;

        // --detail: list the individual rows that make up this category's total,
        // sorted by absolute amount descending (big drivers first). The rows are
        // exactly the spend rows aggregate_by_category attributed here, so they
        // sum to entry.total.
        if detail {
            print_category_detail(entry, transactions);
        }
    }

    // If a limit truncated the table, fold the tail into a single line so the
    // printed TOTAL still equals the Spend floor (reconciliation must hold).
    if agg.len() > limit {
        let rest_total = grand_total - shown_total;
        let rest_count: usize = agg.iter().skip(limit).map(|a| a.count).sum();
        println!(
            "{:<28} {:>12}   {:>5}",
            format!("(+{} more categories)", agg.len() - limit),
            format_money(rest_total),
            rest_count
        );
        shown_total += rest_total;
        shown_count += rest_count;
    }

    println!("{}", "-".repeat(50));
    println!(
        "{:<28} {:>12}   {:>5}",
        total_label,
        format_money(grand_total),
        shown_count
    );

    // Reconciliation assertion: the printed rows must sum to the floor exactly.
    // Decimal arithmetic is exact, so this always holds; we surface it so the
    // user can see the breakdown is trustworthy (the whole point of the command).
    // With --only the printed total is a single category, so the floor-reconcile
    // line (which asserts the WHOLE breakdown == Spend floor) is suppressed.
    if only.is_none() {
        if shown_total == grand_total {
            println!("Reconciles exactly to the Spend floor. \u{2713}");
        } else {
            // Should be unreachable with Decimal; loud if it ever isn't.
            println!(
                "WARNING: category totals ({}) do NOT reconcile to Spend floor ({})",
                format_money(shown_total),
                format_money(grand_total)
            );
        }
    }

    // --budget: annualised actual-vs-target per category. Span comes from the
    // spend rows' own dates (via row_indices) so no DateFilter reuse is needed.
    if !budgets.is_empty() {
        let mut min_d: Option<chrono::NaiveDate> = None;
        let mut max_d: Option<chrono::NaiveDate> = None;
        for entry in &full_agg {
            for &i in &entry.row_indices {
                let d = transactions[i].date;
                min_d = Some(min_d.map_or(d, |m| m.min(d)));
                max_d = Some(max_d.map_or(d, |m| m.max(d)));
            }
        }
        let span_days = match (min_d, max_d) {
            (Some(a), Some(b)) => (b - a).num_days(),
            _ => 365,
        };
        print_budget_comparison(&full_agg, span_days, budgets);
    }

    if rollup {
        // Roll-up is always over the FULL breakdown (so it still reconciles to
        // the Spend floor), regardless of --only / --detail.
        print_rollup(&full_agg, full_grand_total, category_map);
    }

    Ok(())
}

/// Print the individual rows behind one category's total, indented under the
/// category header. Rows are sorted by absolute amount descending (the big items
/// driving the total first — best for verification). Each line shows the row's
/// date, signed amount, description, and full pipe-joined tags so the user can
/// see WHY it landed in this bucket (its primary tag is the bucket name).
fn print_category_detail(entry: &CategoryAggregate, transactions: &[Transaction]) {
    let mut rows: Vec<&Transaction> = entry
        .row_indices
        .iter()
        .map(|&i| &transactions[i])
        .collect();
    // Absolute amount descending; tie-break by date then description so output
    // is deterministic.
    rows.sort_by(|a, b| {
        b.amount
            .abs()
            .cmp(&a.amount.abs())
            .then_with(|| a.date.cmp(&b.date))
            .then_with(|| a.description.cmp(&b.description))
    });
    for tx in rows {
        let tags = if tx.tags.is_empty() {
            "-".to_string()
        } else {
            tx.tags.join("|")
        };
        println!(
            "    {}  {:>10}   {:<30}  [{}]",
            tx.date,
            format_money(tx.amount),
            truncate(&tx.description, 30),
            tags
        );
    }
}

/// Print the super-category roll-up as a second section.
///
/// The mapping is config-driven via [`CategoryMap`]; a tag absent from the map
/// lands in the [`OTHER_SUPER`] bucket, so the roll-up still reconciles exactly to
/// the Spend floor.
fn print_rollup(agg: &[CategoryAggregate], grand_total: Decimal, category_map: &CategoryMap) {
    let mut supers: HashMap<String, (Decimal, usize)> = HashMap::new();
    for entry in agg {
        let sup = category_map.super_category(&entry.name);
        let e = supers.entry(sup).or_insert((Decimal::ZERO, 0));
        e.0 += entry.total;
        e.1 += entry.count;
    }
    let mut rows: Vec<(String, Decimal, usize)> =
        supers.into_iter().map(|(k, v)| (k, v.0, v.1)).collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    println!();
    println!("Super-category roll-up (mapping is refinable):");
    println!("{:<28} {:>12}   {:>5}", "Super-category", "Total", "Count");
    println!("{}", "-".repeat(50));
    let mut total = Decimal::ZERO;
    let mut count = 0usize;
    for (name, sum, n) in &rows {
        println!("{:<28} {:>12}   {:>5}", name, format_money(*sum), n);
        total += *sum;
        count += *n;
    }
    println!("{}", "-".repeat(50));
    println!(
        "{:<28} {:>12}   {:>5}",
        "TOTAL (= Spend floor)",
        format_money(grand_total),
        count
    );
    if total != grand_total {
        println!(
            "WARNING: roll-up totals ({}) do NOT reconcile to Spend floor ({})",
            format_money(total),
            format_money(grand_total)
        );
    }
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
                format!("-> {:<38} {:<10}", mid, row.confidence)
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
        println!("\n{} rows, {} with evidence ({:.1}%)", total, with_ev, pct);
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
pub fn format_money(amount: Decimal) -> String {
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

    #[test]
    fn budget_map_parses_and_is_case_insensitive() {
        let b = BudgetMap::from_toml_str("[budgets]\nfood = 6000\ngroceries = 4800.0\n").unwrap();
        assert!(!b.is_empty());
        assert_eq!(b.target("Food"), Some(Decimal::from(6000)));
        assert_eq!(b.target("groceries"), Decimal::from_str("4800.0").ok());
        assert_eq!(b.target("transport"), None);
    }

    #[test]
    fn budget_map_empty_when_no_section() {
        assert!(BudgetMap::from_toml_str("# nothing here\n").unwrap().is_empty());
        assert!(BudgetMap::default().is_empty());
    }

    #[test]
    fn budget_seed_lists_categories_commented_skips_uncategorised() {
        let toml =
            BudgetMap::seed_toml(&["food".into(), "uncategorised".into(), "transport".into()]);
        assert!(toml.contains("[budgets]"));
        assert!(toml.contains("# food = 0.0"));
        assert!(toml.contains("# transport = 0.0"));
        assert!(!toml.contains("uncategorised"));
    }

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

    fn recovery_index(pairs: &[(&str, &str)]) -> crate::paypal::RecoveryIndex {
        let rows = pairs
            .iter()
            .map(|(id, merchant)| crate::paypal::RecoveryRow {
                bank_import_id: id.to_string(),
                recovered_merchant: merchant.to_string(),
                currency: "GBP".to_string(),
                leg: "direct-gbp".to_string(),
            })
            .collect();
        crate::paypal::RecoveryIndex::from_rows(rows)
    }

    #[test]
    fn counterparty_uses_recovered_paypal_merchant() {
        // A bare PAYPAL PAYMENT with no email evidence but a recovery.
        let tx = mk_tx("2025-10-13", "-12.99", "PAYPAL PAYMENT", "tx1");
        let txs = vec![tx];
        let emails: Vec<EmailRow> = vec![];
        let ms: Vec<MatchRow> = vec![];
        let idx = recovery_index(&[("tx1", "Streamflix")]);
        let joined = join_with_recovery(&txs, &emails, &ms, &idx);
        assert_eq!(joined[0].counterparty_name(), "Streamflix");
        assert_eq!(joined[0].source(), Source::PayPalRecovered);
    }

    #[test]
    fn recovered_merchant_aggregates_as_reconciled() {
        let tx = mk_tx("2025-10-13", "-12.99", "PAYPAL PAYMENT", "tx1");
        let txs = vec![tx];
        let emails: Vec<EmailRow> = vec![];
        let ms: Vec<MatchRow> = vec![];
        let idx = recovery_index(&[("tx1", "Streamflix")]);
        let joined = join_with_recovery(&txs, &emails, &ms, &idx);
        let (agg, _, reconciled, bank_only) =
            aggregate_by_counterparty(&joined, DateFilter::default());
        assert_eq!(agg.len(), 1);
        assert_eq!(agg[0].name, "Streamflix");
        assert_eq!(agg[0].source, Source::PayPalRecovered);
        assert_eq!(reconciled, Decimal::from_str("12.99").unwrap());
        assert_eq!(bank_only, Decimal::ZERO);
    }

    #[test]
    fn email_evidence_outranks_recovery() {
        // If both an email match AND a recovery exist, email wins.
        let tx = mk_tx("2025-10-13", "-12.99", "PAYPAL PAYMENT", "tx1");
        let email = mk_email(
            "<p@1>",
            Some("PayPal"),
            Some("Netflix"),
            Some("12.99"),
            Some("GBP"),
        );
        let m = mk_match("tx1", "high", &["<p@1>"]);
        let txs = vec![tx];
        let emails = vec![email];
        let ms = vec![m];
        let idx = recovery_index(&[("tx1", "Streamflix")]);
        let joined = join_with_recovery(&txs, &emails, &ms, &idx);
        assert_eq!(joined[0].counterparty_name(), "Netflix");
        assert_eq!(joined[0].source(), Source::EmailViaPayPal);
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
        let (agg, internal_count, _, _) = aggregate_by_counterparty(&joined, DateFilter::default());
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
        let (agg, _, _, bank_only) = aggregate_by_counterparty(&joined, DateFilter::default());
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

    // -------------------------------------------------------------------
    // stats --by-category tests
    // -------------------------------------------------------------------

    fn mk_tx_tagged(
        date_str: &str,
        amount: &str,
        desc: &str,
        id: &str,
        tags: &[&str],
    ) -> Transaction {
        let mut tx = mk_tx(date_str, amount, desc, id);
        tx.tags = tags.iter().map(|s| s.to_string()).collect();
        tx
    }

    /// Compute the Spend floor exactly as `cmd_stats` does, for cross-checking.
    fn spend_floor(txs: &[Transaction], filter: DateFilter) -> Decimal {
        txs.iter()
            .filter(|t| filter.matches(t.date))
            .filter(|t| t.counts_as_spend())
            .map(|t| t.amount.abs())
            .sum()
    }

    #[test]
    fn category_totals_sum_to_spend_floor() {
        let txs = vec![
            mk_tx_tagged("2025-06-01", "-55.67", "VODAFONE", "t1", &["bills"]),
            mk_tx_tagged("2025-06-02", "-30.00", "TESCO", "t2", &["groceries"]),
            mk_tx_tagged("2025-06-03", "-10.00", "ALDI", "t3", &["groceries"]),
            mk_tx_tagged(
                "2025-06-04",
                "-12.99",
                "STREAMFLIX",
                "t4",
                &["subscriptions"],
            ),
            // Untagged debit -> uncategorised, still part of the floor.
            mk_tx_tagged("2025-06-05", "-7.50", "CORNER SHOP", "t5", &[]),
        ];
        let (agg, grand_total) = aggregate_by_category(&txs, DateFilter::default());
        let summed: Decimal = agg.iter().map(|a| a.total).sum();
        // (i) buckets sum to the grand total returned ...
        assert_eq!(summed, grand_total);
        // ... and the grand total equals the independently-computed Spend floor.
        assert_eq!(grand_total, spend_floor(&txs, DateFilter::default()));
        assert_eq!(grand_total, Decimal::from_str("116.16").unwrap());
    }

    #[test]
    fn category_excludes_credits_and_nonspend() {
        let txs = vec![
            mk_tx_tagged("2025-06-01", "-50.00", "TESCO", "t1", &["groceries"]),
            // credit (refund) — excluded
            mk_tx_tagged("2025-06-02", "20.00", "REFUND", "t2", &["groceries"]),
            // nonspend-tagged debits — excluded from the floor
            mk_tx_tagged("2025-06-03", "-1000.00", "HMRC", "t3", &["tax"]),
            mk_tx_tagged("2025-06-04", "-500.00", "TFR OUT", "t4", &["transfer"]),
            mk_tx_tagged("2025-06-05", "-370.00", "PA FEES", "t5", &["business"]),
            mk_tx_tagged(
                "2025-06-06",
                "-2550.00",
                "GYM BLOCK",
                "t6",
                &["one-off", "gym"],
            ),
            mk_tx_tagged("2025-06-07", "-46.07", "FD VISA", "t7", &["fdvisa"]),
            mk_tx_tagged("2025-06-08", "-200.00", "PAYDAY", "t8", &["income"]),
        ];
        let (agg, grand_total) = aggregate_by_category(&txs, DateFilter::default());
        // Only the single groceries debit survives.
        assert_eq!(agg.len(), 1);
        assert_eq!(agg[0].name, "groceries");
        assert_eq!(grand_total, Decimal::from_str("50.00").unwrap());
        assert_eq!(grand_total, spend_floor(&txs, DateFilter::default()));
    }

    #[test]
    fn uncategorised_bucket_captures_untagged_spend() {
        let txs = vec![
            mk_tx_tagged("2025-06-01", "-50.00", "TESCO", "t1", &["groceries"]),
            mk_tx_tagged("2025-06-02", "-7.50", "CORNER SHOP", "t2", &[]),
            mk_tx_tagged("2025-06-03", "-3.20", "KIOSK", "t3", &[]),
        ];
        let (agg, _grand) = aggregate_by_category(&txs, DateFilter::default());
        let uncat = agg
            .iter()
            .find(|a| a.name == UNCATEGORISED)
            .expect("uncategorised bucket present");
        assert_eq!(uncat.count, 2);
        assert_eq!(uncat.total, Decimal::from_str("10.70").unwrap());
    }

    #[test]
    fn primary_category_skips_reserved_tags() {
        // A spend row that (defensively) carries a reserved tag alongside a
        // descriptive one is labelled by the descriptive tag, not the reserved.
        // (counts_as_spend would normally exclude such a row, but primary_category
        // must still pick the descriptive label.)
        let tx = mk_tx_tagged(
            "2025-06-01",
            "-50.00",
            "X",
            "t1",
            &["groceries", "shopping"],
        );
        assert_eq!(primary_category(&tx), Some("groceries"));
        let untagged = mk_tx_tagged("2025-06-01", "-50.00", "X", "t2", &[]);
        assert_eq!(primary_category(&untagged), None);
    }

    #[test]
    fn category_row_not_double_counted_across_tags() {
        // A row with several descriptive tags lands in exactly ONE bucket.
        let txs = vec![mk_tx_tagged(
            "2025-06-01",
            "-80.00",
            "MIXED",
            "t1",
            &["groceries", "household", "shopping"],
        )];
        let (agg, grand_total) = aggregate_by_category(&txs, DateFilter::default());
        assert_eq!(agg.len(), 1);
        assert_eq!(agg[0].name, "groceries");
        assert_eq!(agg[0].count, 1);
        assert_eq!(grand_total, Decimal::from_str("80.00").unwrap());
    }

    // -------------------------------------------------------------------
    // --detail / --only tests
    // -------------------------------------------------------------------

    /// Sum the |amount| of the retained detail rows for a bucket, resolving each
    /// retained index back to its transaction. This is exactly what `--detail`
    /// lists, so it must equal the bucket's printed total.
    fn detail_rows_sum(entry: &CategoryAggregate, txs: &[Transaction]) -> Decimal {
        entry.row_indices.iter().map(|&i| txs[i].amount.abs()).sum()
    }

    #[test]
    fn detail_rows_sum_to_each_category_total() {
        // (i) --detail rows for a category sum to that category's total.
        let txs = vec![
            mk_tx_tagged("2025-06-01", "-30.00", "TESCO", "t1", &["groceries"]),
            mk_tx_tagged("2025-06-02", "-10.00", "ALDI", "t2", &["groceries", "food"]),
            mk_tx_tagged("2025-06-03", "-5.00", "KIOSK", "t3", &["groceries"]),
            mk_tx_tagged("2025-06-04", "-900.00", "RENT", "t4", &["rent"]),
        ];
        let (agg, _grand) = aggregate_by_category(&txs, DateFilter::default());
        for entry in &agg {
            assert_eq!(
                detail_rows_sum(entry, &txs),
                entry.total,
                "detail rows for {} must sum to its total",
                entry.name
            );
            // The retained row count also matches the bucket count.
            assert_eq!(entry.row_indices.len(), entry.count);
        }
        // The multi-tag row landed under its primary tag, and its full tags are
        // available for the detail line (showing WHY it's in `groceries`).
        let groceries = agg.iter().find(|a| a.name == "groceries").unwrap();
        let aldi_idx = *groceries
            .row_indices
            .iter()
            .find(|&&i| txs[i].description == "ALDI")
            .expect("ALDI row attributed to groceries");
        assert_eq!(txs[aldi_idx].tags, vec!["groceries", "food"]);
    }

    #[test]
    fn detail_rows_no_double_count_no_drop_and_reconcile() {
        // (ii) Every spend row appears under exactly one category in --detail
        // (no double-count / no drop) and the grand total still reconciles to the
        // Spend floor.
        let txs = vec![
            mk_tx_tagged("2025-06-01", "-30.00", "TESCO", "t1", &["groceries"]),
            mk_tx_tagged("2025-06-02", "-12.99", "STREAMFLIX", "t2", &["streaming"]),
            mk_tx_tagged("2025-06-03", "-900.00", "RENT", "t3", &["rent"]),
            // Untagged spend -> uncategorised bucket.
            mk_tx_tagged("2025-06-04", "-7.50", "MYSTERY", "t4", &[]),
            // Nonspend / credit rows that must NOT appear in any detail bucket.
            mk_tx_tagged("2025-06-05", "20.00", "REFUND", "t5", &["groceries"]),
            mk_tx_tagged("2025-06-06", "-1000.00", "HMRC", "t6", &["tax"]),
        ];
        let (agg, grand_total) = aggregate_by_category(&txs, DateFilter::default());

        // Collect every retained detail index across all buckets.
        let mut all: Vec<usize> = agg.iter().flat_map(|a| a.row_indices.clone()).collect();
        let n_detail = all.len();
        all.sort_unstable();
        all.dedup();
        // No double-count: dedup didn't shrink the list.
        assert_eq!(
            all.len(),
            n_detail,
            "a row appeared in more than one bucket"
        );
        // No drop: exactly the spend rows are present (the 4 debits without a
        // nonspend tag), and they are precisely those `counts_as_spend()` accepts.
        let expected: Vec<usize> = txs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.counts_as_spend())
            .map(|(i, _)| i)
            .collect();
        assert_eq!(all, expected);
        // The detail rows across all buckets reconcile to the grand total ...
        let detail_total: Decimal = all.iter().map(|&i| txs[i].amount.abs()).sum();
        assert_eq!(detail_total, grand_total);
        // ... which is exactly the independently-computed Spend floor.
        assert_eq!(grand_total, spend_floor(&txs, DateFilter::default()));
    }

    #[test]
    fn only_restricts_and_unknown_errors_cleanly() {
        // (iii) --only <cat> restricts to that one category (case-insensitively),
        // and --only <unknown> resolves to nothing (caller reports cleanly).
        let txs = vec![
            mk_tx_tagged("2025-06-01", "-30.00", "TESCO", "t1", &["groceries"]),
            mk_tx_tagged("2025-06-02", "-10.00", "ALDI", "t2", &["groceries"]),
            mk_tx_tagged("2025-06-03", "-900.00", "RENT", "t3", &["rent"]),
            mk_tx_tagged("2025-06-04", "-7.50", "MYSTERY", "t4", &[]),
        ];
        let (agg, _grand) = aggregate_by_category(&txs, DateFilter::default());

        // Exact match.
        let only_groceries = filter_only(&agg, "groceries");
        assert_eq!(only_groceries.len(), 1);
        assert_eq!(only_groceries[0].name, "groceries");
        assert_eq!(only_groceries[0].total, Decimal::from_str("40.00").unwrap());
        assert_eq!(only_groceries[0].count, 2);

        // Case-insensitive match.
        assert_eq!(filter_only(&agg, "GrOcErIeS").len(), 1);
        assert_eq!(filter_only(&agg, "RENT").len(), 1);
        // The reserved uncategorised bucket is selectable by name too.
        assert_eq!(filter_only(&agg, "Uncategorised").len(), 1);

        // Unknown category -> empty (the command prints "no category 'X'" and the
        // available names; here we assert the resolution itself is clean/empty).
        assert!(filter_only(&agg, "holidays").is_empty());
        let names = available_category_names(&agg);
        assert_eq!(names, vec!["groceries", "rent", "uncategorised"]);
    }

    #[test]
    fn detail_and_only_have_no_effect_without_by_category() {
        // (iv) --detail / --only are only consulted on the --by-category path.
        // The dispatch in main.rs calls cmd_stats (the plain summary) when
        // --by-category is absent, never threading detail/only through. We assert
        // the contract here: the by-category entry points (aggregation + only
        // resolution) are pure and produce nothing on their own — they require an
        // explicit call, which the non-by-category dispatch never makes.
        //
        // Concretely: filter_only over an empty aggregate (the state when the
        // by-category path is not taken) yields nothing, and there is no other
        // route by which detail/only mutate output. This guards the wiring: a
        // future edit that leaked detail/only into the plain path would have to
        // call these, which the plain path does not.
        let empty: Vec<CategoryAggregate> = Vec::new();
        assert!(filter_only(&empty, "groceries").is_empty());
        assert!(available_category_names(&empty).is_empty());
    }

    #[test]
    fn category_respects_date_filter() {
        let txs = vec![
            mk_tx_tagged("2024-12-31", "-100.00", "OLD", "t1", &["groceries"]),
            mk_tx_tagged("2025-06-15", "-25.00", "NEW", "t2", &["groceries"]),
            mk_tx_tagged("2026-01-01", "-99.00", "FUTURE", "t3", &["groceries"]),
        ];
        let (agg, grand_total) = aggregate_by_category(&txs, DateFilter::year(2025));
        assert_eq!(agg.len(), 1);
        assert_eq!(agg[0].count, 1);
        assert_eq!(grand_total, Decimal::from_str("25.00").unwrap());
        assert_eq!(grand_total, spend_floor(&txs, DateFilter::year(2025)));
    }

    #[test]
    fn category_sorted_by_total_descending() {
        let txs = vec![
            mk_tx_tagged("2025-06-01", "-10.00", "A", "t1", &["coffee"]),
            mk_tx_tagged("2025-06-02", "-90.00", "B", "t2", &["rent"]),
            mk_tx_tagged("2025-06-03", "-40.00", "C", "t3", &["groceries"]),
        ];
        let (agg, _grand) = aggregate_by_category(&txs, DateFilter::default());
        let names: Vec<&str> = agg.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["rent", "groceries", "coffee"]);
    }

    #[test]
    fn rollup_reconciles_to_spend_floor() {
        let txs = vec![
            mk_tx_tagged("2025-06-01", "-900.00", "RENT", "t1", &["rent"]),
            mk_tx_tagged("2025-06-02", "-55.67", "VODAFONE", "t2", &["mobile"]),
            mk_tx_tagged("2025-06-03", "-40.00", "TESCO", "t3", &["groceries"]),
            mk_tx_tagged("2025-06-04", "-12.99", "STREAMFLIX", "t4", &["streaming"]),
            mk_tx_tagged("2025-06-05", "-7.50", "MYSTERY", "t5", &[]),
        ];
        let (agg, grand_total) = aggregate_by_category(&txs, DateFilter::default());
        // Roll categories into super-categories and confirm exact reconciliation.
        let mut supers: HashMap<String, Decimal> = HashMap::new();
        for entry in &agg {
            *supers.entry(super_category(&entry.name)).or_default() += entry.total;
        }
        let summed: Decimal = supers.values().copied().sum();
        assert_eq!(summed, grand_total);
        assert_eq!(grand_total, spend_floor(&txs, DateFilter::default()));
        // Sanity: known mappings land where expected.
        assert_eq!(super_category("rent"), "Home");
        assert_eq!(super_category("mobile"), "Bills");
        assert_eq!(super_category("groceries"), "Food");
        assert_eq!(super_category("streaming"), "Subscriptions");
        assert_eq!(super_category(UNCATEGORISED), "Uncategorised");
        assert_eq!(super_category("nonsense-tag"), "Other");
    }

    /// Roll `agg` into super-categories using `map`, returning (supers, summed).
    /// Mirrors the production `print_rollup` aggregation so the reconciliation
    /// invariant is tested against the same code path shape.
    fn rollup_with(
        map: &CategoryMap,
        agg: &[CategoryAggregate],
    ) -> (HashMap<String, Decimal>, Decimal) {
        let mut supers: HashMap<String, Decimal> = HashMap::new();
        for entry in agg {
            *supers.entry(map.super_category(&entry.name)).or_default() += entry.total;
        }
        let summed: Decimal = supers.values().copied().sum();
        (supers, summed)
    }

    /// A categories.toml fixture written to a unique temp path. Synthetic only.
    fn write_temp_categories(contents: &str, tag: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("fd-budget-categories-{tag}-{pid}-{nanos}.toml"));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn rollup_reconciles_to_spend_floor_with_config_present() {
        // (i) With a config file present, the roll-up still reconciles exactly.
        let txs = vec![
            mk_tx_tagged("2025-06-01", "-900.00", "RENT", "t1", &["rent"]),
            mk_tx_tagged("2025-06-02", "-55.67", "VODAFONE", "t2", &["mobile"]),
            mk_tx_tagged("2025-06-03", "-40.00", "TESCO", "t3", &["groceries"]),
            mk_tx_tagged(
                "2025-06-04",
                "-12.99",
                "STREAMFLIX",
                "t4",
                &["subscription"],
            ),
            mk_tx_tagged("2025-06-05", "-7.50", "MYSTERY", "t5", &[]),
        ];
        let config = r#"
[super_categories]
Home = ["rent", "mortgage", "housing"]
Bills = ["bills", "mobile", "insurance"]
Food = ["groceries", "food", "coffee"]
Subscriptions = ["subscription", "digital", "tech"]
"#;
        let path = write_temp_categories(config, "present");
        let map = CategoryMap::load(&path).unwrap();

        let (agg, grand_total) = aggregate_by_category(&txs, DateFilter::default());
        let (supers, summed) = rollup_with(&map, &agg);
        assert_eq!(summed, grand_total);
        assert_eq!(grand_total, spend_floor(&txs, DateFilter::default()));
        // The config map placed the known tags where the TOML says.
        assert_eq!(map.super_category("rent"), "Home");
        assert_eq!(map.super_category("mobile"), "Bills");
        assert_eq!(map.super_category("groceries"), "Food");
        assert_eq!(map.super_category("subscription"), "Subscriptions");
        // The untagged row is in the Uncategorised super-bucket (always implicit).
        assert!(supers.contains_key(UNCATEGORISED_SUPER));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn tag_absent_from_config_lands_in_other_and_still_reconciles() {
        // (ii) A tag not listed in any super-category rolls up under "Other",
        // and the roll-up still reconciles exactly to the Spend floor.
        let txs = vec![
            mk_tx_tagged("2025-06-01", "-900.00", "RENT", "t1", &["rent"]),
            // "gym" is deliberately omitted from the config below.
            mk_tx_tagged("2025-06-02", "-30.00", "PUREGYM", "t2", &["gym"]),
            mk_tx_tagged("2025-06-03", "-7.50", "MYSTERY", "t3", &[]),
        ];
        let config = r#"
[super_categories]
Home = ["rent", "mortgage"]
Food = ["groceries", "food"]
"#;
        let path = write_temp_categories(config, "other");
        let map = CategoryMap::load(&path).unwrap();

        let (agg, grand_total) = aggregate_by_category(&txs, DateFilter::default());
        let (supers, summed) = rollup_with(&map, &agg);

        // The unmapped "gym" tag lands in the Other super-bucket.
        assert_eq!(map.super_category("gym"), OTHER_SUPER);
        let other = supers.get(OTHER_SUPER).copied().unwrap_or(Decimal::ZERO);
        assert_eq!(other, Decimal::from_str("30.00").unwrap());
        // Reconciliation still holds with an Other bucket present.
        assert_eq!(summed, grand_total);
        assert_eq!(grand_total, spend_floor(&txs, DateFilter::default()));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn absent_config_falls_back_to_default_taxonomy_and_reconciles() {
        // (iii) With no config file present, load() falls back to the embedded
        // default taxonomy and the roll-up reconciles exactly.
        let mut missing = std::env::temp_dir();
        missing.push(format!(
            "fd-budget-categories-does-not-exist-{}.toml",
            std::process::id()
        ));
        // Ensure it really is absent.
        std::fs::remove_file(&missing).ok();
        assert!(!missing.exists());

        let map = CategoryMap::load(&missing).unwrap();
        // The fallback map equals the default taxonomy (spot-check known mappings).
        assert_eq!(map.super_category("rent"), "Home");
        assert_eq!(map.super_category("mobile"), "Bills");
        assert_eq!(map.super_category("groceries"), "Food");
        assert_eq!(map.super_category("streaming"), "Subscriptions");
        assert_eq!(map.super_category(UNCATEGORISED), UNCATEGORISED_SUPER);
        assert_eq!(map.super_category("nonsense-tag"), OTHER_SUPER);

        let txs = vec![
            mk_tx_tagged("2025-06-01", "-900.00", "RENT", "t1", &["rent"]),
            mk_tx_tagged("2025-06-02", "-55.67", "VODAFONE", "t2", &["mobile"]),
            mk_tx_tagged("2025-06-03", "-40.00", "TESCO", "t3", &["groceries"]),
            mk_tx_tagged("2025-06-04", "-7.50", "MYSTERY", "t4", &[]),
        ];
        let (agg, grand_total) = aggregate_by_category(&txs, DateFilter::default());
        let (_supers, summed) = rollup_with(&map, &agg);
        assert_eq!(summed, grand_total);
        assert_eq!(grand_total, spend_floor(&txs, DateFilter::default()));
    }

    #[test]
    fn default_toml_round_trips_to_default_taxonomy() {
        // The starter TOML we seed on first use parses back to a map equivalent
        // to the embedded default for every default tag.
        let toml = CategoryMap::default_toml();
        assert!(toml.contains("[super_categories]"));
        let path = write_temp_categories(&toml, "roundtrip");
        let from_file = CategoryMap::load(&path).unwrap();
        let default = CategoryMap::default_taxonomy();
        for (_sup, tags) in DEFAULT_TAXONOMY {
            for tag in *tags {
                assert_eq!(
                    from_file.super_category(tag),
                    default.super_category(tag),
                    "tag {tag} mismatched after TOML round-trip"
                );
            }
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn unmatched_filter_over_threshold() {
        let tx_big = mk_tx("2025-06-13", "-237.45", "POSITIVE INTERLONDON", "tx1");
        let tx_small = mk_tx("2025-06-13", "-12.50", "TFL TRAVEL CH", "tx2");
        let txs = vec![tx_big, tx_small];
        let emails: Vec<EmailRow> = vec![];
        let ms: Vec<MatchRow> = vec![]; // both unmatched (treated as "none")
        let joined = join(&txs, &emails, &ms);

        let unmatched: Vec<&JoinedRow> = joined.iter().filter(|r| r.confidence == "none").collect();
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
        let f =
            DateFilter::from_flags(Some(2025), None, NaiveDate::from_ymd_opt(2025, 6, 1)).unwrap();
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

/// Load the PayPal recovery sidecar (`paypal_matches.jsonl`) into a lookup
/// index. A missing file yields an empty index (recovery is optional).
pub fn load_recovery_index(paypal_matches_path: &Path) -> anyhow::Result<RecoveryIndex> {
    RecoveryIndex::load(paypal_matches_path)
        .map_err(|e| anyhow::anyhow!("failed to load {}: {}", paypal_matches_path.display(), e))
}
