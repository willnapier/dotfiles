//! Live sources for the House Model: practiceforge (Xero) and fd-budget.
//! Both are read through their own CLIs; nothing here talks to a network directly.
//! Only aggregates cross this boundary (pseudonymised billing report, aggregate
//! cashflow model, fd-budget summary lines).

use anyhow::{anyhow, bail, Context, Result};
use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EarningWeek {
    pub earning_weeks: u32,
    pub sse_per_earning_week_mean: f64,
    pub sse_per_earning_week_median: f64,
    pub planned_earning_weeks_annual: Option<u32>,
}

/// The fields of practiceforge's `DephiedReport` that the House Model can use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillingReport {
    pub period_start: NaiveDate,
    pub period_end: NaiveDate,
    pub weeks_in_period: u32,
    pub total_billed: f64,
    pub session_equivalents: f64,
    pub average_sse_per_week: f64,
    pub earning_week: Option<EarningWeek>,
}

impl BillingReport {
    /// Realised fee per single-session-equivalent.
    pub fn fee_per_sse(&self) -> Option<f64> {
        if self.session_equivalents > 0.0 {
            Some(self.total_billed / self.session_equivalents)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveExpenses {
    pub accounts: Vec<(String, f64)>,
    pub as_of: Option<String>,
    pub business_expenses_used: Option<f64>,
    pub one_off_stripped: Option<f64>,
    pub recurring_subtotal: Option<f64>,
    pub source: Option<String>,
}

/// The fields of practiceforge's aggregate `CashflowModel` that the House Model can use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cashflow {
    pub revenue: f64,
    pub floor_fund_life_only: f64,
    pub net_living_funded: f64,
    pub live_expenses: Option<LiveExpenses>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl Cashflow {
    pub fn account(&self, name: &str) -> Option<f64> {
        let n = name.to_lowercase();
        self.live_expenses.as_ref()?.accounts.iter().find(|(a, _)| a.to_lowercase() == n).map(|(_, v)| *v)
    }
    /// Sum of the small recurring P&L lines (the model's "Other recurring").
    pub fn small_lines(&self) -> Option<f64> {
        let skip = ["rent", "motor vehicle expenses", "pensions costs", "corporation tax", "bad debt expense", "wages", "salaries", "directors remuneration"];
        let le = self.live_expenses.as_ref()?;
        Some(le.accounts.iter().filter(|(a, _)| !skip.contains(&a.to_lowercase().as_str())).map(|(_, v)| *v).sum())
    }
}

/// fd-budget `stats` summary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpendFloor {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub spend: f64,
    pub income: f64,
    pub business: f64,
    pub one_off: f64,
    pub untagged_debits: u32,
}

impl SpendFloor {
    pub fn days(&self) -> i64 {
        (self.to - self.from).num_days().max(1)
    }
    pub fn spend_annualised(&self) -> f64 {
        self.spend * 365.25 / self.days() as f64
    }
    pub fn stale_days(&self, today: NaiveDate) -> i64 {
        (today - self.to).num_days()
    }
}

pub fn parse_billing_report(json: &str) -> Result<BillingReport> {
    serde_json::from_str(json).context("parsing practiceforge billing report JSON")
}

pub fn parse_cashflow(json: &str) -> Result<Cashflow> {
    serde_json::from_str(json).context("parsing practiceforge cashflow JSON")
}

fn money_after(line: &str) -> Option<f64> {
    let i = line.find('£')?;
    let rest = &line[i + '£'.len_utf8()..];
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == ',').collect();
    num.replace(',', "").parse().ok()
}

pub fn parse_fd_budget_stats(text: &str) -> Result<SpendFloor> {
    let mut from = None;
    let mut to = None;
    let mut spend = None;
    let mut income = None;
    let mut business = None;
    let mut one_off = None;
    let mut untagged_debits = 0u32;
    for line in text.lines() {
        let l = line.trim();
        if let Some(r) = l.strip_prefix("Date range:") {
            let parts: Vec<&str> = r.split(" to ").map(|s| s.trim()).collect();
            if parts.len() == 2 {
                from = NaiveDate::parse_from_str(parts[0], "%Y-%m-%d").ok();
                to = NaiveDate::parse_from_str(parts[1], "%Y-%m-%d").ok();
            }
        } else if l.starts_with("Spend (") {
            spend = money_after(l);
        } else if l.starts_with("Income (") {
            income = money_after(l);
        } else if l.starts_with("Business (") {
            business = money_after(l);
        } else if l.starts_with("One-off (") {
            one_off = money_after(l);
        } else if l.starts_with("note:") {
            untagged_debits = l.split_whitespace().nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
        }
    }
    Ok(SpendFloor {
        from: from.ok_or_else(|| anyhow!("fd-budget stats: no 'Date range:' line"))?,
        to: to.ok_or_else(|| anyhow!("fd-budget stats: bad 'Date range:' line"))?,
        spend: spend.ok_or_else(|| anyhow!("fd-budget stats: no 'Spend (' line"))?,
        income: income.unwrap_or(0.0),
        business: business.unwrap_or(0.0),
        one_off: one_off.unwrap_or(0.0),
        untagged_debits,
    })
}

fn run(cmd: &str, args: &[String]) -> Result<String> {
    let out = Command::new(cmd).args(args).output().with_context(|| format!("running {cmd} (is it on PATH?)"))?;
    if !out.status.success() {
        bail!("{cmd} {} failed ({}): {}", args.join(" "), out.status, String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn fetch_billing_report(from: NaiveDate, to: NaiveDate) -> Result<BillingReport> {
    let args = ["billing", "report", "--from", &from.to_string(), "--to", &to.to_string(), "--anonymised-jsonl"].map(String::from);
    parse_billing_report(&run("practiceforge", &args)?)
}

pub fn fetch_cashflow() -> Result<Cashflow> {
    let args = ["billing", "cashflow", "--anonymised-jsonl"].map(String::from);
    parse_cashflow(&run("practiceforge", &args)?)
}

pub fn fetch_spend_floor(since: NaiveDate) -> Result<SpendFloor> {
    let args = ["stats", "--since", &since.to_string()].map(String::from);
    parse_fd_budget_stats(&run("fd-budget", &args)?)
}

pub fn default_report_from(today: NaiveDate) -> NaiveDate {
    today - Duration::days(105)
}

pub fn default_spend_since(today: NaiveDate) -> NaiveDate {
    today - Duration::days(365)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fd_budget_summary() {
        let text = "Transactions: 1358\n  Current: 1249\nTagged: 1189 (87.6%)\nUntagged: 169\nDate range: 2025-09-01 to 2026-06-17\n\nSpend (recurring personal living cost):                £69941.64\nIncome (all credits):                                  £153535.97\nBusiness (professional — excluded from floor):         £14310.88\nOne-off (lumpy — excluded from floor):                 £12296.08  (≈£12296/yr amortised)\nExcluded (transfer/income/tax):                        £57931.85\n  note: 152 untagged debit(s) still counted as spend — tag any\n";
        let f = parse_fd_budget_stats(text).unwrap();
        assert_eq!(f.from, NaiveDate::from_ymd_opt(2025, 9, 1).unwrap());
        assert_eq!(f.to, NaiveDate::from_ymd_opt(2026, 6, 17).unwrap());
        assert!((f.spend - 69941.64).abs() < 1e-9);
        assert!((f.business - 14310.88).abs() < 1e-9);
        assert_eq!(f.untagged_debits, 152);
        assert_eq!(f.days(), 289);
        let ann = f.spend_annualised();
        assert!((ann - 69941.64 * 365.25 / 289.0).abs() < 1e-6);
        assert_eq!(f.stale_days(NaiveDate::from_ymd_opt(2026, 9, 13).unwrap()), 88);
    }

    #[test]
    fn fd_budget_parse_fails_closed_without_spend_line() {
        assert!(parse_fd_budget_stats("Date range: 2025-01-01 to 2025-02-01\n").is_err());
        assert!(parse_fd_budget_stats("Spend (x): £1.00\n").is_err());
    }

    #[test]
    fn parses_billing_report_subset_and_ignores_extra_fields() {
        let j = r#"{"period_start":"2026-06-01","period_end":"2026-09-13","weeks_in_period":15,"total_billed":82450.0,"currency":"GBP","average_fee_per_week":5500.0,"session_count":400,"average_sessions_per_week":26.7,"session_equivalents":476.1,"average_sse_per_week":31.74,"arrears":[],"per_client_breakdown":[],"payer_mix":{"self_pay":0.7},"earning_week":{"earning_weeks":12,"holiday_weeks":0,"planned_earning_weeks_annual":42,"sse_per_earning_week_mean":32.675,"sse_per_earning_week_median":32.15,"busiest_week_sse":44.0,"undated_sse":5.0}}"#;
        let r = parse_billing_report(j).unwrap();
        assert!((r.fee_per_sse().unwrap() - 82450.0 / 476.1).abs() < 1e-9);
        assert_eq!(r.earning_week.as_ref().unwrap().earning_weeks, 12);
    }

    #[test]
    fn parses_cashflow_and_sums_small_lines() {
        let j = r#"{"revenue":282779.74,"floor_fund_life_only":27.8,"net_living_funded":85000.0,"live_expenses":{"accounts":[["Advertising & Marketing",654.0],["Audit & Accountancy fees",2495.0],["Bad debt expense",3920.0],["Motor Vehicle Expenses",7634.8],["Pensions Costs",13750.0],["Rent",36100.0],["Corporation Tax",52377.27]],"as_of":"2026-09-13","business_expenses_used":44434.0,"one_off_stripped":6968.0,"recurring_subtotal":51402.0,"source":"xero_profit_and_loss","total_operating_expenses":121449.27},"warnings":["x"]}"#;
        let c = parse_cashflow(j).unwrap();
        assert_eq!(c.account("Rent"), Some(36100.0));
        assert!((c.small_lines().unwrap() - (654.0 + 2495.0)).abs() < 1e-9);
        assert_eq!(c.warnings.len(), 1);
    }
}
