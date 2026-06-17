//! PayPal CSV import + the typed sidecar store (`paypal.csv`).
//!
//! First Direct's 4-column current-account export strips the PayPal merchant:
//! every PayPal purchase lands as a bare `PAYPAL PAYMENT  -£X` with no merchant
//! name. PayPal's own CSV export DOES carry the merchant. This module reads that
//! export into a typed [`PayPalTxn`] and persists it to a **sidecar** CSV
//! (`~/.config/fd-budget/paypal.csv`) — NEVER into `transactions.csv`. The join
//! that recovers merchants lives in [`crate::paypal::recover`].
//!
//! Import is idempotent by PayPal's `Transaction ID`, exactly as the FD importer
//! dedups by `import_id`: re-importing overlapping date-range exports adds no
//! duplicate rows.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use csv::ReaderBuilder;
use rust_decimal::Decimal;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read};
use std::path::Path;
use std::str::FromStr;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PayPalError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),
    #[error("unexpected PayPal CSV header: {0}")]
    BadHeader(String),
}

/// A single normalised row from a PayPal activity CSV export.
///
/// `amount` is **signed** (negative = money leaving the PayPal balance, e.g. a
/// payment to a merchant or an outbound currency conversion; positive = money
/// arriving, e.g. a `Bank Deposit to PP Account`). `currency` may be non-GBP
/// (EUR/USD) for the foreign payment leg of an FX chain.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PayPalTxn {
    pub date: NaiveDate,
    /// PayPal `Time` column (col index 1). The legs of one purchase are posted
    /// within seconds of each other, so date+time is the strongest STRUCTURAL
    /// link binding a chain's legs together. `None` when the export leaves it
    /// blank (older exports / summary rows).
    pub time: Option<NaiveTime>,
    /// Merchant name — the value we want to recover. BLANK for some system rows
    /// (deposits, currency conversions).
    pub name: String,
    /// PayPal `Type`, trimmed (the real export has a trailing space on some,
    /// e.g. `"Bank Deposit to PP Account "`).
    pub txn_type: String,
    pub status: String,
    pub currency: String,
    pub amount: Decimal,
    /// PayPal `Exchange Rate` column (col index 10). Populated only on the
    /// foreign payment leg of an FX chain (e.g. `1.1009`); blank/`""` → `None`.
    /// `amount.abs() * exchange_rate` reconstructs the GBP value of THIS chain,
    /// giving a true amount-link to its conversion leg.
    pub exchange_rate: Option<Decimal>,
    pub balance: Option<Decimal>,
    /// PayPal `Transaction ID` — unique key, used for idempotent import.
    pub transaction_id: String,
    pub item_title: String,
}

impl PayPalTxn {
    pub fn is_debit(&self) -> bool {
        self.amount.is_sign_negative()
    }

    /// Date+time as a single ordering key. Used as a TIE-BREAK / orderer when
    /// binding a chain's legs (legs of one checkout are adjacent in time). When
    /// the `Time` column is blank we fall back to midnight, so date-only rows
    /// still order deterministically by date.
    pub fn datetime(&self) -> NaiveDateTime {
        self.date.and_time(self.time.unwrap_or(NaiveTime::MIN))
    }

    pub fn is_credit(&self) -> bool {
        self.amount.is_sign_positive()
    }

    /// A leg that carries a merchant name (the thing we recover): a negative
    /// payment with a non-empty `name`. Deposits and conversions are excluded —
    /// they have blank names and are plumbing.
    pub fn is_payment_leg(&self) -> bool {
        self.is_debit() && !self.name.trim().is_empty() && !self.is_currency_conversion()
    }

    /// `Bank Deposit to PP Account` — funds a purchase; its amount equals the
    /// bank `PAYPAL PAYMENT` debit.
    pub fn is_deposit(&self) -> bool {
        let t = self.txn_type.to_lowercase();
        self.is_credit() && t.contains("deposit")
    }

    /// `General Currency Conversion` — the FX plumbing leg.
    pub fn is_currency_conversion(&self) -> bool {
        self.txn_type.to_lowercase().contains("currency conversion")
    }
}

/// Strip a leading UTF-8 BOM (`EF BB BF`) if present. PayPal exports as
/// `utf-8-sig`; the BOM would otherwise contaminate the first header cell.
fn strip_bom(bytes: Vec<u8>) -> Vec<u8> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        bytes[3..].to_vec()
    } else {
        bytes
    }
}

/// Indices of the columns we read, resolved by header name.
struct ColumnMap {
    date: usize,
    time: Option<usize>,
    name: usize,
    txn_type: usize,
    status: usize,
    currency: usize,
    amount: usize,
    exchange_rate: Option<usize>,
    balance: Option<usize>,
    transaction_id: usize,
    item_title: Option<usize>,
}

/// Map the PayPal header row to our field indices, by name (case-insensitive,
/// whitespace-trimmed). Returns indices for the columns we read.
fn map_columns(headers: &csv::StringRecord) -> Result<ColumnMap, PayPalError> {
    let find = |name: &str| -> Option<usize> {
        headers
            .iter()
            .position(|h| h.trim().eq_ignore_ascii_case(name))
    };
    let required = |name: &str| -> Result<usize, PayPalError> {
        find(name)
            .ok_or_else(|| PayPalError::BadHeader(format!("missing required column '{name}'")))
    };
    Ok(ColumnMap {
        date: required("Date")?,
        time: find("Time"),
        name: required("Name")?,
        txn_type: required("Type")?,
        status: required("Status")?,
        currency: required("Currency")?,
        amount: required("Amount")?,
        exchange_rate: find("Exchange Rate"),
        balance: find("Balance"),
        transaction_id: required("Transaction ID")?,
        item_title: find("Item Title"),
    })
}

/// Parse a PayPal `Amount`-style cell: strips thousands commas and any stray
/// currency symbol, keeps the sign. PayPal amounts are like `-12.99`,
/// `1,234.56`, `-299.40`.
fn parse_pp_amount(s: &str) -> Option<Decimal> {
    let cleaned: String = s
        .trim()
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '-' || *c == '.')
        .collect();
    if cleaned.is_empty() || cleaned == "-" {
        return None;
    }
    Decimal::from_str(&cleaned).ok()
}

/// Parse a PayPal `Time` cell. PayPal exports `HH:MM:SS`; some locales omit the
/// seconds (`HH:MM`). Blank → `None` (tolerated, not an error).
fn parse_pp_time(s: &str) -> Option<NaiveTime> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    NaiveTime::parse_from_str(s, "%H:%M:%S")
        .or_else(|_| NaiveTime::parse_from_str(s, "%H:%M"))
        .ok()
}

/// Parse a PayPal activity CSV export (UTF-8-with-BOM, 15 quoted columns) into
/// typed rows. Rows that don't parse a date are skipped (defensive — PayPal
/// occasionally appends summary rows). Header-driven, so column order is
/// tolerated.
pub fn parse_paypal_csv<R: Read>(mut reader: R) -> Result<Vec<PayPalTxn>, PayPalError> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    let bytes = strip_bom(bytes);

    let mut csv_reader = ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(&bytes[..]);

    let header = csv_reader.headers()?.clone();
    let cols = map_columns(&header)?;

    let mut out = Vec::new();
    for result in csv_reader.records() {
        let record = match result {
            Ok(r) => r,
            Err(_) => continue,
        };
        let get = |idx: usize| record.get(idx).unwrap_or("").trim();

        let date = match NaiveDate::parse_from_str(get(cols.date), "%d/%m/%Y") {
            Ok(d) => d,
            Err(_) => continue, // skip un-dated / summary rows
        };
        let amount = match parse_pp_amount(get(cols.amount)) {
            Some(a) => a,
            None => continue,
        };
        let transaction_id = get(cols.transaction_id).to_string();
        if transaction_id.is_empty() {
            continue; // no idempotency key — skip
        }

        out.push(PayPalTxn {
            date,
            time: cols.time.and_then(|i| parse_pp_time(get(i))),
            name: get(cols.name).to_string(),
            txn_type: get(cols.txn_type).to_string(),
            status: get(cols.status).to_string(),
            currency: get(cols.currency).to_string(),
            amount,
            exchange_rate: cols.exchange_rate.and_then(|i| parse_pp_amount(get(i))),
            balance: cols.balance.and_then(|i| parse_pp_amount(get(i))),
            transaction_id,
            item_title: cols
                .item_title
                .map(|i| get(i).to_string())
                .unwrap_or_default(),
        });
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// Sidecar store (paypal.csv) — typed, idempotent by Transaction ID.
// ---------------------------------------------------------------------------

/// Our canonical sidecar CSV header for stored PayPal rows.
const STORE_HEADERS: &[&str] = &[
    "date",
    "time",
    "name",
    "txn_type",
    "status",
    "currency",
    "amount",
    "exchange_rate",
    "balance",
    "transaction_id",
    "item_title",
];

/// Serialisable shape for the sidecar CSV.
///
/// `time` and `exchange_rate` are `#[serde(default)]` so a pre-existing sidecar
/// written before these columns existed still deserialises (the missing cells
/// read back as empty strings → `None`).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct StoredPayPal {
    date: String,
    #[serde(default)]
    time: String,
    name: String,
    txn_type: String,
    status: String,
    currency: String,
    amount: String,
    #[serde(default)]
    exchange_rate: String,
    balance: String,
    transaction_id: String,
    item_title: String,
}

impl From<&PayPalTxn> for StoredPayPal {
    fn from(t: &PayPalTxn) -> Self {
        Self {
            date: t.date.to_string(),
            time: t.time.map(|t| t.to_string()).unwrap_or_default(),
            name: t.name.clone(),
            txn_type: t.txn_type.clone(),
            status: t.status.clone(),
            currency: t.currency.clone(),
            amount: t.amount.to_string(),
            exchange_rate: t.exchange_rate.map(|r| r.to_string()).unwrap_or_default(),
            balance: t.balance.map(|b| b.to_string()).unwrap_or_default(),
            transaction_id: t.transaction_id.clone(),
            item_title: t.item_title.clone(),
        }
    }
}

impl From<StoredPayPal> for PayPalTxn {
    fn from(s: StoredPayPal) -> Self {
        Self {
            date: NaiveDate::parse_from_str(&s.date, "%Y-%m-%d")
                .unwrap_or_else(|_| NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()),
            time: parse_pp_time(&s.time),
            name: s.name,
            txn_type: s.txn_type,
            status: s.status,
            currency: s.currency,
            amount: Decimal::from_str(&s.amount).unwrap_or_default(),
            exchange_rate: Decimal::from_str(s.exchange_rate.trim()).ok(),
            balance: Decimal::from_str(&s.balance).ok(),
            transaction_id: s.transaction_id,
            item_title: s.item_title,
        }
    }
}

/// The PayPal sidecar store at `~/.config/fd-budget/paypal.csv`.
pub struct PayPalStore {
    path: std::path::PathBuf,
}

impl PayPalStore {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Load all stored PayPal rows.
    pub fn load_all(&self) -> Result<Vec<PayPalTxn>, PayPalError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&self.path)?;
        let reader = BufReader::new(file);
        let mut csv_reader = csv::Reader::from_reader(reader);
        let mut out = Vec::new();
        for result in csv_reader.deserialize() {
            let stored: StoredPayPal = result?;
            out.push(stored.into());
        }
        Ok(out)
    }

    /// Load the set of stored `Transaction ID`s for idempotent import.
    pub fn load_transaction_ids(&self) -> Result<HashSet<String>, PayPalError> {
        Ok(self
            .load_all()?
            .into_iter()
            .map(|t| t.transaction_id)
            .collect())
    }

    /// Append new rows (writes the header if the file is new). Returns the count
    /// written.
    pub fn append(&self, rows: &[PayPalTxn]) -> Result<usize, PayPalError> {
        if rows.is_empty() {
            return Ok(0);
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file_exists = self.path.exists();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let writer = BufWriter::new(file);
        let mut csv_writer = csv::WriterBuilder::new()
            .has_headers(false)
            .from_writer(writer);
        if !file_exists {
            csv_writer.write_record(STORE_HEADERS)?;
        }
        for row in rows {
            let stored: StoredPayPal = row.into();
            csv_writer.serialize(&stored)?;
        }
        csv_writer.flush()?;
        Ok(rows.len())
    }
}

/// Drop rows whose `Transaction ID` already exists in `existing_ids`.
/// Also de-dups WITHIN the incoming batch (overlapping export files may both
/// contain the same row).
pub fn deduplicate(rows: Vec<PayPalTxn>, existing_ids: &HashSet<String>) -> Vec<PayPalTxn> {
    let mut seen = existing_ids.clone();
    let mut out = Vec::new();
    for row in rows {
        if seen.insert(row.transaction_id.clone()) {
            out.push(row);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic fixtures ONLY — fictional merchants, round amounts.
    // 15-column UTF-8-with-BOM PayPal export (header exactly as the real one).
    const BOM: &str = "\u{feff}";

    fn sample_csv() -> String {
        format!(
            "{BOM}Date,Time,Time zone,Name,Type,Status,Currency,Amount,Fees,Total,Exchange Rate,Receipt ID,Balance,Transaction ID,Item Title\n\
             05/03/2026,10:00:00,GMT,Streamflix,Express Checkout Payment,Completed,GBP,-12.99,0.00,-12.99,,,100.00,TXN-DIRECT-1,Monthly plan\n\
             10/03/2026,09:00:00,GMT,,Bank Deposit to PP Account ,Completed,GBP,272.01,0.00,272.01,,,272.01,TXN-DEP-1,\n\
             10/03/2026,09:01:00,GMT,,General Currency Conversion,Completed,GBP,-272.01,0.00,-272.01,,,0.00,TXN-CONV-1,\n\
             10/03/2026,09:02:00,GMT,Acme Foreign GmbH,Express Checkout Payment,Completed,EUR,-299.40,0.00,-299.40,1.1009,,0.00,TXN-FX-1,Widget\n"
        )
    }

    #[test]
    fn parses_bom_and_columns_by_name() {
        let rows = parse_paypal_csv(sample_csv().as_bytes()).unwrap();
        assert_eq!(rows.len(), 4);
        // BOM did not contaminate the Date column.
        assert_eq!(rows[0].date, NaiveDate::from_ymd_opt(2026, 3, 5).unwrap());
        assert_eq!(rows[0].name, "Streamflix");
        assert_eq!(rows[0].amount, Decimal::from_str("-12.99").unwrap());
        assert_eq!(rows[0].currency, "GBP");
    }

    #[test]
    fn parses_time_and_exchange_rate_columns() {
        let rows = parse_paypal_csv(sample_csv().as_bytes()).unwrap();
        // Direct GBP payment: time set, no exchange rate, datetime = date+time.
        assert_eq!(
            rows[0].time,
            Some(NaiveTime::from_hms_opt(10, 0, 0).unwrap())
        );
        assert_eq!(rows[0].exchange_rate, None);
        assert_eq!(
            rows[0].datetime(),
            NaiveDate::from_ymd_opt(2026, 3, 5)
                .unwrap()
                .and_hms_opt(10, 0, 0)
                .unwrap()
        );
        // FX foreign leg (row 3): carries the exchange rate that links it to its
        // conversion (amount.abs() * rate ≈ GBP value).
        assert_eq!(rows[3].currency, "EUR");
        assert_eq!(
            rows[3].exchange_rate,
            Some(Decimal::from_str("1.1009").unwrap())
        );
        assert_eq!(
            rows[3].time,
            Some(NaiveTime::from_hms_opt(9, 2, 0).unwrap())
        );
    }

    #[test]
    fn blank_time_and_exchange_rate_tolerated() {
        // A row with empty Time and empty Exchange Rate must parse, not skip.
        let csv = format!(
            "{BOM}Date,Time,Time zone,Name,Type,Status,Currency,Amount,Fees,Total,Exchange Rate,Receipt ID,Balance,Transaction ID,Item Title\n\
             05/03/2026,,GMT,Streamflix,General Payment,Completed,GBP,-12.99,0.00,-12.99,,,0.00,TXN-NOTIME,Plan\n"
        );
        let rows = parse_paypal_csv(csv.as_bytes()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].time, None);
        assert_eq!(rows[0].exchange_rate, None);
        // datetime() falls back to midnight when time is blank.
        assert_eq!(
            rows[0].datetime(),
            NaiveDate::from_ymd_opt(2026, 3, 5)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
        );
    }

    #[test]
    fn classifies_leg_types() {
        let rows = parse_paypal_csv(sample_csv().as_bytes()).unwrap();
        let payment = &rows[0];
        assert!(payment.is_payment_leg());
        assert!(!payment.is_deposit());

        let deposit = &rows[1];
        assert!(deposit.is_deposit());
        assert!(!deposit.is_payment_leg());

        let conversion = &rows[2];
        assert!(conversion.is_currency_conversion());
        assert!(!conversion.is_payment_leg());

        let fx_payment = &rows[3];
        assert!(fx_payment.is_payment_leg());
        assert_eq!(fx_payment.currency, "EUR");
        assert_eq!(fx_payment.name, "Acme Foreign GmbH");
    }

    #[test]
    fn deduplicates_by_transaction_id() {
        let rows = parse_paypal_csv(sample_csv().as_bytes()).unwrap();
        let mut existing = HashSet::new();
        existing.insert("TXN-DIRECT-1".to_string());
        let deduped = deduplicate(rows, &existing);
        // The direct payment is dropped (already imported); 3 remain.
        assert_eq!(deduped.len(), 3);
        assert!(deduped.iter().all(|r| r.transaction_id != "TXN-DIRECT-1"));
    }

    #[test]
    fn deduplicates_within_batch() {
        // Two overlapping exports both carry TXN-DIRECT-1.
        let doubled = format!(
            "{}{}",
            sample_csv(),
            "05/03/2026,10:00:00,GMT,Streamflix,Express Checkout Payment,Completed,GBP,-12.99,0.00,-12.99,,,100.00,TXN-DIRECT-1,Monthly plan\n"
        );
        let rows = parse_paypal_csv(doubled.as_bytes()).unwrap();
        assert_eq!(rows.len(), 5); // raw parse keeps both
        let deduped = deduplicate(rows, &HashSet::new());
        assert_eq!(deduped.len(), 4); // dedup collapses to one
    }

    #[test]
    fn parse_amount_strips_commas() {
        assert_eq!(
            parse_pp_amount("1,234.56"),
            Some(Decimal::from_str("1234.56").unwrap())
        );
        assert_eq!(
            parse_pp_amount("-299.40"),
            Some(Decimal::from_str("-299.40").unwrap())
        );
        assert_eq!(parse_pp_amount(""), None);
    }

    #[test]
    fn store_roundtrip_idempotent() {
        let dir = std::env::temp_dir().join(format!("fd-budget-pptest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("paypal.csv");
        let store = PayPalStore::new(&path);

        let rows = parse_paypal_csv(sample_csv().as_bytes()).unwrap();
        let existing = store.load_transaction_ids().unwrap();
        let fresh = deduplicate(rows.clone(), &existing);
        assert_eq!(store.append(&fresh).unwrap(), 4);

        // Re-import the same file: nothing new.
        let existing2 = store.load_transaction_ids().unwrap();
        let fresh2 = deduplicate(rows, &existing2);
        assert_eq!(fresh2.len(), 0);

        // Round-trips intact — including the new time / exchange_rate columns.
        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 4);
        assert_eq!(loaded[0].name, "Streamflix");
        assert_eq!(loaded[3].currency, "EUR");
        assert_eq!(
            loaded[0].time,
            Some(NaiveTime::from_hms_opt(10, 0, 0).unwrap())
        );
        assert_eq!(loaded[0].exchange_rate, None);
        assert_eq!(
            loaded[3].exchange_rate,
            Some(Decimal::from_str("1.1009").unwrap())
        );
        assert_eq!(
            loaded[3].time,
            Some(NaiveTime::from_hms_opt(9, 2, 0).unwrap())
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
