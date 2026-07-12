//! Household lump-smoothing — size a sinking-fund buffer and a monthly standing
//! order that turn lumpy annual obligations (gym blocks, insurance renewals,
//! holidays, a flat service charge) into a smooth monthly drip.
//!
//! The idea: instead of letting big irregular bills batter the current account,
//! drip a fixed amount each month into a buffer account and pay the lumps out of
//! it. Two numbers fall out:
//!
//!   * **monthly drip** = (sum of all annual lump obligations) / 12 — the
//!     standing order into the buffer account.
//!   * **buffer to hold** = the peak-to-trough swing of the buffer balance over
//!     the year, so it never runs dry. This is driven by *timing*: clustered
//!     lumps need a bigger buffer than evenly-spaced ones.
//!
//! Read-only over `transactions.csv`. The only thing this module ever *writes*
//! is a seeded template config (`smoothing.toml`) on first run — never bank data.
//!
//! The config lists "lump categories". Each has a `tag` (debits carrying it are
//! the obligation) and an OPTIONAL `annual_budget` (a fixed yearly £ figure used
//! INSTEAD of summing actuals — for categories the bank data under-captures, e.g.
//! holidays paid partly on other cards). A fixed-budget category is spread evenly
//! across the 12 months; an actuals category is summed on each lump's real month.
//!
//! **Scope — this is a one-shot RETROSPECTIVE sizing calc.** It looks back over
//! a 12-month window, sizes a single drip + buffer figure, and prints them. It
//! does NOT maintain a forward per-pot running balance, nor does it track a live
//! pot or raise a "this pot won't cover its next due lump" shortfall flag — no
//! such forward ledger exists here. The only forward-looking hints are the
//! plausibility `notes` (e.g. window-drift double-counting, a tag with no
//! matches, thin data coverage), which caveat the retrospective figures rather
//! than track a balance going forward.

use crate::query::DateFilter;
use crate::Transaction;
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Number of months a smoothing window spans. The buffer walk always covers
/// exactly this many chronological buckets.
pub const WINDOW_MONTHS: usize = 12;

// ---------------------------------------------------------------------------
// Config (smoothing.toml)
// ---------------------------------------------------------------------------

/// One lump category in `smoothing.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Lump {
    /// Tag whose debits are this obligation (matched case-insensitively).
    pub tag: String,
    /// Optional fixed yearly £ figure. When set, it is used INSTEAD of summing
    /// actual debits, and is spread evenly across the 12 months for timing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annual_budget: Option<Decimal>,
}

/// The on-disk `smoothing.toml` schema: a list of `[[lump]]` tables.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SmoothingConfig {
    #[serde(default)]
    pub lump: Vec<Lump>,
}

impl SmoothingConfig {
    /// Load the config from `path`.
    ///
    /// An absent file is the caller's concern (see [`load_or_seed`]); here a
    /// missing file simply yields an empty set. A parse error IS surfaced — a
    /// malformed config is a user error worth reporting, not silently swallowing.
    pub fn load<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {}", path.display(), e))?;
        let config: SmoothingConfig = toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("failed to parse {}: {}", path.display(), e))?;
        // A fixed `annual_budget` must be strictly positive. A zero or negative
        // budget spreads a nonsense (zero/negative) monthly outflow and prints a
        // meaningless negative drip, so reject it at load, naming the tag.
        for lump in &config.lump {
            if let Some(budget) = lump.annual_budget {
                if budget <= Decimal::ZERO {
                    return Err(anyhow::anyhow!(
                        "lump '{}' has a non-positive annual_budget ({}) in {}; \
                         it must be greater than zero",
                        lump.tag,
                        budget,
                        path.display()
                    ));
                }
            }
        }
        Ok(config)
    }

    /// True when no lump categories are configured.
    pub fn is_empty(&self) -> bool {
        self.lump.is_empty()
    }
}

/// A GENERIC, mostly-commented starter `smoothing.toml`.
///
/// Deliberately carries NO real tags or figures — every example is commented
/// out, with an explanation of `tag` and `annual_budget`, so the seeded file is
/// a template the user fills in rather than a working config. Loading it yields
/// an empty set (every `[[lump]]` is a comment), which the command reports
/// plainly.
pub fn default_template_toml() -> String {
    "\
# fd-budget smoothing config — household lump-smoothing.
#
# List the lumpy ANNUAL obligations you want to smooth into a monthly standing
# order plus a buffer account. Each `[[lump]]` is one category.
#
#   tag           = the tag your debits carry for this obligation. Debits tagged
#                   with it (case-insensitive) are summed on their real month.
#   annual_budget = OPTIONAL. A fixed yearly £ figure used INSTEAD of summing
#                   actual debits — for categories your bank data under-captures
#                   (e.g. a holiday paid partly on another card). A fixed budget
#                   is spread EVENLY across the 12 months for buffer timing.
#
# Uncomment and edit the examples below, or add your own. Tag your transactions
# first with `fd-budget tag` / `fd-budget categorize`.

# [[lump]]
# tag = \"insurance\"          # sum actual debits tagged `insurance` over the window

# [[lump]]
# tag = \"gym\"                # an annual gym block, summed from actuals

# [[lump]]
# tag = \"holiday\"
# annual_budget = 6000         # use this fixed figure instead of actuals

# [[lump]]
# tag = \"service-charge\"     # a flat / property service charge
"
    .to_string()
}

/// Load the config, seeding a generic template on first run.
///
/// Behaviour:
///   * **Present** → load it (errors on a malformed file).
///   * **Absent, writable** → write the generic template, then load it (yields
///     an empty set, since every example is commented out) and flag that a
///     template was seeded so the caller can tell the user to fill it in.
///   * **Absent, NOT writable** → fall back to an empty set and flag that the
///     write failed, so the caller can tell the user to create the file by hand.
///
/// Returns `(config, status)`.
pub fn load_or_seed<P: AsRef<Path>>(path: P) -> anyhow::Result<(SmoothingConfig, SeedStatus)> {
    let path = path.as_ref();
    if path.exists() {
        return Ok((SmoothingConfig::load(path)?, SeedStatus::Existed));
    }
    match std::fs::write(path, default_template_toml()) {
        Ok(()) => Ok((SmoothingConfig::load(path)?, SeedStatus::Seeded)),
        Err(e) => Ok((
            SmoothingConfig::default(),
            SeedStatus::SeedFailed(e.to_string()),
        )),
    }
}

/// What [`load_or_seed`] found / did.
#[derive(Debug, Clone, PartialEq)]
pub enum SeedStatus {
    /// The config already existed and was loaded.
    Existed,
    /// No config existed; a generic template was written.
    Seeded,
    /// No config existed and the template could not be written (message).
    SeedFailed(String),
}

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

/// A 12-month chronological window, anchored on a start year-month.
///
/// Month 0 is `(start_year, start_month)`; month 11 is 11 months later. A date's
/// bucket index is the number of whole months between the window start and that
/// date. Dates outside `0..WINDOW_MONTHS` are not in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub start_year: i32,
    pub start_month: u32,
}

impl Window {
    /// A window whose FIRST month is `(year, month)`.
    pub fn starting(year: i32, month: u32) -> Self {
        Window {
            start_year: year,
            start_month: month,
        }
    }

    /// A window whose LAST month is `(year, month)` — i.e. the 12 months ending
    /// at and including that month. Used for "last 12 months" anchored on the
    /// latest data month.
    pub fn ending(year: i32, month: u32) -> Self {
        // Step back 11 months from the end.
        let total = year * 12 + (month as i32 - 1) - (WINDOW_MONTHS as i32 - 1);
        let sy = total.div_euclid(12);
        let sm = total.rem_euclid(12) as u32 + 1;
        Window {
            start_year: sy,
            start_month: sm,
        }
    }

    /// The bucket index (0..WINDOW_MONTHS) for `date`, or `None` if the date
    /// falls outside the window.
    pub fn bucket(&self, date: NaiveDate) -> Option<usize> {
        let start = self.start_year * 12 + (self.start_month as i32 - 1);
        let d = date.year() * 12 + (date.month() as i32 - 1);
        let idx = d - start;
        if (0..WINDOW_MONTHS as i32).contains(&idx) {
            Some(idx as usize)
        } else {
            None
        }
    }

    /// The (year, month) of bucket `i` (0..WINDOW_MONTHS).
    pub fn month_of(&self, i: usize) -> (i32, u32) {
        let total = self.start_year * 12 + (self.start_month as i32 - 1) + i as i32;
        (total.div_euclid(12), total.rem_euclid(12) as u32 + 1)
    }
}

/// Resolve the smoothing window from CLI date flags and the data.
///
/// Priority:
///   * `--month YYYY-MM` → a 12-month window ENDING at that month (the month is
///     the most-recent in the window).
///   * `--year YYYY` → the 12 months Jan..Dec of that year (window starting Jan).
///   * `--since` only / no flags → the 12 months ending at the latest transaction
///     month (or, if there are no transactions, the current month).
///
/// `today` is injected for determinism in tests.
pub fn resolve_window(
    filter: &DateFilter,
    year: Option<i32>,
    month: Option<&str>,
    transactions: &[Transaction],
    today: NaiveDate,
) -> Window {
    if let Some(m) = month {
        // DateFilter has already validated the YYYY-MM shape; re-parse the parts.
        if let Some((y, mo)) = parse_year_month(m) {
            return Window::ending(y, mo);
        }
    }
    if let Some(y) = year {
        return Window::starting(y, 1);
    }
    // Anchor on the latest transaction within the filter (so `--since` narrows
    // the anchor), else the latest transaction overall, else today.
    let anchor = transactions
        .iter()
        .filter(|t| filter.matches(t.date))
        .map(|t| t.date)
        .max()
        .or_else(|| transactions.iter().map(|t| t.date).max())
        .unwrap_or(today);
    Window::ending(anchor.year(), anchor.month())
}

fn parse_year_month(s: &str) -> Option<(i32, u32)> {
    let (y, m) = s.split_once('-')?;
    Some((y.parse().ok()?, m.parse().ok()?))
}

// ---------------------------------------------------------------------------
// Compute
// ---------------------------------------------------------------------------

/// How a category's annual total was derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// Summed from actual tagged debits in the window (with the payment count).
    Actuals(usize),
    /// A fixed `annual_budget`, spread evenly across the 12 months.
    FixedBudget,
}

/// One matched debit row contributing to a category (for `--detail`).
#[derive(Debug, Clone)]
pub struct MatchedRow {
    pub date: NaiveDate,
    pub amount: Decimal, // The obligation magnitude (positive).
    pub tag: String,
}

/// A computed per-category line of the report.
#[derive(Debug, Clone)]
pub struct CategoryResult {
    pub tag: String,
    pub annual_total: Decimal,
    pub basis: Basis,
    /// The 12 monthly outflows this category contributes (index 0..12).
    pub monthly: [Decimal; WINDOW_MONTHS],
    /// The individual matched actual rows (empty for a fixed-budget category).
    pub rows: Vec<MatchedRow>,
}

/// The full smoothing computation over a window.
#[derive(Debug, Clone)]
pub struct Smoothing {
    pub window: Window,
    pub categories: Vec<CategoryResult>,
    /// Σ of every category's annual total.
    pub annual_total: Decimal,
    /// `annual_total / 12` — the monthly standing order.
    pub drip: Decimal,
    /// Total outflow in each of the 12 months (all categories).
    pub monthly_outflow: [Decimal; WINDOW_MONTHS],
    /// Buffer float at the END of each month (after that month's drip and
    /// outflow): `cum[i] = cum[i-1] - outflow[i] + drip`.
    pub cum: [Decimal; WINDOW_MONTHS],
    /// Peak-to-trough float to hold, sized PESSIMISTICALLY: the highest
    /// end-of-month peak minus the worst-case intra-month (pre-drip) trough,
    /// i.e. assuming a lump can pay before the drip arrives. One drip larger
    /// than the naive `max(cum) - min(cum)` swing.
    pub buffer: Decimal,
    /// Plausibility notes surfaced during compute — e.g. an annual bill caught
    /// twice by window drift (counted once for the drip), or a configured tag
    /// with no payments in the window (an obligation invisible here).
    pub notes: Vec<String>,
}

/// Compute the smoothing for a window over the configured lump categories.
///
/// For each category:
///   * `annual_budget` set → use it, spread evenly (budget/12 in every month).
///   * else → sum the actual debit rows carrying that tag, each on its real month.
///
/// Then:
///   * `annual_total` = Σ over categories (each category's budget-or-actuals total).
///   * `drip` = annual_total / 12.
///   * walk the 12 months in order with PESSIMISTIC intra-month ordering: debit
///     the outflow first (recording the pre-drip low), then add the drip and
///     record the end-of-month `cum`. `buffer = max(cum) - min(pre_drip_low)` —
///     one drip larger than the naive swing, so it covers the worst-case
///     ordering where a bill pays before the drip lands.
pub fn compute(
    config: &SmoothingConfig,
    transactions: &[Transaction],
    window: Window,
) -> Smoothing {
    let twelve = Decimal::from(WINDOW_MONTHS as u64);
    let mut categories = Vec::with_capacity(config.lump.len());
    let mut annual_total = Decimal::ZERO;
    let mut notes: Vec<String> = Vec::new();

    for lump in &config.lump {
        let mut monthly = [Decimal::ZERO; WINDOW_MONTHS];
        let mut rows = Vec::new();

        let (total, basis) = if let Some(budget) = lump.annual_budget {
            // Fixed budget: spread evenly across all 12 months.
            let slice = budget / twelve;
            for m in monthly.iter_mut() {
                *m = slice;
            }
            (budget, Basis::FixedBudget)
        } else {
            // Actuals: collect the debit rows carrying this tag within the window.
            let mut matched: Vec<(usize, NaiveDate, Decimal)> = Vec::new();
            for tx in transactions {
                if !tx.is_debit() {
                    continue;
                }
                if !tx.tags.iter().any(|t| t.eq_ignore_ascii_case(&lump.tag)) {
                    continue;
                }
                let Some(idx) = window.bucket(tx.date) else {
                    continue;
                };
                matched.push((idx, tx.date, tx.amount.abs()));
            }

            // Window-drift guard: an ANNUAL bill can land TWICE in a rolling
            // 12-month window when its renewal date drifts across the edge (last
            // year's + this year's both caught), doubling the drip and buffer.
            // If exactly two similar-amount matches sit ~a year apart, keep only
            // the most recent — the steady-state obligation is one per year.
            if is_annual_drift_double(&matched) {
                matched.sort_by_key(|(_, d, _)| *d);
                let kept = matched.pop().unwrap();
                notes.push(format!(
                    "{}: an annual bill appears twice in the 12-month window \
                     (~a year apart) — counted once for the drip.",
                    lump.tag
                ));
                matched = vec![kept];
            } else if matched.is_empty() {
                notes.push(format!(
                    "{}: no payments matched in the 12-month window — an annual \
                     obligation due outside it would be invisible here.",
                    lump.tag
                ));
            }

            let mut sum = Decimal::ZERO;
            for (idx, date, mag) in &matched {
                monthly[*idx] += *mag;
                sum += *mag;
                rows.push(MatchedRow {
                    date: *date,
                    amount: *mag,
                    tag: lump.tag.clone(),
                });
            }
            (sum, Basis::Actuals(matched.len()))
        };

        annual_total += total;
        categories.push(CategoryResult {
            tag: lump.tag.clone(),
            annual_total: total,
            basis,
            monthly,
            rows,
        });
    }

    // M3: short-history coverage caveat. Actuals-based drips sum tagged debits
    // within the window; a month with NO data in the store is silently treated
    // as £0 outflow, so if the transaction store does not SPAN the full window
    // the drip can be understated. Emit a note (still compute) so a partial
    // figure is not read as complete. Fixed-budget-only configs don't depend on
    // actuals coverage, so the note is skipped when no actuals category exists.
    let has_actuals = config.lump.iter().any(|l| l.annual_budget.is_none());
    if has_actuals {
        if let (Some(first), Some(last)) = (
            transactions.iter().map(|t| t.date).min(),
            transactions.iter().map(|t| t.date).max(),
        ) {
            let (ws_y, ws_m) = window.month_of(0);
            let (we_y, we_m) = window.month_of(WINDOW_MONTHS - 1);
            let key = |y: i32, m: u32| y * 12 + m as i32;
            let short_start = key(first.year(), first.month()) > key(ws_y, ws_m);
            let short_end = key(last.year(), last.month()) < key(we_y, we_m);
            if short_start || short_end {
                notes.push(format!(
                    "transaction data spans {:04}-{:02}..{:04}-{:02}, short of the \
                     {:04}-{:02}..{:04}-{:02} window — actuals-based drips may be \
                     understated for lack of coverage.",
                    first.year(),
                    first.month(),
                    last.year(),
                    last.month(),
                    ws_y,
                    ws_m,
                    we_y,
                    we_m,
                ));
            }
        }
    }

    // Per-month total outflow across every category.
    let mut monthly_outflow = [Decimal::ZERO; WINDOW_MONTHS];
    for cat in &categories {
        for (i, m) in cat.monthly.iter().enumerate() {
            monthly_outflow[i] += *m;
        }
    }

    let drip = annual_total / twelve;

    // Walk the 12 months; record the cumulative buffer float each month.
    //
    // Ordering is PESSIMISTIC within a month: we cannot assume the standing
    // order (drip) lands before the bill each month. If the bill pays first,
    // the true intra-month trough is one drip lower than the end-of-month
    // float. So each month we DEBIT the outflow first — recording that
    // pre-drip low as the worst case the buffer must cover — and only THEN
    // credit the drip. `cum[i]` stays the end-of-month float (for the detail
    // view); `pre_drip[i]` is the worst-case low used to size the buffer.
    let mut cum = [Decimal::ZERO; WINDOW_MONTHS];
    let mut pre_drip = [Decimal::ZERO; WINDOW_MONTHS];
    let mut running = Decimal::ZERO;
    for i in 0..WINDOW_MONTHS {
        running -= monthly_outflow[i]; // bill pays first (worst-case ordering)
        pre_drip[i] = running; // worst-case intra-month low
        running += drip; // then the drip arrives
        cum[i] = running; // end-of-month float
    }

    // Buffer = highest end-of-month peak to the worst-case (pre-drip) trough.
    // Using the pre-drip lows makes the buffer robust to the bill landing
    // before the drip in any month (one drip larger than the naive swing).
    let max = cum.iter().copied().max().unwrap_or(Decimal::ZERO);
    let min = pre_drip.iter().copied().min().unwrap_or(Decimal::ZERO);
    let buffer = max - min;

    Smoothing {
        window,
        categories,
        annual_total,
        drip,
        monthly_outflow,
        cum,
        buffer,
        notes,
    }
}

/// True when a tag's matched actuals look like ONE annual bill caught TWICE by
/// window drift: exactly two payments, ~a year apart, of a similar amount. Such
/// a pair should size the drip as one occurrence, not two.
fn is_annual_drift_double(matched: &[(usize, chrono::NaiveDate, Decimal)]) -> bool {
    use rust_decimal::prelude::ToPrimitive;
    if matched.len() != 2 {
        return false;
    }
    let gap_days = (matched[1].1 - matched[0].1).num_days().abs();
    if gap_days < 330 {
        return false; // not ~annual apart (e.g. a genuine semi-annual bill)
    }
    let a = matched[0].2.to_f64().unwrap_or(0.0).abs();
    let b = matched[1].2.to_f64().unwrap_or(0.0).abs();
    let base = a.min(b).max(f64::MIN_POSITIVE);
    (a - b).abs() / base <= 0.25 // similar amount => same recurring bill
}

impl Smoothing {
    /// The buffer balance to hold each month so the trough sits at exactly £0.
    ///
    /// The raw `cum` series is the steady-state float relative to an arbitrary
    /// zero; shifting it up by `-min(cum)` makes the lowest point £0 and every
    /// other month the float held above empty. Handy for the `--detail` view.
    pub fn held_balance(&self) -> [Decimal; WINDOW_MONTHS] {
        let min = self.cum.iter().copied().min().unwrap_or(Decimal::ZERO);
        let mut held = [Decimal::ZERO; WINDOW_MONTHS];
        for (i, c) in self.cum.iter().enumerate() {
            held[i] = *c - min;
        }
        held
    }

    /// The monthly standing-order figure to actually pay into the buffer: the
    /// raw `drip` rounded UP (away from zero) to whole pence.
    ///
    /// The raw `drip` (`annual_total / 12`) can carry sub-penny fractions. A user
    /// who pays the *printed* £X.XX standing order would then diverge from the
    /// model by up to £0.005/month — a shortfall that compounds over years and
    /// leaves the buffer perpetually a touch short. Rounding the presented figure
    /// UP guarantees the standing order is always a hair's surplus, never a
    /// shortfall: `standing_order() * 12 >= annual_total`.
    pub fn standing_order(&self) -> Decimal {
        self.drip
            .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::AwayFromZero)
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render the smoothing report to a string.
///
/// `period_label` is a human window description (e.g. "last 12 months"). With
/// `detail`, also prints the month-by-month float trajectory and the matched
/// rows so the user can sanity-check what is counted.
pub fn render(s: &Smoothing, period_label: &str, detail: bool) -> String {
    use crate::query::format_money;
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "Household lump-smoothing ({})", period_label);

    if s.categories.is_empty() {
        let _ = writeln!(out, "  (no lump categories configured — nothing to smooth)");
        return out;
    }

    let mut any_fixed = false;
    for cat in &s.categories {
        let basis = match cat.basis {
            Basis::Actuals(n) => {
                let plural = if n == 1 { "payment" } else { "payments" };
                format!("(actuals, {} {})", n, plural)
            }
            Basis::FixedBudget => {
                any_fixed = true;
                "(fixed budget)".to_string()
            }
        };
        let _ = writeln!(
            out,
            "  {:<16} {:>10}   {}",
            truncate(&cat.tag, 16),
            format_money(cat.annual_total),
            basis
        );
    }

    let _ = writeln!(out, "  {}", "-".repeat(43));
    let _ = writeln!(
        out,
        "  {:<16} {:>10}",
        "Annual total",
        format_money(s.annual_total)
    );
    let _ = writeln!(
        out,
        "  {:<16} {:>10}   <- standing order into the buffer account",
        "Monthly drip",
        format_money(s.standing_order())
    );
    let _ = writeln!(
        out,
        "  {:<16} {:>10}   <- keep about this much in it",
        "Buffer to hold",
        format_money(s.buffer)
    );

    if any_fixed {
        let _ = writeln!(
            out,
            "  note: fixed-budget categories are assumed spread evenly across the 12 months."
        );
    }

    for note in &s.notes {
        let _ = writeln!(out, "  \u{26a0} {}", note);
    }

    if detail {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  Month-by-month buffer float (balance held; trough = £0):"
        );
        let held = s.held_balance();
        for (i, held_i) in held.iter().enumerate() {
            let (y, m) = s.window.month_of(i);
            let _ = writeln!(
                out,
                "    {:04}-{:02}   out {:>10}   held {:>10}",
                y,
                m,
                format_money(s.monthly_outflow[i]),
                format_money(*held_i)
            );
        }

        // Matched actual rows, so the user can check exactly what is counted.
        let mut printed_header = false;
        for cat in &s.categories {
            if cat.rows.is_empty() {
                continue;
            }
            if !printed_header {
                let _ = writeln!(out);
                let _ = writeln!(out, "  Matched rows (actuals):");
                printed_header = true;
            }
            for r in &cat.rows {
                let _ = writeln!(
                    out,
                    "    {}   {:>10}   {}",
                    r.date,
                    format_money(r.amount),
                    r.tag
                );
            }
        }
    }

    out
}

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_string()
    } else {
        s.chars().take(width).collect()
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

    /// A debit tagged `tag`, dated `(y, m, d)`.
    fn debit(y: i32, m: u32, d: u32, amount: &str, tag: &str) -> Transaction {
        Transaction {
            date: NaiveDate::from_ymd_opt(y, m, d).unwrap(),
            account: Account::Current,
            tx_type: TxType::Contactless,
            amount: Decimal::from_str(amount).unwrap(),
            description: "x".into(),
            raw_description: "x".into(),
            balance: None,
            tags: vec![tag.to_string()],
            import_id: "id".into(),
        }
    }

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    // -- Window mechanics ---------------------------------------------------

    #[test]
    fn window_buckets_dates_into_12_months() {
        let w = Window::starting(2025, 1);
        assert_eq!(
            w.bucket(NaiveDate::from_ymd_opt(2025, 1, 15).unwrap()),
            Some(0)
        );
        assert_eq!(
            w.bucket(NaiveDate::from_ymd_opt(2025, 7, 3).unwrap()),
            Some(6)
        );
        assert_eq!(
            w.bucket(NaiveDate::from_ymd_opt(2025, 12, 31).unwrap()),
            Some(11)
        );
        // Out of window.
        assert_eq!(
            w.bucket(NaiveDate::from_ymd_opt(2024, 12, 31).unwrap()),
            None
        );
        assert_eq!(w.bucket(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()), None);
    }

    #[test]
    fn window_ending_steps_back_eleven_months() {
        // 12 months ending Dec 2025 starts Jan 2025.
        assert_eq!(Window::ending(2025, 12), Window::starting(2025, 1));
        // Crossing a year boundary: ending May 2026 starts June 2025.
        assert_eq!(Window::ending(2026, 5), Window::starting(2025, 6));
        let w = Window::ending(2026, 5);
        assert_eq!(w.month_of(0), (2025, 6));
        assert_eq!(w.month_of(11), (2026, 5));
    }

    // -- GOLDEN TEST (pins the algorithm) -----------------------------------

    #[test]
    fn golden_single_1200_lump_in_month_7() {
        // One actuals lump of £1200 in the 7th month of the window (index 6),
        // no fixed budgets. Window: Jan..Dec 2025, lump dated July 2025.
        let config = SmoothingConfig {
            lump: vec![Lump {
                tag: "insurance".into(),
                annual_budget: None,
            }],
        };
        let txs = vec![debit(2025, 7, 15, "-1200.00", "insurance")];
        let w = Window::starting(2025, 1);
        let s = compute(&config, &txs, w);

        // drip = 1200 / 12 = 100.00
        assert_eq!(s.drip, dec("100.00"));
        assert_eq!(s.annual_total, dec("1200.00"));

        // The lump lands in bucket index 6.
        assert_eq!(s.monthly_outflow[6], dec("1200.00"));

        // Cumulative series: 100,200,300,400,500,600,-500,-400,-300,-200,-100,0
        let expected = [
            "100", "200", "300", "400", "500", "600", "-500", "-400", "-300", "-200", "-100", "0",
        ];
        for (i, e) in expected.iter().enumerate() {
            assert_eq!(s.cum[i], dec(e), "cum[{}] mismatch", i);
        }

        // Pessimistic buffer (M1): the peak end-of-month float is 600; the
        // worst-case intra-month trough is month 6's PRE-drip low of
        // 600 - 1200 = -600 (the £1200 bill can pay before the £100 drip
        // arrives). buffer = 600 - (-600) = 1200.00 — one drip (£100) more than
        // the old naive max(cum) - min(cum) = 1100, which optimistically
        // assumed the drip always landed first.
        assert_eq!(s.buffer, dec("1200.00"));
    }

    #[test]
    fn buffer_is_pessimistic_one_drip_above_naive_swing() {
        // M1: for any single-lump case the pessimistic buffer is exactly one
        // drip larger than the naive end-of-month peak-to-trough swing, because
        // the worst-case trough sits one drip below the end-of-month low.
        let config = SmoothingConfig {
            lump: vec![Lump {
                tag: "insurance".into(),
                annual_budget: None,
            }],
        };
        let txs = vec![debit(2025, 7, 15, "-1200.00", "insurance")];
        let s = compute(&config, &txs, Window::starting(2025, 1));

        let naive_max = s.cum.iter().copied().max().unwrap();
        let naive_min = s.cum.iter().copied().min().unwrap();
        let naive_swing = naive_max - naive_min; // old buffer definition
        assert_eq!(s.buffer, naive_swing + s.drip);
    }

    // -- Window-drift guard (H10) -------------------------------------------

    #[test]
    fn annual_bill_caught_twice_by_drift_counts_once() {
        // An annual insurance bill whose renewal drifted: last year's (Jan) and
        // this year's (Dec) both land in the 12-month window. It must size the
        // drip as ONE occurrence, not the doubled sum, and say so.
        let config = SmoothingConfig {
            lump: vec![Lump {
                tag: "insurance".into(),
                annual_budget: None,
            }],
        };
        let txs = vec![
            debit(2025, 1, 15, "-1200.00", "insurance"),
            debit(2025, 12, 20, "-1210.00", "insurance"),
        ];
        let s = compute(&config, &txs, Window::starting(2025, 1));
        assert_eq!(s.annual_total, dec("1210.00"), "sum must not double the annual bill");
        assert!(matches!(s.categories[0].basis, Basis::Actuals(1)));
        assert!(
            s.notes.iter().any(|n| n.contains("twice")),
            "expected a drift note, got: {:?}",
            s.notes
        );
    }

    #[test]
    fn two_similar_charges_only_six_months_apart_are_not_deduped() {
        // A genuine semi-annual bill (two similar charges ~6 months apart) is NOT
        // window drift — both count.
        let config = SmoothingConfig {
            lump: vec![Lump {
                tag: "insurance".into(),
                annual_budget: None,
            }],
        };
        let txs = vec![
            debit(2025, 1, 15, "-600.00", "insurance"),
            debit(2025, 7, 15, "-600.00", "insurance"),
        ];
        let s = compute(&config, &txs, Window::starting(2025, 1));
        assert_eq!(s.annual_total, dec("1200.00"));
        assert!(matches!(s.categories[0].basis, Basis::Actuals(2)));
    }

    #[test]
    fn zero_match_actuals_tag_is_noted() {
        // A configured actuals tag with no payments in the window: its obligation
        // is invisible here, so the report must flag it rather than show a silent
        // £0.
        let config = SmoothingConfig {
            lump: vec![Lump {
                tag: "insurance".into(),
                annual_budget: None,
            }],
        };
        let txs = vec![debit(2025, 3, 1, "-50.00", "groceries")];
        let s = compute(&config, &txs, Window::starting(2025, 1));
        assert_eq!(s.annual_total, Decimal::ZERO);
        assert!(
            s.notes.iter().any(|n| n.contains("no payments matched")),
            "expected a no-match note, got: {:?}",
            s.notes
        );
    }

    #[test]
    fn short_history_store_emits_coverage_note() {
        // M3: the window is the full 12 months of 2025, but the store only holds
        // data for July 2025. Actuals-based drips can be understated because the
        // uncovered months are silently £0, so a coverage note must be emitted
        // (while still computing).
        let config = SmoothingConfig {
            lump: vec![Lump {
                tag: "insurance".into(),
                annual_budget: None,
            }],
        };
        let txs = vec![debit(2025, 7, 15, "-1200.00", "insurance")];
        let s = compute(&config, &txs, Window::starting(2025, 1));
        assert_eq!(s.annual_total, dec("1200.00")); // still computes
        assert!(
            s.notes.iter().any(|n| n.contains("lack of coverage")),
            "expected a short-history coverage note, got: {:?}",
            s.notes
        );
    }

    #[test]
    fn full_coverage_store_emits_no_coverage_note() {
        // A store spanning the whole window must NOT get a coverage note.
        let config = SmoothingConfig {
            lump: vec![Lump {
                tag: "insurance".into(),
                annual_budget: None,
            }],
        };
        let txs = vec![
            debit(2025, 1, 5, "-10.00", "misc"),
            debit(2025, 7, 15, "-1200.00", "insurance"),
            debit(2025, 12, 20, "-10.00", "misc"),
        ];
        let s = compute(&config, &txs, Window::starting(2025, 1));
        assert!(
            !s.notes.iter().any(|n| n.contains("lack of coverage")),
            "unexpected coverage note, got: {:?}",
            s.notes
        );
    }

    #[test]
    fn fixed_budget_only_config_skips_coverage_note() {
        // Fixed-budget-only configs don't depend on actuals coverage, so no
        // coverage note even with a short/empty store.
        let config = SmoothingConfig {
            lump: vec![Lump {
                tag: "holiday".into(),
                annual_budget: Some(dec("6000")),
            }],
        };
        let txs = vec![debit(2025, 7, 15, "-50.00", "groceries")];
        let s = compute(&config, &txs, Window::starting(2025, 1));
        assert!(
            !s.notes.iter().any(|n| n.contains("lack of coverage")),
            "unexpected coverage note for fixed-budget-only config, got: {:?}",
            s.notes
        );
    }

    // -- Fixed budget spread evenly -----------------------------------------

    #[test]
    fn fixed_budget_is_spread_evenly() {
        // A fixed £6000 holiday budget, no actuals. Spread evenly = £500/month,
        // which exactly equals the drip, so the end-of-month float never moves.
        // The pessimistic buffer is one drip (£500) — the worst-case intra-month
        // gap if the slice pays before the drip lands.
        let config = SmoothingConfig {
            lump: vec![Lump {
                tag: "holiday".into(),
                annual_budget: Some(dec("6000")),
            }],
        };
        let txs: Vec<Transaction> = vec![];
        let w = Window::starting(2025, 1);
        let s = compute(&config, &txs, w);

        assert_eq!(s.annual_total, dec("6000"));
        assert_eq!(s.drip, dec("500"));
        assert!(matches!(s.categories[0].basis, Basis::FixedBudget));
        // Every month's outflow is the evenly-spread slice.
        for m in &s.monthly_outflow {
            assert_eq!(*m, dec("500"));
        }
        // End-of-month cum is flat at £0 (drip == outflow each month).
        for c in &s.cum {
            assert_eq!(*c, Decimal::ZERO);
        }
        // But pessimistically (M1) the £500 slice can pay before the £500 drip
        // arrives, so each month's worst-case pre-drip trough is -£500. The
        // buffer covers that: 0 - (-500) = £500 = one drip.
        assert_eq!(s.buffer, dec("500"));
    }

    #[test]
    fn fixed_budget_combined_with_actuals_lump() {
        // £6000 holiday spread evenly (£500/mo) PLUS a £1200 insurance lump in
        // month index 6. annual_total = 7200, drip = 600.
        let config = SmoothingConfig {
            lump: vec![
                Lump {
                    tag: "holiday".into(),
                    annual_budget: Some(dec("6000")),
                },
                Lump {
                    tag: "insurance".into(),
                    annual_budget: None,
                },
            ],
        };
        let txs = vec![debit(2025, 7, 1, "-1200.00", "insurance")];
        let w = Window::starting(2025, 1);
        let s = compute(&config, &txs, w);

        assert_eq!(s.annual_total, dec("7200"));
        assert_eq!(s.drip, dec("600"));
        // Outflow: 500 every month, +1200 in month 6.
        for (i, m) in s.monthly_outflow.iter().enumerate() {
            if i == 6 {
                assert_eq!(*m, dec("1700"));
            } else {
                assert_eq!(*m, dec("500"));
            }
        }
        // End-of-month cum: (600-500)=100 each month, then month 6 dips by 1100.
        // 100,200,300,400,500,600,-500,-400,-300,-200,-100,0 -> same shape as golden.
        // Pessimistic buffer (M1): peak 600, worst-case pre-drip trough at month
        // 6 = 600 - 1700 = -1100, so buffer = 600 - (-1100) = 1700 — one drip
        // (£600) above the old naive 1100.
        assert_eq!(s.buffer, dec("1700"));
    }

    // -- Configured tag matching no rows ------------------------------------

    #[test]
    fn configured_tag_with_no_rows_contributes_zero() {
        // A tag that matches nothing must contribute £0 with no panic.
        let config = SmoothingConfig {
            lump: vec![Lump {
                tag: "nonexistent".into(),
                annual_budget: None,
            }],
        };
        let txs = vec![debit(2025, 7, 15, "-50.00", "groceries")];
        let w = Window::starting(2025, 1);
        let s = compute(&config, &txs, w);

        assert_eq!(s.annual_total, Decimal::ZERO);
        assert_eq!(s.drip, Decimal::ZERO);
        assert_eq!(s.buffer, Decimal::ZERO);
        assert_eq!(s.categories.len(), 1);
        assert_eq!(s.categories[0].annual_total, Decimal::ZERO);
        assert!(matches!(s.categories[0].basis, Basis::Actuals(0)));
        assert!(s.categories[0].rows.is_empty());
    }

    // -- Extra coverage -----------------------------------------------------

    #[test]
    fn actuals_only_sum_and_credits_ignored() {
        // Two insurance debits in different months sum into the annual total;
        // a credit carrying the tag (a refund) is ignored, and a debit OUTSIDE
        // the window is ignored.
        let config = SmoothingConfig {
            lump: vec![Lump {
                tag: "insurance".into(),
                annual_budget: None,
            }],
        };
        let txs = vec![
            debit(2025, 3, 1, "-1500.00", "insurance"),
            debit(2025, 9, 1, "-2008.00", "insurance"),
            // credit (refund) — ignored
            debit(2025, 6, 1, "25.00", "insurance"),
            // out of window — ignored
            debit(2024, 12, 1, "-999.00", "insurance"),
        ];
        let w = Window::starting(2025, 1);
        let s = compute(&config, &txs, w);
        assert_eq!(s.annual_total, dec("3508.00"));
        assert!(matches!(s.categories[0].basis, Basis::Actuals(2)));
        assert_eq!(s.monthly_outflow[2], dec("1500.00")); // March = idx 2
        assert_eq!(s.monthly_outflow[8], dec("2008.00")); // Sept = idx 8
    }

    #[test]
    fn tag_match_is_case_insensitive() {
        let config = SmoothingConfig {
            lump: vec![Lump {
                tag: "Insurance".into(),
                annual_budget: None,
            }],
        };
        let txs = vec![debit(2025, 7, 1, "-1200.00", "insurance")];
        let s = compute(&config, &txs, Window::starting(2025, 1));
        assert_eq!(s.annual_total, dec("1200.00"));
    }

    #[test]
    fn held_balance_floors_trough_at_zero() {
        // Reuse the golden scenario: min(cum) = -500, so held = cum + 500.
        let config = SmoothingConfig {
            lump: vec![Lump {
                tag: "insurance".into(),
                annual_budget: None,
            }],
        };
        let txs = vec![debit(2025, 7, 15, "-1200.00", "insurance")];
        let s = compute(&config, &txs, Window::starting(2025, 1));
        let held = s.held_balance();
        // First month: cum 100 + 500 = 600; trough month 6: -500 + 500 = 0.
        // held_balance floors by the END-of-month min (-500), a per-month
        // display, so its peak is 1100.
        assert_eq!(held[0], dec("600"));
        assert_eq!(held[6], Decimal::ZERO);
        assert_eq!(held[5], dec("1100")); // month 5 cum=600 -> held=1100
        // The buffer to hold (1200) is one drip MORE than the end-of-month held
        // peak, because it also covers the worst-case intra-month pre-drip
        // trough (M1) that the per-month display does not surface.
        assert_eq!(s.buffer, held[5] + s.drip);
    }

    // -- Config load / seed --------------------------------------------------

    #[test]
    fn template_loads_as_empty_set() {
        // The seeded template is fully commented, so it parses to no lumps.
        let toml = default_template_toml();
        let config: SmoothingConfig = toml::from_str(&toml).unwrap();
        assert!(config.is_empty());
    }

    #[test]
    fn config_roundtrips_lumps() {
        let toml = "\
[[lump]]
tag = \"insurance\"

[[lump]]
tag = \"holiday\"
annual_budget = 6000
";
        let config: SmoothingConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.lump.len(), 2);
        assert_eq!(config.lump[0].tag, "insurance");
        assert_eq!(config.lump[0].annual_budget, None);
        assert_eq!(config.lump[1].tag, "holiday");
        assert_eq!(config.lump[1].annual_budget, Some(dec("6000")));
    }

    #[test]
    fn resolve_window_prefers_month_then_year_then_data() {
        let txs = vec![debit(2025, 8, 1, "-10.00", "x")];
        let filter = DateFilter::default();
        let today = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();

        // --month wins: 12 months ending that month.
        let w = resolve_window(&filter, Some(2020), Some("2026-05"), &txs, today);
        assert_eq!(w, Window::ending(2026, 5));

        // --year: Jan..Dec that year.
        let w = resolve_window(&filter, Some(2024), None, &txs, today);
        assert_eq!(w, Window::starting(2024, 1));

        // No flags: anchor on the latest transaction month (Aug 2025).
        let w = resolve_window(&filter, None, None, &txs, today);
        assert_eq!(w, Window::ending(2025, 8));

        // No flags, no data: anchor on today.
        let w = resolve_window(&filter, None, None, &[], today);
        assert_eq!(w, Window::ending(2026, 1));
    }

    // -- L4: presented standing order is rounded UP, never a shortfall -------

    #[test]
    fn standing_order_times_twelve_covers_annual_total() {
        // annual_total = 1000 -> raw drip = 83.3333... The presented standing
        // order must round UP to 83.34 so that 12 payments (£1000.08) never
        // fall short of the £1000 annual obligation.
        let config = SmoothingConfig {
            lump: vec![Lump {
                tag: "holiday".into(),
                annual_budget: Some(dec("1000")),
            }],
        };
        let s = compute(&config, &[], Window::starting(2025, 1));
        let so = s.standing_order();
        assert_eq!(so, dec("83.34"), "standing order rounds up to whole pence");
        assert!(
            so * Decimal::from(WINDOW_MONTHS as u64) >= s.annual_total,
            "printed drip * 12 ({}) must cover the annual total ({})",
            so * Decimal::from(WINDOW_MONTHS as u64),
            s.annual_total
        );
        // The render surfaces the rounded figure, not the raw drip.
        let text = render(&s, "test", false);
        assert!(text.contains("83.34"), "rendered: {text}");
    }

    // -- L6: a non-positive annual_budget is rejected at load ----------------

    #[test]
    fn negative_annual_budget_is_rejected_at_load() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);

        // Write `content` to a unique temp file, run `load`, then clean up.
        fn load_toml(content: &str) -> anyhow::Result<SmoothingConfig> {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "fd-budget-smoothing-test-{}-{}.toml",
                std::process::id(),
                n
            ));
            std::fs::write(&path, content).unwrap();
            let result = SmoothingConfig::load(&path);
            let _ = std::fs::remove_file(&path);
            result
        }

        let err = load_toml("[[lump]]\ntag = \"holiday\"\nannual_budget = -6000")
            .expect_err("negative annual_budget must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("holiday"), "error must name the tag: {msg}");
        assert!(
            msg.contains("non-positive") || msg.contains("greater than zero"),
            "error must explain the constraint: {msg}"
        );

        // Zero is likewise rejected.
        let err = load_toml("[[lump]]\ntag = \"gym\"\nannual_budget = 0")
            .expect_err("zero annual_budget must be rejected");
        assert!(err.to_string().contains("gym"));

        // A positive budget still loads fine.
        let cfg = load_toml("[[lump]]\ntag = \"holiday\"\nannual_budget = 6000").unwrap();
        assert_eq!(cfg.lump[0].annual_budget, Some(dec("6000")));
    }
}
