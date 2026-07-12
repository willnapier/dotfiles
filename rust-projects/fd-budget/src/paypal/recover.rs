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
//!
//! ## Binding a chain's legs (the structural link)
//!
//! Day-granular proximity alone mis-binds when two purchases collide (same
//! amount, or two same-day FX chains): the wrong leg — and thus the wrong
//! MERCHANT — gets attached. We retain the linking signals PayPal already
//! provides and bind by the strongest one available, falling back gracefully
//! (time is a TIE-BREAK/orderer, never a hard gate — legs of one event post a
//! few seconds apart, not at an identical timestamp):
//!
//! * **FX foreign leg**: among non-GBP payment legs, prefer the one whose
//!   `amount.abs() * exchange_rate` reconstructs the conversion's GBP amount
//!   (= `bank_abs`) — a true amount-link to THIS chain. Tie-break by timestamp
//!   nearest the conversion. Fall back to pure nearest-time only when the
//!   exchange rate is unavailable.
//! * **Two-leg payment**: among same-amount GBP payment legs, prefer the one
//!   nearest in TIMESTAMP to the deposit — the legs of one checkout are
//!   adjacent in time, so an unrelated same-amount direct purchase nearby is
//!   not mistaken for the true payment leg.

use crate::paypal::store::PayPalTxn;
use crate::Transaction;
use chrono::{NaiveDate, NaiveDateTime, Utc};
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

/// Max seconds between a rate-less foreign leg and the currency conversion for
/// the amount-blind FX fallback to bind them. A real chain's legs post within
/// seconds; beyond this we refuse to guess (a wrong-merchant bind is worse than
/// a miss). Generous vs the same-second reality of real exports.
const FX_FALLBACK_MAX_SECS: i64 = 120;

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
            let dep_dt = paypal[dep_idx].datetime();
            let dep_id = paypal[dep_idx].transaction_id.clone();

            // --- Case 2: Two-leg — deposit → same-amount GBP payment leg. ---
            // Legs of ONE checkout are adjacent in TIME, so we bind to the
            // same-amount GBP payment leg nearest in TIMESTAMP to the deposit.
            // This stops an unrelated same-amount direct-GBP purchase nearby
            // (same day, but seconds/minutes apart) from being mis-picked.
            if let Some(pay_idx) = nearest_unconsumed_by_time(
                paypal,
                &consumed,
                dep_date,
                dep_dt,
                PP_CHAIN_WINDOW_DAYS,
                |p| {
                    p.is_payment_leg()
                        && p.currency.eq_ignore_ascii_case("GBP")
                        && within(p.amount.abs(), bank_abs, GBP_TOLERANCE)
                },
            ) {
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
                let conv_dt = paypal[conv_idx].datetime();
                let conv_id = paypal[conv_idx].transaction_id.clone();
                // Foreign payment leg: a non-GBP payment carrying a merchant.
                // Bind it to THIS chain via the amount link, using the exchange
                // rate carried on the GBP conversion row (`conv_idx`) — PayPal
                // records the rate there, not on the foreign leg. Tie-break by
                // timestamp-nearest-to-the-conversion; falls back to a tight
                // time-window MEDIUM bind when the conversion has no usable rate.
                let conv_rate = paypal[conv_idx].exchange_rate;
                if let Some((fx_idx, confidence)) = nearest_fx_leg(
                    paypal,
                    &consumed,
                    conv_date,
                    conv_dt,
                    conv_rate,
                    bank_abs,
                    PP_CHAIN_WINDOW_DAYS,
                ) {
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
                        confidence,
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

/// Absolute distance in seconds between two timestamps.
fn time_delta_secs(a: NaiveDateTime, b: NaiveDateTime) -> i64 {
    (a - b).num_seconds().abs()
}

/// Find the index of the unconsumed PayPal row nearest in TIMESTAMP to
/// `target_dt` (among rows within `window` days of `target_date`) that
/// satisfies `pred`. Used to bind legs of one checkout, which are adjacent in
/// time. Ties (identical timestamp) broken by Transaction ID for determinism.
///
/// Time is a TIE-BREAK/orderer, not a gate: a row with a blank `Time`
/// (`datetime()` → midnight) still participates, it just orders by date.
fn nearest_unconsumed_by_time<F>(
    paypal: &[PayPalTxn],
    consumed: &HashSet<String>,
    target_date: NaiveDate,
    target_dt: NaiveDateTime,
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
        if date_delta(p.date, target_date) > window {
            continue;
        }
        if !pred(p) {
            continue;
        }
        let td = time_delta_secs(p.datetime(), target_dt);
        match best {
            Some((bi, bd)) => {
                let better = td < bd || (td == bd && p.transaction_id < paypal[bi].transaction_id);
                if better {
                    best = Some((i, td));
                }
            }
            None => best = Some((i, td)),
        }
    }
    best.map(|(i, _)| i)
}

/// A non-GBP payment leg carrying a merchant — the foreign leg of an FX chain.
fn is_foreign_payment_leg(p: &PayPalTxn) -> bool {
    p.is_payment_leg() && !p.currency.eq_ignore_ascii_case("GBP")
}

/// Pick the foreign payment leg of an FX chain, binding it to THIS chain.
///
/// Primary binder: the AMOUNT link, using the exchange rate from the GBP
/// `General Currency Conversion` row (`conv_rate`) — which is where PayPal
/// actually records it. The rate is NOT on the foreign payment leg (that cell is
/// blank in real exports), which is why keying off the leg's own rate never
/// fired. A foreign leg's amount reconstructs the GBP it cost via the rate;
/// PayPal's rate can be GBP-per-foreign OR foreign-per-GBP (real data is the
/// latter: `amount / rate = GBP`), so accept a candidate whose reconstruction by
/// EITHER `amount * rate` or `amount / rate` is within `GBP_TOLERANCE` of
/// `bank_abs`. Among matches pick the one nearest in timestamp to the conversion
/// (tie-break by Transaction ID) — stops two same-second FX chains swapping
/// foreign legs. -> High.
///
/// Fallback: if the conversion carries no usable rate, or nothing links by
/// amount, bind only a foreign leg posting within a TIGHT time window of the
/// conversion (a real chain's legs post within seconds) as MEDIUM — never an
/// amount-blind "high" bind of an unrelated payment.
fn nearest_fx_leg(
    paypal: &[PayPalTxn],
    consumed: &HashSet<String>,
    conv_date: NaiveDate,
    conv_dt: NaiveDateTime,
    conv_rate: Option<Decimal>,
    bank_abs: Decimal,
    window: i64,
) -> Option<(usize, RecoveryConfidence)> {
    // Pass 1: amount-link via the conversion row's exchange rate -> High.
    if let Some(rate) = conv_rate {
        if !rate.is_zero() {
            let mut best: Option<(usize, i64)> = None;
            for (i, p) in paypal.iter().enumerate() {
                if consumed.contains(&p.transaction_id) {
                    continue;
                }
                if date_delta(p.date, conv_date) > window {
                    continue;
                }
                if !is_foreign_payment_leg(p) {
                    continue;
                }
                let amt = p.amount.abs();
                let links = within(amt * rate, bank_abs, GBP_TOLERANCE)
                    || within(amt / rate, bank_abs, GBP_TOLERANCE);
                if !links {
                    continue;
                }
                let td = time_delta_secs(p.datetime(), conv_dt);
                match best {
                    Some((bi, bd)) => {
                        let better =
                            td < bd || (td == bd && p.transaction_id < paypal[bi].transaction_id);
                        if better {
                            best = Some((i, td));
                        }
                    }
                    None => best = Some((i, td)),
                }
            }
            if let Some((i, _)) = best {
                return Some((i, RecoveryConfidence::High));
            }
        }
    }

    // Pass 2 (fallback): the conversion carried no usable rate, or nothing
    // linked by amount. We cannot reconstruct GBP to check the amount, so bind
    // ONLY a foreign leg that posts within a TIGHT time window of the conversion
    // — a real chain's legs post within seconds — and record it as MEDIUM, never
    // High. This refuses to attribute an unrelated payment days away, which the
    // old amount-blind nearest-within-window bind did (and mislabelled "high").
    let idx = nearest_unconsumed_by_time(
        paypal,
        consumed,
        conv_date,
        conv_dt,
        window,
        is_foreign_payment_leg,
    )?;
    if time_delta_secs(paypal[idx].datetime(), conv_dt) > FX_FALLBACK_MAX_SECS {
        return None;
    }
    Some((idx, RecoveryConfidence::Medium))
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
        // REAL PayPal structure: the exchange rate is on the GBP conversion row,
        // NOT the foreign payment leg (that cell is blank in real exports).
        // bank -197.62 -> deposit +197.62 -> GBP conversion -197.62 @
        // 1.265008669165684 -> USD payment -249.99
        // (249.99 / 1.265008669165684 = 197.62). The amount-link fires -> High.
        let txs = vec![bank("2026-05-10", "-197.62", "bank-3")];
        let paypal = pp(
            "10/05/2026,09:00:00,GMT,,Bank Deposit to PP Account ,Completed,GBP,197.62,0,197.62,,,197.62,PP-DEP,\n\
             10/05/2026,09:01:00,GMT,,General Currency Conversion,Completed,GBP,-197.62,0,-197.62,1.265008669165684,,0,PP-CONV,\n\
             10/05/2026,09:02:00,GMT,Acme Foreign Inc,Pre-approved Payment Bill User Payment,Completed,USD,-249.99,0,-249.99,,,0,PP-FX,Widget\n",
        );
        let (recs, summary) = recover(&txs, &paypal, RecoverOptions::default());
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].recovered_merchant, "Acme Foreign Inc");
        assert_eq!(recs[0].leg, Leg::FxChain);
        assert_eq!(recs[0].currency, "USD");
        assert_eq!(recs[0].merchant_amount, Decimal::from_str("-249.99").unwrap());
        // Amount-linked via the conversion row's rate -> High (not the fallback).
        assert_eq!(recs[0].confidence.as_str(), "high");
        assert_eq!(recs[0].chain_txn_ids, vec!["PP-DEP", "PP-CONV", "PP-FX"]);
        assert_eq!(summary.fx_chain, 1);
    }

    #[test]
    fn rate_less_far_time_fx_leg_is_not_bound() {
        // No exchange rate on the foreign leg (the common real-export case), and
        // the only foreign leg posts HOURS from the conversion — an unrelated
        // payment. The amount-blind fallback must REFUSE to bind it (H5: no
        // wrong-merchant attribution), leaving the bank row unrecovered.
        let txs = vec![bank("2026-05-10", "-272.01", "bank-x")];
        let paypal = pp(
            "10/05/2026,09:00:00,GMT,,Bank Deposit to PP Account ,Completed,GBP,272.01,0,272.01,,,272.01,PP-DEP,\n\
             10/05/2026,09:01:00,GMT,,General Currency Conversion,Completed,GBP,-272.01,0,-272.01,,,0,PP-CONV,\n\
             10/05/2026,15:00:00,GMT,Unrelated Merchant,Express Checkout Payment,Completed,USD,-5.00,0,-5.00,,,0,PP-FX,Thing\n",
        );
        let (_recs, summary) = recover(&txs, &paypal, RecoverOptions::default());
        assert_eq!(summary.fx_chain, 0, "far-time rate-less leg must not bind");
    }

    #[test]
    fn rate_less_close_time_fx_leg_binds_as_medium() {
        // Same, but the foreign leg posts within seconds of the conversion — a
        // real chain. Bind it, but as MEDIUM (amount-blind), never High.
        let txs = vec![bank("2026-05-10", "-272.01", "bank-y")];
        let paypal = pp(
            "10/05/2026,09:00:00,GMT,,Bank Deposit to PP Account ,Completed,GBP,272.01,0,272.01,,,272.01,PP-DEP,\n\
             10/05/2026,09:01:00,GMT,,General Currency Conversion,Completed,GBP,-272.01,0,-272.01,,,0,PP-CONV,\n\
             10/05/2026,09:01:05,GMT,Acme Foreign GmbH,Express Checkout Payment,Completed,EUR,-299.40,0,-299.40,,,0,PP-FX,Widget\n",
        );
        let (recs, summary) = recover(&txs, &paypal, RecoverOptions::default());
        assert_eq!(summary.fx_chain, 1);
        assert_eq!(recs[0].recovered_merchant, "Acme Foreign GmbH");
        assert_eq!(recs[0].confidence.as_str(), "medium");
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

    /// REGRESSION (HIGH — FX foreign-leg cross-link, recover.rs:266-269 pre-fix).
    ///
    /// Two FX chains on the SAME DAY, distinct amounts/merchants. The old code
    /// picked the foreign leg as `is_payment_leg() && non-GBP` nearest by DAY to
    /// the conversion, tie-broken by Transaction ID — so when both foreign legs
    /// are same-day, the first-processed bank row grabbed whichever leg had the
    /// lower Transaction ID, regardless of which chain it belonged to. Here the
    /// foreign-leg ids are ordered so the OLD code swaps them (bank-1 would have
    /// grabbed "RIGHT-B" because PP-FX-B < PP-FX-A). The amount link
    /// (`foreign_amount.abs() * exchange_rate ≈ bank_abs`) now binds each leg to
    /// its true chain.
    #[test]
    fn two_same_day_fx_chains_do_not_swap() {
        let txs = vec![
            bank("2026-05-10", "-100.00", "bank-1"),
            bank("2026-05-10", "-200.00", "bank-2"),
        ];
        // EUR leg: 110 * 0.909091 ≈ 100.00 (links to the -100 chain).
        // USD leg: 260 * 0.769231 ≈ 200.00 (links to the -200 chain).
        // Foreign-leg ids chosen so the OLD nearest-day + id tie-break SWAPS:
        // both foreign legs are same-day, so the old code broke the tie by
        // Transaction ID — and RIGHT-B's leg (PP-FX-1) sorts BEFORE RIGHT-A's
        // leg (PP-FX-2), so bank-1 (processed first) wrongly grabbed RIGHT-B.
        // The amount link now binds each leg to its own chain regardless of id.
        let paypal = pp(
            "10/05/2026,09:00:00,GMT,,Bank Deposit to PP Account ,Completed,GBP,100.00,0,100.00,,,100.00,PP-DEP-A,\n\
             10/05/2026,09:00:01,GMT,,General Currency Conversion,Completed,GBP,-100.00,0,-100.00,,,0,PP-CONV-A,\n\
             10/05/2026,09:00:02,GMT,RIGHT-A,Express Checkout Payment,Completed,EUR,-110.00,0,-110.00,0.909091,,0,PP-FX-2,Widget\n\
             10/05/2026,11:00:00,GMT,,Bank Deposit to PP Account ,Completed,GBP,200.00,0,200.00,,,200.00,PP-DEP-B,\n\
             10/05/2026,11:00:01,GMT,,General Currency Conversion,Completed,GBP,-200.00,0,-200.00,,,0,PP-CONV-B,\n\
             10/05/2026,11:00:02,GMT,RIGHT-B,Express Checkout Payment,Completed,USD,-260.00,0,-260.00,0.769231,,0,PP-FX-1,Gadget\n",
        );
        let (recs, summary) = recover(&txs, &paypal, RecoverOptions::default());
        assert_eq!(summary.fx_chain, 2);
        assert_eq!(summary.recovered, 2);
        let by_id: std::collections::HashMap<_, _> = recs
            .iter()
            .map(|r| (r.bank_import_id.as_str(), r))
            .collect();
        // The amount link binds each foreign leg to its OWN chain — no swap.
        assert_eq!(by_id["bank-1"].recovered_merchant, "RIGHT-A");
        assert_eq!(by_id["bank-1"].currency, "EUR");
        assert_eq!(by_id["bank-1"].leg, Leg::FxChain);
        assert_eq!(by_id["bank-2"].recovered_merchant, "RIGHT-B");
        assert_eq!(by_id["bank-2"].currency, "USD");
        assert_eq!(by_id["bank-2"].leg, Leg::FxChain);
    }

    /// REGRESSION (MEDIUM-HIGH — two-leg same-amount mis-pick, recover.rs:226-231
    /// pre-fix).
    ///
    /// A true two-leg purchase (deposit +45 + payment -45 "RealMerchant") sits
    /// near an UNRELATED same-amount direct-GBP purchase (-45 "OtherMerchant").
    /// The old code picked the deposit's payment leg by same-amount + nearest
    /// DAY + id tie-break, which could grab the unrelated direct purchase. Legs
    /// of one checkout are adjacent in TIME, so we now bind the payment leg
    /// nearest in TIMESTAMP to the deposit, recovering "RealMerchant".
    #[test]
    fn two_leg_prefers_timestamp_adjacent_leg_over_unrelated_same_amount() {
        let txs = vec![bank("2026-04-10", "-45.00", "bank-1")];
        // Deposit at 14:00:00; the TRUE payment leg posts a second later at
        // 14:00:01. The unrelated direct -45 "OtherMerchant" is hours away at
        // 09:00:00 (same day) — its id (PP-OTHER) would have sorted before
        // PP-PAY under the old nearest-day + id tie-break, mis-binding it.
        let paypal = pp(
            "10/04/2026,09:00:00,GMT,OtherMerchant,General Payment,Completed,GBP,-45.00,0,-45.00,,,0,PP-OTHER,Unrelated\n\
             10/04/2026,14:00:00,GMT,,Bank Deposit to PP Account ,Completed,GBP,45.00,0,45.00,,,45.00,PP-DEP,\n\
             10/04/2026,14:00:01,GMT,RealMerchant,General Payment,Completed,GBP,-45.00,0,-45.00,,,0,PP-PAY,Thing\n",
        );
        let (recs, summary) = recover(&txs, &paypal, RecoverOptions::default());
        assert_eq!(recs.len(), 1);
        assert_eq!(summary.two_leg, 1);
        assert_eq!(recs[0].recovered_merchant, "RealMerchant");
        assert_eq!(recs[0].leg, Leg::TwoLeg);
        assert_eq!(recs[0].chain_txn_ids, vec!["PP-DEP", "PP-PAY"]);
        // The unrelated direct purchase was NOT consumed by this bank row.
        assert!(!recs[0].chain_txn_ids.contains(&"PP-OTHER".to_string()));
    }
}
