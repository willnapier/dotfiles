//! Merchant-recovery join: recover the real merchant behind a bare bank
//! `PAYPAL PAYMENT` debit from the PayPal sidecar, REPRODUCIBLY (no AI).
//!
//! Mirrors the `enrich → matches.jsonl` sidecar pattern: one output row per
//! recovered bank row, keyed by `bank_import_id`, written to
//! `paypal_matches.jsonl`. `transactions.csv` is NEVER rewritten here.
//!
//! Three real cases (see the build primer):
//!
//! 1. **Direct GBP** — bank `-12.99` ↔ a PayPal GBP payment leg `-12.99` that
//!    carries the merchant. One hop.
//! 2. **Two-leg** — each purchase is a payment leg (`-£X`, merchant) PLUS a
//!    `Bank Deposit to PP Account` (`+£X`). The DEPOSIT equals the bank debit;
//!    the PAYMENT carries the merchant. We anchor on the deposit, then follow to
//!    the same-amount GBP payment leg.
//! 3. **FX chain** — bank GBP debit → deposit (`+GBP`) → `General Currency
//!    Conversion` (`-GBP`, same amount) → foreign payment (`-EUR/USD`, carries
//!    merchant). We follow deposit → conversion → nearest foreign payment leg.
//!
//! Disambiguation: when an amount recurs (several `-12.99` in a month), each
//! bank row is matched to the **nearest-dated** PayPal candidate, and each
//! PayPal leg is consumed at most once (greedy by date proximity).

use crate::paypal::store::PayPalTxn;
use crate::Transaction;
use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

/// Default window (days) between a bank `PAYPAL PAYMENT` posting and the
/// matching PayPal-side leg. Card postings can lag the PayPal date by a few
/// days; 5 is generous without colliding across a recurring monthly charge.
pub const DEFAULT_BANK_WINDOW_DAYS: i64 = 5;

/// Window (days) used to walk WITHIN the PayPal side (deposit → payment leg,
/// conversion → foreign payment). These legs are usually same-day or within a
/// day of each other.
const PP_CHAIN_WINDOW_DAYS: i64 = 2;

/// Amount tolerance for GBP-side matches (bank↔deposit, deposit↔conversion,
/// bank↔direct-payment). PayPal GBP legs match the bank debit to the penny.
const GBP_TOLERANCE: Decimal = Decimal::from_parts(1, 0, 0, false, 2); // 0.01

/// Which chain a recovery followed — recorded in the sidecar for auditability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Leg {
    /// bank → GBP payment leg (one hop).
    DirectGbp,
    /// bank → deposit → GBP payment leg.
    TwoLeg,
    /// bank → deposit → currency conversion → foreign payment leg.
    FxChain,
}

impl Leg {
    pub fn as_str(&self) -> &'static str {
        match self {
            Leg::DirectGbp => "direct-gbp",
            Leg::TwoLeg => "two-leg",
            Leg::FxChain => "fx-chain",
        }
    }
}

/// Recovery confidence. `High` = unambiguous single nearest candidate;
/// `Ambiguous` would be used if the disambiguator could not pick cleanly (we
/// currently always resolve to a single nearest leg, so recoveries are High).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecoveryConfidence {
    High,
    Medium,
}

impl RecoveryConfidence {
    pub fn as_str(&self) -> &'static str {
        match self {
            RecoveryConfidence::High => "high",
            RecoveryConfidence::Medium => "medium",
        }
    }
}

/// One recovered bank row.
#[derive(Debug, Clone)]
pub struct Recovery {
    pub bank_import_id: String,
    pub recovered_merchant: String,
    /// Currency the merchant was actually paid in (GBP for direct/two-leg,
    /// EUR/USD for FX).
    pub currency: String,
    /// The merchant-leg amount in its own currency (e.g. -299.40 EUR).
    pub merchant_amount: Decimal,
    /// The bank GBP amount this recovery explains (negative).
    pub bank_amount: Decimal,
    pub confidence: RecoveryConfidence,
    pub leg: Leg,
    /// The PayPal Transaction IDs that formed the chain, in order.
    pub chain_txn_ids: Vec<String>,
}

/// True if the bank row is a bare PayPal payment that needs recovery.
///
/// FD posts these as `PAYPAL PAYMENT` (the merchant stripped). We match on
/// `raw_description` containing "paypal" AND it being a debit — the recovery
/// only applies to outgoing PayPal spend.
pub fn is_bare_paypal_payment(tx: &Transaction) -> bool {
    if !tx.is_debit() {
        return false;
    }
    let d = tx.raw_description.to_lowercase();
    d.contains("paypal")
}

fn within(a: Decimal, b: Decimal, tol: Decimal) -> bool {
    (a - b).abs() <= tol
}

fn date_delta(a: NaiveDate, b: NaiveDate) -> i64 {
    (a - b).num_days().abs()
}

/// Options controlling the bank↔PayPal date window.
#[derive(Debug, Clone, Copy)]
pub struct RecoverOptions {
    pub bank_window_days: i64,
}

impl Default for RecoverOptions {
    fn default() -> Self {
        Self {
            bank_window_days: DEFAULT_BANK_WINDOW_DAYS,
        }
    }
}

/// Summary stats for the recovery run.
#[derive(Debug, Clone, Default)]
pub struct RecoverSummary {
    /// Bank rows that looked like bare PayPal payments.
    pub bare_paypal_rows: usize,
    /// How many we recovered a merchant for.
    pub recovered: usize,
    pub direct_gbp: usize,
    pub two_leg: usize,
    pub fx_chain: usize,
    /// Total £ value of bare PayPal rows (abs).
    pub total_value: Decimal,
    /// £ value recovered (abs).
    pub recovered_value: Decimal,
}

impl RecoverSummary {
    /// Percentage of £-value recovered.
    pub fn pct_value_recovered(&self) -> f64 {
        if self.total_value.is_zero() {
            0.0
        } else {
            use rust_decimal::prelude::ToPrimitive;
            (self.recovered_value / self.total_value)
                .to_f64()
                .unwrap_or(0.0)
                * 100.0
        }
    }
}

/// The core join. For each bare bank `PAYPAL PAYMENT` debit, recover the
/// merchant by following the appropriate leg/chain.
///
/// Greedy disambiguation: bank rows are processed oldest-first; each PayPal
/// leg (payment, deposit, conversion, foreign payment) is consumed at most once
/// so recurring identical amounts in a month map to distinct PayPal entries by
/// nearest date.
pub fn recover(
    transactions: &[Transaction],
    paypal: &[PayPalTxn],
    opts: RecoverOptions,
) -> (Vec<Recovery>, RecoverSummary) {
    // Index PayPal rows by category for fast candidate scans. We track consumed
    // Transaction IDs to enforce one-leg-one-use.
    let mut consumed: HashSet<String> = HashSet::new();

    // Bank rows needing recovery, oldest first (stable disambiguation).
    let mut bank_rows: Vec<&Transaction> = transactions
        .iter()
        .filter(|t| is_bare_paypal_payment(t))
        .collect();
    bank_rows.sort_by(|a, b| a.date.cmp(&b.date).then(a.import_id.cmp(&b.import_id)));

    let mut summary = RecoverSummary {
        bare_paypal_rows: bank_rows.len(),
        ..Default::default()
    };
    for tx in &bank_rows {
        summary.total_value += tx.amount.abs();
    }

    let mut recoveries = Vec::new();

    for tx in bank_rows {
        let bank_abs = tx.amount.abs();

        // Order matters. A genuine two-leg / FX purchase ALSO contains a GBP
        // payment leg of the same amount, which would spuriously satisfy the
        // direct-GBP test. The distinguishing signal is the presence of a
        // `Bank Deposit to PP Account` equal to the bank debit (direct purchases
        // have NO such deposit). So we try the deposit-anchored chains FIRST and
        // fall back to direct-GBP only when no matching deposit exists.

        // --- Anchor: a Bank Deposit equal to the bank debit (cases 2 & 3). ---
        if let Some(dep_idx) =
            nearest_unconsumed(paypal, &consumed, tx.date, opts.bank_window_days, |p| {
                p.is_deposit() && within(p.amount.abs(), bank_abs, GBP_TOLERANCE)
            })
        {
            let dep_date = paypal[dep_idx].date;
            let dep_id = paypal[dep_idx].transaction_id.clone();

            // --- Case 2: Two-leg — deposit → same-amount GBP payment leg. ---
            if let Some(pay_idx) =
                nearest_unconsumed(paypal, &consumed, dep_date, PP_CHAIN_WINDOW_DAYS, |p| {
                    p.is_payment_leg()
                        && p.currency.eq_ignore_ascii_case("GBP")
                        && within(p.amount.abs(), bank_abs, GBP_TOLERANCE)
                })
            {
                let pay = &paypal[pay_idx];
                consumed.insert(dep_id.clone());
                consumed.insert(pay.transaction_id.clone());
                recoveries.push(Recovery {
                    bank_import_id: tx.import_id.clone(),
                    recovered_merchant: pay.name.trim().to_string(),
                    currency: pay.currency.clone(),
                    merchant_amount: pay.amount,
                    bank_amount: tx.amount,
                    confidence: RecoveryConfidence::High,
                    leg: Leg::TwoLeg,
                    chain_txn_ids: vec![dep_id, pay.transaction_id.clone()],
                });
                summary.recovered += 1;
                summary.two_leg += 1;
                summary.recovered_value += bank_abs;
                continue;
            }

            // --- Case 3: FX chain — deposit → conversion (-GBP, same amt) →
            //     nearest foreign payment leg (non-GBP, carries merchant). ---
            if let Some(conv_idx) =
                nearest_unconsumed(paypal, &consumed, dep_date, PP_CHAIN_WINDOW_DAYS, |p| {
                    p.is_currency_conversion()
                        && p.is_debit()
                        && p.currency.eq_ignore_ascii_case("GBP")
                        && within(p.amount.abs(), bank_abs, GBP_TOLERANCE)
                })
            {
                let conv_date = paypal[conv_idx].date;
                let conv_id = paypal[conv_idx].transaction_id.clone();
                // Foreign payment leg: a non-GBP payment carrying a merchant,
                // nearest to the conversion date.
                if let Some(fx_idx) =
                    nearest_unconsumed(paypal, &consumed, conv_date, PP_CHAIN_WINDOW_DAYS, |p| {
                        p.is_payment_leg() && !p.currency.eq_ignore_ascii_case("GBP")
                    })
                {
                    let fx = &paypal[fx_idx];
                    consumed.insert(dep_id.clone());
                    consumed.insert(conv_id.clone());
                    consumed.insert(fx.transaction_id.clone());
                    recoveries.push(Recovery {
                        bank_import_id: tx.import_id.clone(),
                        recovered_merchant: fx.name.trim().to_string(),
                        currency: fx.currency.clone(),
                        merchant_amount: fx.amount,
                        bank_amount: tx.amount,
                        confidence: RecoveryConfidence::High,
                        leg: Leg::FxChain,
                        chain_txn_ids: vec![dep_id, conv_id, fx.transaction_id.clone()],
                    });
                    summary.recovered += 1;
                    summary.fx_chain += 1;
                    summary.recovered_value += bank_abs;
                    continue;
                }
            }
            // A deposit matched but neither chain resolved — fall through to the
            // direct-GBP attempt below. We did NOT consume the deposit, so it
            // stays available for another same-amount bank row.
        }

        // --- Case 1 (fallback): Direct GBP — bank ↔ a GBP payment leg, same
        //     amount, with no funding deposit. ---
        if let Some(idx) =
            nearest_unconsumed(paypal, &consumed, tx.date, opts.bank_window_days, |p| {
                p.is_payment_leg()
                    && p.currency.eq_ignore_ascii_case("GBP")
                    && within(p.amount.abs(), bank_abs, GBP_TOLERANCE)
            })
        {
            let leg = &paypal[idx];
            consumed.insert(leg.transaction_id.clone());
            recoveries.push(Recovery {
                bank_import_id: tx.import_id.clone(),
                recovered_merchant: leg.name.trim().to_string(),
                currency: leg.currency.clone(),
                merchant_amount: leg.amount,
                bank_amount: tx.amount,
                confidence: RecoveryConfidence::High,
                leg: Leg::DirectGbp,
                chain_txn_ids: vec![leg.transaction_id.clone()],
            });
            summary.recovered += 1;
            summary.direct_gbp += 1;
            summary.recovered_value += bank_abs;
            continue;
        }
        // No deposit anchor and no direct GBP payment leg — leave unrecovered.
    }

    recoveries.sort_by(|a, b| a.bank_import_id.cmp(&b.bank_import_id));
    (recoveries, summary)
}

/// Find the index of the unconsumed PayPal row nearest in date to `target`
/// (within `window` days) that satisfies `pred`. Ties broken by earliest date
/// then Transaction ID for determinism.
fn nearest_unconsumed<F>(
    paypal: &[PayPalTxn],
    consumed: &HashSet<String>,
    target: NaiveDate,
    window: i64,
    pred: F,
) -> Option<usize>
where
    F: Fn(&PayPalTxn) -> bool,
{
    let mut best: Option<(usize, i64)> = None;
    for (i, p) in paypal.iter().enumerate() {
        if consumed.contains(&p.transaction_id) {
            continue;
        }
        let dd = date_delta(p.date, target);
        if dd > window {
            continue;
        }
        if !pred(p) {
            continue;
        }
        match best {
            Some((bi, bd)) => {
                let better = dd < bd
                    || (dd == bd
                        && (p.date < paypal[bi].date
                            || (p.date == paypal[bi].date
                                && p.transaction_id < paypal[bi].transaction_id)));
                if better {
                    best = Some((i, dd));
                }
            }
            None => best = Some((i, dd)),
        }
    }
    best.map(|(i, _)| i)
}

// ---------------------------------------------------------------------------
// Sidecar I/O: paypal_matches.jsonl
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct OutputRow<'a> {
    bank_import_id: &'a str,
    recovered_merchant: &'a str,
    currency: &'a str,
    merchant_amount: String,
    bank_amount: String,
    confidence: &'a str,
    leg: &'a str,
    chain_txn_ids: &'a [String],
    recovered_at: String,
}

/// Write recoveries to `paypal_matches.jsonl` (one JSON object per line),
/// creating the parent directory if needed. Mirrors `enrich::write_matches`.
pub fn write_recoveries<P: AsRef<Path>>(path: P, recoveries: &[Recovery]) -> std::io::Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = File::create(path.as_ref())?;
    let mut writer = BufWriter::new(file);
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    for r in recoveries {
        let row = OutputRow {
            bank_import_id: &r.bank_import_id,
            recovered_merchant: &r.recovered_merchant,
            currency: &r.currency,
            merchant_amount: r.merchant_amount.to_string(),
            bank_amount: r.bank_amount.to_string(),
            confidence: r.confidence.as_str(),
            leg: r.leg.as_str(),
            chain_txn_ids: &r.chain_txn_ids,
            recovered_at: now.clone(),
        };
        let line = serde_json::to_string(&row)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("JSON: {e}")))?;
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    Ok(())
}

/// One row read back from `paypal_matches.jsonl`.
#[derive(Debug, Clone, Deserialize)]
pub struct RecoveryRow {
    pub bank_import_id: String,
    pub recovered_merchant: String,
    #[serde(default)]
    pub currency: String,
    #[serde(default)]
    pub leg: String,
}

/// Load `paypal_matches.jsonl` into memory (tolerant of bad lines, JSONL
/// convention). Missing file → empty.
pub fn load_recoveries<P: AsRef<Path>>(path: P) -> std::io::Result<Vec<RecoveryRow>> {
    if !path.as_ref().exists() {
        return Ok(Vec::new());
    }
    let file = File::open(path.as_ref())?;
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Ok(row) = serde_json::from_str::<RecoveryRow>(t) {
            out.push(row);
        }
    }
    Ok(out)
}

/// Index of recoveries keyed by `bank_import_id`, for cheap lookup from the
/// query layer and the tag-rule hook. This is the reusable
/// `recovered_merchant_for(bank_import_id)` surface the rule engine consumes.
#[derive(Debug, Clone, Default)]
pub struct RecoveryIndex {
    by_id: std::collections::HashMap<String, RecoveryRow>,
}

impl RecoveryIndex {
    pub fn from_rows(rows: Vec<RecoveryRow>) -> Self {
        let by_id = rows
            .into_iter()
            .map(|r| (r.bank_import_id.clone(), r))
            .collect();
        Self { by_id }
    }

    /// Load from the sidecar path. Missing file → empty index.
    pub fn load<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        Ok(Self::from_rows(load_recoveries(path)?))
    }

    /// The recovered merchant for a bank row, if any.
    pub fn recovered_merchant_for(&self, bank_import_id: &str) -> Option<&str> {
        self.by_id
            .get(bank_import_id)
            .map(|r| r.recovered_merchant.as_str())
    }

    pub fn get(&self, bank_import_id: &str) -> Option<&RecoveryRow> {
        self.by_id.get(bank_import_id)
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paypal::store::parse_paypal_csv;
    use crate::{Account, TxType};
    use std::str::FromStr;

    fn bank(date: &str, amount: &str, id: &str) -> Transaction {
        Transaction {
            date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            account: Account::Current,
            tx_type: TxType::Unknown(0),
            amount: Decimal::from_str(amount).unwrap(),
            description: "PAYPAL PAYMENT".into(),
            raw_description: "PAYPAL PAYMENT".into(),
            balance: None,
            tags: Vec::new(),
            import_id: id.into(),
        }
    }

    const BOM: &str = "\u{feff}";
    const HEADER: &str = "Date,Time,Time zone,Name,Type,Status,Currency,Amount,Fees,Total,Exchange Rate,Receipt ID,Balance,Transaction ID,Item Title";

    fn pp(rows: &str) -> Vec<PayPalTxn> {
        let csv = format!("{BOM}{HEADER}\n{rows}");
        parse_paypal_csv(csv.as_bytes()).unwrap()
    }

    #[test]
    fn direct_gbp_recovers_merchant() {
        let txs = vec![bank("2026-03-05", "-12.99", "bank-1")];
        let paypal = pp(
            "05/03/2026,10:00:00,GMT,Streamflix,Express Checkout Payment,Completed,GBP,-12.99,0,-12.99,,,0,PP-1,plan\n",
        );
        let (recs, summary) = recover(&txs, &paypal, RecoverOptions::default());
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].recovered_merchant, "Streamflix");
        assert_eq!(recs[0].leg, Leg::DirectGbp);
        assert_eq!(recs[0].currency, "GBP");
        assert_eq!(summary.direct_gbp, 1);
        assert_eq!(summary.recovered, 1);
    }

    #[test]
    fn two_leg_recovers_merchant() {
        // Bank -45.00 ↔ deposit +45.00 + payment -45.00 "Acme Shop".
        let txs = vec![bank("2026-04-10", "-45.00", "bank-2")];
        let paypal = pp(
            "10/04/2026,09:00:00,GMT,,Bank Deposit to PP Account ,Completed,GBP,45.00,0,45.00,,,45.00,PP-DEP,\n\
             10/04/2026,09:01:00,GMT,Acme Shop,General Payment,Completed,GBP,-45.00,0,-45.00,,,0,PP-PAY,Thing\n",
        );
        let (recs, summary) = recover(&txs, &paypal, RecoverOptions::default());
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].recovered_merchant, "Acme Shop");
        assert_eq!(recs[0].leg, Leg::TwoLeg);
        assert_eq!(recs[0].chain_txn_ids, vec!["PP-DEP", "PP-PAY"]);
        assert_eq!(summary.two_leg, 1);
    }

    #[test]
    fn fx_chain_recovers_foreign_merchant() {
        // Bank -272.01 → deposit +272.01 → conversion -272.01 GBP → -299.40 EUR.
        let txs = vec![bank("2026-05-10", "-272.01", "bank-3")];
        let paypal = pp(
            "10/05/2026,09:00:00,GMT,,Bank Deposit to PP Account ,Completed,GBP,272.01,0,272.01,,,272.01,PP-DEP,\n\
             10/05/2026,09:01:00,GMT,,General Currency Conversion,Completed,GBP,-272.01,0,-272.01,,,0,PP-CONV,\n\
             10/05/2026,09:02:00,GMT,Acme Foreign GmbH,Express Checkout Payment,Completed,EUR,-299.40,0,-299.40,1.1009,,0,PP-FX,Widget\n",
        );
        let (recs, summary) = recover(&txs, &paypal, RecoverOptions::default());
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].recovered_merchant, "Acme Foreign GmbH");
        assert_eq!(recs[0].leg, Leg::FxChain);
        assert_eq!(recs[0].currency, "EUR");
        assert_eq!(
            recs[0].merchant_amount,
            Decimal::from_str("-299.40").unwrap()
        );
        assert_eq!(recs[0].chain_txn_ids, vec!["PP-DEP", "PP-CONV", "PP-FX"]);
        assert_eq!(summary.fx_chain, 1);
    }

    #[test]
    fn recurring_amount_disambiguated_by_date() {
        // Two -12.99 bank rows in a month; two Streamflix-ish payments.
        // Each bank row should bind to its nearest-dated PayPal leg.
        let txs = vec![
            bank("2026-06-03", "-12.99", "bank-a"),
            bank("2026-06-17", "-12.99", "bank-b"),
        ];
        let paypal = pp(
            "03/06/2026,10:00:00,GMT,Streamflix,General Payment,Completed,GBP,-12.99,0,-12.99,,,0,PP-A,\n\
             17/06/2026,10:00:00,GMT,Newsly,General Payment,Completed,GBP,-12.99,0,-12.99,,,0,PP-B,\n",
        );
        let (recs, _) = recover(&txs, &paypal, RecoverOptions::default());
        assert_eq!(recs.len(), 2);
        let by_id: std::collections::HashMap<_, _> = recs
            .iter()
            .map(|r| (r.bank_import_id.as_str(), r))
            .collect();
        assert_eq!(by_id["bank-a"].recovered_merchant, "Streamflix");
        assert_eq!(by_id["bank-b"].recovered_merchant, "Newsly");
    }

    #[test]
    fn non_paypal_rows_untouched() {
        let txs = vec![bank("2026-03-05", "-12.99", "bank-1"), {
            let mut t = bank("2026-03-05", "-30.00", "bank-x");
            t.raw_description = "TESCO STORES".into();
            t.description = "TESCO STORES".into();
            t
        }];
        let paypal = pp(
            "05/03/2026,10:00:00,GMT,Streamflix,General Payment,Completed,GBP,-12.99,0,-12.99,,,0,PP-1,\n",
        );
        let (recs, summary) = recover(&txs, &paypal, RecoverOptions::default());
        // Only the PayPal row is considered; TESCO is not a bare-paypal row.
        assert_eq!(summary.bare_paypal_rows, 1);
        assert_eq!(recs.len(), 1);
    }

    #[test]
    fn no_double_count_one_leg_one_use() {
        // Two identical -20.00 bank rows but only ONE matching PayPal payment.
        // Greedy consumption means exactly one is recovered; the leg is not
        // double-claimed.
        let txs = vec![
            bank("2026-07-01", "-20.00", "bank-1"),
            bank("2026-07-01", "-20.00", "bank-2"),
        ];
        let paypal = pp(
            "01/07/2026,10:00:00,GMT,Solo Merchant,General Payment,Completed,GBP,-20.00,0,-20.00,,,0,PP-ONLY,\n",
        );
        let (recs, summary) = recover(&txs, &paypal, RecoverOptions::default());
        assert_eq!(recs.len(), 1);
        assert_eq!(summary.recovered, 1);
    }

    #[test]
    fn sidecar_roundtrip_and_index() {
        let txs = vec![bank("2026-03-05", "-12.99", "bank-1")];
        let paypal = pp(
            "05/03/2026,10:00:00,GMT,Streamflix,General Payment,Completed,GBP,-12.99,0,-12.99,,,0,PP-1,\n",
        );
        let (recs, _) = recover(&txs, &paypal, RecoverOptions::default());

        let dir = std::env::temp_dir().join(format!("fd-budget-recovtest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("paypal_matches.jsonl");
        write_recoveries(&path, &recs).unwrap();

        let idx = RecoveryIndex::load(&path).unwrap();
        assert_eq!(idx.recovered_merchant_for("bank-1"), Some("Streamflix"));
        assert_eq!(idx.recovered_merchant_for("nope"), None);
        assert_eq!(idx.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pct_value_recovered() {
        let txs = vec![
            bank("2026-03-05", "-12.99", "bank-1"), // recovered
            bank("2026-03-06", "-87.01", "bank-2"), // not recovered
        ];
        let paypal = pp(
            "05/03/2026,10:00:00,GMT,Streamflix,General Payment,Completed,GBP,-12.99,0,-12.99,,,0,PP-1,\n",
        );
        let (_, summary) = recover(&txs, &paypal, RecoverOptions::default());
        assert_eq!(summary.bare_paypal_rows, 2);
        assert_eq!(summary.recovered, 1);
        // 12.99 of 100.00 = 12.99%
        assert!((summary.pct_value_recovered() - 12.99).abs() < 0.01);
    }
}
