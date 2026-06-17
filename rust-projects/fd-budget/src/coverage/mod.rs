//! Data-coverage / gap detector.
//!
//! The personal Spend floor silently UNDER-counts when a data source doesn't
//! cover the full period it should. Two real cases motivated this module:
//!
//!   * the **Visa** export itemised only ~7 of 12 months (≈£2k of card spend
//!     simply absent from `transactions.csv`), and
//!   * a **PayPal** export covered only 6 of ~19 months of bank `PAYPAL PAYMENT`
//!     rows, so `paypal recover` could only ever reach 22.4% — nothing FLAGGED
//!     that the rest was unrecoverable for lack of export coverage.
//!
//! Nothing in the tool made either gap visible. This module reports, per data
//! source, the date span and month-coverage, lists the missing months, and —
//! the killer metric — counts how many bank `PAYPAL PAYMENT` rows fall WITHIN
//! the PayPal export's span (i.e. are even recoverable given current coverage).
//! That last number is what explains a low `paypal recover` %.
//!
//! Sources reported:
//!   * bank `current` rows (from `transactions.csv`, `Account::Current`)
//!   * bank `visa`    rows (from `transactions.csv`, `Account::Visa`)
//!   * the PayPal export (from the `paypal.csv` sidecar)
//!
//! Every source is handled gracefully when absent (empty → "no rows").

use crate::paypal::{is_bare_paypal_payment, PayPalTxn};
use crate::query::DateFilter;
use crate::{Account, Transaction};
use chrono::{Datelike, NaiveDate};

/// Fraction of months-in-span that must be present before a source is treated
/// as well-covered. Below this it is flagged as sparse. ~80%.
pub const SPARSE_THRESHOLD: f64 = 0.80;

/// A calendar year-month, e.g. 2025-07. Ordered chronologically, so a `Vec` of
/// these sorts into timeline order and the span between two of them is a simple
/// month count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct YearMonth {
    pub year: i32,
    pub month: u32, // 1..=12
}

impl YearMonth {
    pub fn new(year: i32, month: u32) -> Self {
        Self { year, month }
    }

    pub fn from_date(d: NaiveDate) -> Self {
        Self {
            year: d.year(),
            month: d.month(),
        }
    }

    /// The next calendar month (wrapping December → January of next year).
    pub fn succ(self) -> Self {
        if self.month == 12 {
            Self::new(self.year + 1, 1)
        } else {
            Self::new(self.year, self.month + 1)
        }
    }

    /// Inclusive count of months from `self` to `other` (`other` must be >=
    /// `self`). A single month spans 1; Jan→Dec of one year spans 12.
    pub fn months_to_inclusive(self, other: YearMonth) -> usize {
        if other < self {
            return 0;
        }
        let a = self.year as i64 * 12 + (self.month as i64 - 1);
        let b = other.year as i64 * 12 + (other.month as i64 - 1);
        (b - a + 1) as usize
    }
}

impl std::fmt::Display for YearMonth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

/// The coverage picture for one data source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCoverage {
    /// Human label, e.g. "bank current", "bank visa", "PayPal export".
    pub label: String,
    /// Total rows seen for this source (after any date filter).
    pub row_count: usize,
    /// Earliest / latest dates seen. `None` when the source has no rows.
    pub earliest: Option<NaiveDate>,
    pub latest: Option<NaiveDate>,
    /// Distinct year-months that have at least one row, sorted ascending.
    pub months_present: Vec<YearMonth>,
    /// Year-months in `[earliest..=latest]` that have NO row — the gaps.
    pub missing_months: Vec<YearMonth>,
}

impl SourceCoverage {
    /// Build coverage from an iterator of dates (already date-filtered by the
    /// caller). Order of the input does not matter.
    pub fn from_dates(
        label: impl Into<String>,
        dates: impl IntoIterator<Item = NaiveDate>,
    ) -> Self {
        let mut present: std::collections::BTreeSet<YearMonth> = std::collections::BTreeSet::new();
        let mut earliest: Option<NaiveDate> = None;
        let mut latest: Option<NaiveDate> = None;
        let mut row_count = 0usize;

        for d in dates {
            row_count += 1;
            present.insert(YearMonth::from_date(d));
            earliest = Some(earliest.map_or(d, |e| e.min(d)));
            latest = Some(latest.map_or(d, |l| l.max(d)));
        }

        let months_present: Vec<YearMonth> = present.iter().copied().collect();

        // Missing = every month in the inclusive span that isn't present.
        let missing_months = match (months_present.first(), months_present.last()) {
            (Some(&first), Some(&last)) => {
                let mut missing = Vec::new();
                let mut cur = first;
                while cur <= last {
                    if !present.contains(&cur) {
                        missing.push(cur);
                    }
                    cur = cur.succ();
                }
                missing
            }
            _ => Vec::new(),
        };

        Self {
            label: label.into(),
            row_count,
            earliest,
            latest,
            months_present,
            missing_months,
        }
    }

    /// True when the source has no rows at all.
    pub fn is_empty(&self) -> bool {
        self.row_count == 0
    }

    /// Inclusive number of months between earliest and latest (the months the
    /// source COULD cover). 0 when empty.
    pub fn span_months(&self) -> usize {
        match (self.months_present.first(), self.months_present.last()) {
            (Some(&first), Some(&last)) => first.months_to_inclusive(last),
            _ => 0,
        }
    }

    pub fn months_present_count(&self) -> usize {
        self.months_present.len()
    }

    pub fn gap_count(&self) -> usize {
        self.missing_months.len()
    }

    /// Fraction of the span's months that are present (1.0 = fully covered, no
    /// gaps). An empty source and a single-month source both report 1.0 (no gap
    /// is possible), so only genuine interior gaps drag it down.
    pub fn coverage_fraction(&self) -> f64 {
        let span = self.span_months();
        if span == 0 {
            return 1.0;
        }
        self.months_present_count() as f64 / span as f64
    }

    /// True when coverage looks sparse: there is a real span (>1 month) and
    /// fewer than `SPARSE_THRESHOLD` of its months are present.
    pub fn is_sparse(&self) -> bool {
        self.span_months() > 1 && self.coverage_fraction() < SPARSE_THRESHOLD
    }

    /// A short plain-English verdict line for this source.
    pub fn verdict(&self) -> String {
        if self.is_empty() {
            return format!("{}: no rows — source absent or empty.", self.label);
        }
        let span = self.span_months();
        let present = self.months_present_count();
        let gaps = self.gap_count();
        let (first, last) = (
            self.months_present.first().unwrap(),
            self.months_present.last().unwrap(),
        );

        if gaps == 0 {
            return format!(
                "{}: covers {}..{} — {} of {} months present, no gaps.",
                self.label, first, last, present, span
            );
        }

        let warn = if self.is_sparse() {
            format!(
                " — SPARSE ({:.0}% of months); floor likely under-counts {} spend.",
                self.coverage_fraction() * 100.0,
                self.label
            )
        } else {
            String::new()
        };
        format!(
            "{}: covers {}..{} — {} of {} months present, {} gap{}{}",
            self.label,
            first,
            last,
            present,
            span,
            gaps,
            if gaps == 1 { "" } else { "s" },
            if warn.is_empty() {
                "; floor may under-count outside this span.".to_string()
            } else {
                warn
            }
        )
    }
}

/// The killer metric: how many bank `PAYPAL PAYMENT` rows fall WITHIN the PayPal
/// export's date span — i.e. are even recoverable given current export coverage.
///
/// A low `paypal recover` % is explained almost entirely by this: rows OUTSIDE
/// the export span have no PayPal data to join to, so they can never recover a
/// merchant no matter how good the join is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaypalRecoverability {
    /// Total bank `PAYPAL PAYMENT` rows (after any date filter).
    pub bank_paypal_rows: usize,
    /// The PayPal export's span (None when the export is empty).
    pub export_earliest: Option<NaiveDate>,
    pub export_latest: Option<NaiveDate>,
    /// Bank PAYPAL rows whose date is within `[export_earliest..=export_latest]`.
    pub within_export_span: usize,
}

impl PaypalRecoverability {
    /// Percentage of bank PAYPAL rows that are even recoverable. 0 when there are
    /// no bank PAYPAL rows.
    pub fn pct_within(&self) -> f64 {
        if self.bank_paypal_rows == 0 {
            0.0
        } else {
            (self.within_export_span as f64 / self.bank_paypal_rows as f64) * 100.0
        }
    }

    /// Bank PAYPAL rows outside the export span — unrecoverable for lack of
    /// export coverage.
    pub fn outside_export_span(&self) -> usize {
        self.bank_paypal_rows
            .saturating_sub(self.within_export_span)
    }

    /// Plain-English verdict explaining the recoverable ceiling.
    pub fn verdict(&self) -> String {
        if self.bank_paypal_rows == 0 {
            return "PayPal recoverability: no bank PAYPAL PAYMENT rows found.".to_string();
        }
        match (self.export_earliest, self.export_latest) {
            (Some(e), Some(l)) => format!(
                "PayPal recoverability: {} of {} bank PAYPAL rows ({:.1}%) fall within the export span {}..{}; \
                 the other {} are unrecoverable until the export is widened.",
                self.within_export_span,
                self.bank_paypal_rows,
                self.pct_within(),
                e,
                l,
                self.outside_export_span()
            ),
            _ => format!(
                "PayPal recoverability: {} bank PAYPAL rows but the PayPal export is empty — \
                 0% recoverable; run `fd-budget paypal import <export.csv>`.",
                self.bank_paypal_rows
            ),
        }
    }
}

/// Compute the PayPal recoverability metric from bank transactions + the PayPal
/// export rows, honouring the date filter on the bank side.
pub fn paypal_recoverability(
    transactions: &[Transaction],
    paypal_rows: &[PayPalTxn],
    filter: DateFilter,
) -> PaypalRecoverability {
    let export_earliest = paypal_rows.iter().map(|p| p.date).min();
    let export_latest = paypal_rows.iter().map(|p| p.date).max();

    let mut bank_paypal_rows = 0usize;
    let mut within = 0usize;
    for tx in transactions {
        if !filter.matches(tx.date) {
            continue;
        }
        if !is_bare_paypal_payment(tx) {
            continue;
        }
        bank_paypal_rows += 1;
        if let (Some(e), Some(l)) = (export_earliest, export_latest) {
            if tx.date >= e && tx.date <= l {
                within += 1;
            }
        }
    }

    PaypalRecoverability {
        bank_paypal_rows,
        export_earliest,
        export_latest,
        within_export_span: within,
    }
}

/// The whole coverage report: one entry per source plus the PayPal
/// recoverability metric.
#[derive(Debug, Clone)]
pub struct CoverageReport {
    pub sources: Vec<SourceCoverage>,
    pub paypal: PaypalRecoverability,
}

impl CoverageReport {
    /// Build the full report from the loaded sources and a date filter.
    pub fn build(
        transactions: &[Transaction],
        paypal_rows: &[PayPalTxn],
        filter: DateFilter,
    ) -> Self {
        let current_dates = transactions
            .iter()
            .filter(|t| t.account == Account::Current && filter.matches(t.date))
            .map(|t| t.date);
        let visa_dates = transactions
            .iter()
            .filter(|t| t.account == Account::Visa && filter.matches(t.date))
            .map(|t| t.date);
        let paypal_dates = paypal_rows
            .iter()
            .filter(|p| filter.matches(p.date))
            .map(|p| p.date);

        let sources = vec![
            SourceCoverage::from_dates("bank current", current_dates),
            SourceCoverage::from_dates("bank visa", visa_dates),
            SourceCoverage::from_dates("PayPal export", paypal_dates),
        ];

        let paypal = paypal_recoverability(transactions, paypal_rows, filter);

        Self { sources, paypal }
    }

    /// Render the report as a plaintext table + per-source verdicts.
    pub fn render(&self) -> String {
        let mut out = String::new();

        // Table header.
        out.push_str(&format!(
            "{:<16} {:>6}  {:>12} {:>12}  {:>7} {:>4} {:>5}\n",
            "Source", "Rows", "Earliest", "Latest", "Months", "Gaps", "Cov%"
        ));
        out.push_str(&format!("{}\n", "-".repeat(68)));

        for s in &self.sources {
            let earliest = s
                .earliest
                .map(|d| d.to_string())
                .unwrap_or_else(|| "-".to_string());
            let latest = s
                .latest
                .map(|d| d.to_string())
                .unwrap_or_else(|| "-".to_string());
            let months = if s.is_empty() {
                "-".to_string()
            } else {
                format!("{}/{}", s.months_present_count(), s.span_months())
            };
            let cov = if s.is_empty() {
                "-".to_string()
            } else {
                format!("{:.0}", s.coverage_fraction() * 100.0)
            };
            let flag = if s.is_sparse() { " *" } else { "" };
            out.push_str(&format!(
                "{:<16} {:>6}  {:>12} {:>12}  {:>7} {:>4} {:>5}{}\n",
                s.label,
                s.row_count,
                earliest,
                latest,
                months,
                s.gap_count(),
                cov,
                flag,
            ));
        }
        out.push_str(&format!("{}\n", "-".repeat(68)));
        out.push_str("(* = sparse coverage — below 80% of months in span)\n");

        // List the gaps per source.
        for s in &self.sources {
            if !s.missing_months.is_empty() {
                let list = format_month_list(&s.missing_months);
                out.push_str(&format!("\n{} missing month(s): {}\n", s.label, list));
            }
        }

        // Verdicts.
        out.push('\n');
        out.push_str("Verdict:\n");
        for s in &self.sources {
            out.push_str(&format!("  {}\n", s.verdict()));
        }
        out.push_str(&format!("  {}\n", self.paypal.verdict()));

        out
    }
}

/// Format a list of missing months: up to 12 listed inline, otherwise a count
/// with the first and last shown (so a huge gap list stays readable).
fn format_month_list(months: &[YearMonth]) -> String {
    const MAX_INLINE: usize = 12;
    if months.is_empty() {
        return String::new();
    }
    if months.len() <= MAX_INLINE {
        months
            .iter()
            .map(|m| m.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        format!(
            "{} months ({} .. {})",
            months.len(),
            months.first().unwrap(),
            months.last().unwrap()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Account, TxType};
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn tx(date: &str, account: Account, amount: &str, raw_desc: &str) -> Transaction {
        Transaction {
            date: d(date),
            account,
            tx_type: TxType::Unknown(0),
            amount: Decimal::from_str(amount).unwrap(),
            description: raw_desc.to_string(),
            raw_description: raw_desc.to_string(),
            balance: None,
            tags: Vec::new(),
            import_id: format!("{date}-{raw_desc}"),
        }
    }

    fn pp(date: &str, name: &str, amount: &str) -> PayPalTxn {
        PayPalTxn {
            date: d(date),
            time: None,
            name: name.to_string(),
            txn_type: "Express Checkout Payment".to_string(),
            status: "Completed".to_string(),
            currency: "GBP".to_string(),
            amount: Decimal::from_str(amount).unwrap(),
            exchange_rate: None,
            balance: None,
            transaction_id: format!("PP-{date}-{name}"),
            item_title: String::new(),
        }
    }

    // --- YearMonth arithmetic --------------------------------------------

    #[test]
    fn year_month_span_and_succ() {
        let jan = YearMonth::new(2025, 1);
        let dec = YearMonth::new(2025, 12);
        assert_eq!(jan.months_to_inclusive(dec), 12);
        assert_eq!(jan.months_to_inclusive(jan), 1);
        assert_eq!(dec.succ(), YearMonth::new(2026, 1));
        // crossing a year boundary
        let nov = YearMonth::new(2024, 11);
        let feb = YearMonth::new(2025, 2);
        assert_eq!(nov.months_to_inclusive(feb), 4); // Nov,Dec,Jan,Feb
    }

    // --- month-gap detection ---------------------------------------------

    #[test]
    fn detects_interior_month_gaps() {
        // Jan, Feb, then a gap at Mar+Apr, then May. Span = 5 months, 3 present,
        // 2 gaps (Mar, Apr).
        let dates = vec![
            d("2025-01-10"),
            d("2025-02-15"),
            d("2025-05-03"),
            d("2025-05-20"), // same month, doesn't add a new month
        ];
        let cov = SourceCoverage::from_dates("bank visa", dates);
        assert_eq!(cov.earliest, Some(d("2025-01-10")));
        assert_eq!(cov.latest, Some(d("2025-05-20")));
        assert_eq!(cov.span_months(), 5);
        assert_eq!(cov.months_present_count(), 3);
        assert_eq!(cov.gap_count(), 2);
        assert_eq!(
            cov.missing_months,
            vec![YearMonth::new(2025, 3), YearMonth::new(2025, 4)]
        );
        // 3/5 = 60% < 80% → sparse.
        assert!(cov.is_sparse());
        assert!(cov.verdict().contains("SPARSE"));
        assert!(cov.verdict().contains("2025-01..2025-05"));
    }

    #[test]
    fn gap_spanning_year_boundary() {
        // Nov 2024 then Feb 2025: Dec 2024 + Jan 2025 missing.
        let dates = vec![d("2024-11-15"), d("2025-02-02")];
        let cov = SourceCoverage::from_dates("bank current", dates);
        assert_eq!(cov.span_months(), 4);
        assert_eq!(cov.months_present_count(), 2);
        assert_eq!(
            cov.missing_months,
            vec![YearMonth::new(2024, 12), YearMonth::new(2025, 1)]
        );
    }

    // --- fully-covered source (no gaps) ----------------------------------

    #[test]
    fn fully_covered_source_has_no_gaps() {
        // Three consecutive months, each present.
        let dates = vec![d("2025-01-05"), d("2025-02-09"), d("2025-03-30")];
        let cov = SourceCoverage::from_dates("bank current", dates);
        assert_eq!(cov.span_months(), 3);
        assert_eq!(cov.months_present_count(), 3);
        assert_eq!(cov.gap_count(), 0);
        assert!(cov.missing_months.is_empty());
        assert!(!cov.is_sparse());
        assert!((cov.coverage_fraction() - 1.0).abs() < f64::EPSILON);
        let v = cov.verdict();
        assert!(v.contains("no gaps"), "verdict was: {v}");
        assert!(v.contains("3 of 3 months"));
    }

    #[test]
    fn single_month_source_is_not_sparse() {
        // One month only: no gap is possible, never flagged sparse.
        let dates = vec![d("2025-06-01"), d("2025-06-15")];
        let cov = SourceCoverage::from_dates("PayPal export", dates);
        assert_eq!(cov.span_months(), 1);
        assert_eq!(cov.gap_count(), 0);
        assert!(!cov.is_sparse());
    }

    // --- a source absent --------------------------------------------------

    #[test]
    fn absent_source_is_handled_gracefully() {
        let cov = SourceCoverage::from_dates("bank visa", Vec::<NaiveDate>::new());
        assert!(cov.is_empty());
        assert_eq!(cov.row_count, 0);
        assert_eq!(cov.earliest, None);
        assert_eq!(cov.latest, None);
        assert_eq!(cov.span_months(), 0);
        assert_eq!(cov.gap_count(), 0);
        assert!(!cov.is_sparse());
        assert!(cov.verdict().contains("no rows"));
    }

    #[test]
    fn report_build_handles_all_sources_absent() {
        // No transactions, no paypal rows at all.
        let report = CoverageReport::build(&[], &[], DateFilter::default());
        assert_eq!(report.sources.len(), 3);
        assert!(report.sources.iter().all(|s| s.is_empty()));
        assert_eq!(report.paypal.bank_paypal_rows, 0);
        // render must not panic on the empty case.
        let rendered = report.render();
        assert!(rendered.contains("no rows"));
    }

    // --- PAYPAL-rows-within-export-coverage count ------------------------

    #[test]
    fn paypal_rows_within_export_coverage() {
        // Bank PAYPAL rows across Jan..Jun 2025; the PayPal export only covers
        // Mar..May 2025. So 3 of the 5 bank rows are within the export span and
        // are even recoverable; the Jan and Jun rows are not.
        let transactions = vec![
            tx("2025-01-10", Account::Current, "-10.00", "PAYPAL PAYMENT"), // before export
            tx("2025-03-10", Account::Current, "-20.00", "PAYPAL PAYMENT"), // within
            tx("2025-04-10", Account::Current, "-30.00", "PAYPAL PAYMENT"), // within
            tx("2025-05-10", Account::Current, "-40.00", "PAYPAL PAYMENT"), // within
            tx("2025-06-10", Account::Current, "-50.00", "PAYPAL PAYMENT"), // after export
            // a non-PayPal control that must not be counted
            tx("2025-04-11", Account::Current, "-5.00", "TESCO STORES"),
        ];
        let paypal_rows = vec![
            pp("2025-03-05", "Streamflix", "-20.00"),
            pp("2025-05-25", "Acme Shop", "-40.00"),
        ];
        let m = paypal_recoverability(&transactions, &paypal_rows, DateFilter::default());
        assert_eq!(m.bank_paypal_rows, 5);
        assert_eq!(m.within_export_span, 3);
        assert_eq!(m.outside_export_span(), 2);
        assert_eq!(m.export_earliest, Some(d("2025-03-05")));
        assert_eq!(m.export_latest, Some(d("2025-05-25")));
        assert!((m.pct_within() - 60.0).abs() < 1e-9);
        let v = m.verdict();
        assert!(v.contains("3 of 5"), "verdict was: {v}");
        assert!(v.contains("60.0%"));
    }

    #[test]
    fn paypal_recoverability_with_empty_export() {
        // Bank has PAYPAL rows but the export is empty → 0% recoverable.
        let transactions = vec![
            tx("2025-01-10", Account::Current, "-10.00", "PAYPAL PAYMENT"),
            tx("2025-02-10", Account::Current, "-20.00", "PAYPAL PAYMENT"),
        ];
        let m = paypal_recoverability(&transactions, &[], DateFilter::default());
        assert_eq!(m.bank_paypal_rows, 2);
        assert_eq!(m.within_export_span, 0);
        assert_eq!(m.export_earliest, None);
        assert_eq!(m.pct_within(), 0.0);
        assert!(m.verdict().contains("export is empty"));
    }

    #[test]
    fn paypal_recoverability_respects_date_filter() {
        // Same data as the within test but filtered to 2025 only; a 2024 PAYPAL
        // row is excluded from bank_paypal_rows entirely.
        let transactions = vec![
            tx("2024-12-10", Account::Current, "-99.00", "PAYPAL PAYMENT"), // filtered out
            tx("2025-03-10", Account::Current, "-20.00", "PAYPAL PAYMENT"),
            tx("2025-06-10", Account::Current, "-50.00", "PAYPAL PAYMENT"),
        ];
        // Export span 2025-03-01..2025-03-31 brackets the Mar 10 bank row but
        // not the Jun row.
        let paypal_rows = vec![
            pp("2025-03-01", "Streamflix", "-20.00"),
            pp("2025-03-31", "Acme Shop", "-99.00"),
        ];
        let m = paypal_recoverability(&transactions, &paypal_rows, DateFilter::year(2025));
        // Only the two 2025 rows count; the 2024 row is filtered.
        assert_eq!(m.bank_paypal_rows, 2);
        assert_eq!(m.within_export_span, 1); // only the Mar row is within the Mar span
    }

    // --- report split by account -----------------------------------------

    #[test]
    fn report_splits_bank_by_account() {
        let transactions = vec![
            tx("2025-01-10", Account::Current, "-10.00", "TESCO"),
            tx("2025-02-10", Account::Current, "-20.00", "ALDI"),
            // visa only in Jan and Mar → Feb gap
            tx("2025-01-15", Account::Visa, "-100.00", "AMAZON"),
            tx("2025-03-15", Account::Visa, "-200.00", "JOHN LEWIS"),
        ];
        let report = CoverageReport::build(&transactions, &[], DateFilter::default());
        let current = report
            .sources
            .iter()
            .find(|s| s.label == "bank current")
            .unwrap();
        let visa = report
            .sources
            .iter()
            .find(|s| s.label == "bank visa")
            .unwrap();
        assert_eq!(current.row_count, 2);
        assert_eq!(current.gap_count(), 0); // Jan,Feb consecutive
        assert_eq!(visa.row_count, 2);
        assert_eq!(visa.gap_count(), 1); // Feb missing
        assert_eq!(visa.missing_months, vec![YearMonth::new(2025, 2)]);
    }

    #[test]
    fn render_does_not_panic_and_shows_metric() {
        let transactions = vec![
            tx("2025-01-10", Account::Current, "-10.00", "PAYPAL PAYMENT"),
            tx("2025-05-10", Account::Current, "-50.00", "PAYPAL PAYMENT"),
        ];
        let paypal_rows = vec![pp("2025-05-05", "Streamflix", "-50.00")];
        let report = CoverageReport::build(&transactions, &paypal_rows, DateFilter::default());
        let out = report.render();
        assert!(out.contains("Source"));
        assert!(out.contains("PayPal recoverability"));
        assert!(out.contains("Verdict"));
    }
}
