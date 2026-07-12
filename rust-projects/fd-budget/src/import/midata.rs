use crate::{Account, Transaction, TxType};
use crate::dedup::{compute_import_id, occurrence_key};
use crate::import::normalize::{clean_description, parse_amount};
use chrono::NaiveDate;
use csv::ReaderBuilder;
use std::collections::HashMap;
use std::io::Read;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ParseError {
    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),
    #[error("Invalid date '{0}': {1}")]
    InvalidDate(String, String),
    #[error("Invalid amount '{0}': {1}")]
    InvalidAmount(String, String),
    #[error("Missing field: {0}")]
    MissingField(String),
}


/// Parse a midata CSV file from First Direct
/// Stops parsing when it hits footer rows (empty lines, overdraft info, etc.)
pub fn parse_midata<R: Read>(
    reader: R,
    account: Account,
) -> Result<Vec<Transaction>, ParseError> {
    let mut csv_reader = ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(reader);

    let mut transactions = Vec::new();
    // Count identical (date, 2dp-amount, description) rows in file order so each
    // gets a distinct occurrence index in its dedup id (see compute_import_id).
    let mut occ_counts: HashMap<String, usize> = HashMap::new();

    for (i, result) in csv_reader.records().enumerate() {
        let record = match result {
            Ok(r) => r,
            Err(_) => break, // malformed CSV structure => end of data / footer
        };

        // Footer begins at the first blank line.
        if record.get(0).map(|s| s.trim().is_empty()).unwrap_or(true) {
            break;
        }

        let date_str = record.get(0).unwrap_or("");
        // The date is the footer discriminator: summary / overdraft footer rows
        // don't parse as a date. Once the first field IS a date this row is a
        // genuine transaction, so a failure on its OTHER fields is corruption,
        // NOT the footer — fail closed with the row number instead of silently
        // truncating the rest of the import (which used to print "success").
        let date = match NaiveDate::parse_from_str(date_str, "%d/%m/%Y") {
            Ok(d) => d,
            Err(_) => break, // not a date => footer section
        };

        if record.len() < 5 {
            return Err(ParseError::MissingField(format!(
                "row {} (date {date_str}): expected 5 columns, got {}",
                i + 2,
                record.len()
            )));
        }

        let tx_type_str = record.get(1).unwrap_or("");
        let description_str = record.get(2).unwrap_or("");
        let amount_str = record.get(3).unwrap_or("");
        let balance_str = record.get(4).unwrap_or("");

        let tx_type = TxType::from_code(tx_type_str);

        let amount = parse_amount(amount_str).map_err(|_| {
            ParseError::InvalidAmount(amount_str.to_string(), format!("row {}", i + 2))
        })?;

        // Parse balance (optional, might fail on some rows)
        let balance = parse_amount(balance_str).ok();

        // Clean description
        let description = clean_description(description_str);
        let raw_description = description_str.to_string();

        // Compute dedup ID
        let occurrence = *occ_counts
            .entry(occurrence_key(&date, &amount, &raw_description))
            .and_modify(|c| *c += 1)
            .or_insert(0);
        let import_id = compute_import_id(&date, &amount, &raw_description, occurrence);

        transactions.push(Transaction {
            date,
            account,
            tx_type,
            amount,
            description,
            raw_description,
            balance,
            tags: Vec::new(),
            import_id,
        });
    }

    Ok(transactions)
}

/// Parse the FD Visa-card CSV format (4 columns: Date, Description,
/// Amount, Reference). Distinct from `parse_midata` because the Visa
/// account export uses a different schema — no `Type` column, no
/// running balance — than the current account's midata.
///
/// Filename convention also differs: current account is
/// `MIDATA_<accountid>.csv`; Visa is `<DDMMYYYY>_<accountid>.csv`.
pub fn parse_midata_visa<R: Read>(
    reader: R,
    account: Account,
) -> Result<Vec<Transaction>, ParseError> {
    let mut csv_reader = ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(reader);

    let mut transactions = Vec::new();
    // Count identical (date, 2dp-amount, description) rows in file order so each
    // gets a distinct occurrence index in its dedup id (see compute_import_id).
    let mut occ_counts: HashMap<String, usize> = HashMap::new();

    for (i, result) in csv_reader.records().enumerate() {
        let record = match result {
            Ok(r) => r,
            Err(_) => break, // malformed CSV structure => end of data / footer
        };

        // Footer begins at the first blank line.
        if record.get(0).map(|s| s.trim().is_empty()).unwrap_or(true) {
            break;
        }

        let date_str = record.get(0).unwrap_or("");
        // Date is the footer discriminator; a dated row with bad fields is
        // corruption, not footer — fail closed rather than silently truncate.
        let date = match NaiveDate::parse_from_str(date_str, "%d/%m/%Y") {
            Ok(d) => d,
            Err(_) => break,
        };

        if record.len() < 3 {
            return Err(ParseError::MissingField(format!(
                "row {} (date {date_str}): expected 3 columns, got {}",
                i + 2,
                record.len()
            )));
        }

        let description_str = record.get(1).unwrap_or("");
        let amount_str = record.get(2).unwrap_or("");

        let amount = parse_amount(amount_str).map_err(|_| {
            ParseError::InvalidAmount(amount_str.to_string(), format!("row {}", i + 2))
        })?;

        let description = clean_description(description_str);
        let raw_description = description_str.to_string();

        let occurrence = *occ_counts
            .entry(occurrence_key(&date, &amount, &raw_description))
            .and_modify(|c| *c += 1)
            .or_insert(0);
        let import_id = compute_import_id(&date, &amount, &raw_description, occurrence);

        transactions.push(Transaction {
            date,
            account,
            tx_type: TxType::Unknown(0),
            amount,
            description,
            raw_description,
            balance: None,
            tags: Vec::new(),
            import_id,
        });
    }

    Ok(transactions)
}

/// Parse First Direct's **current-account midata in its newer 4-column
/// schema**: `Date, Description, Amount, Balance`.
///
/// FD slimmed the current-account midata export down from the older
/// 5-column shape (`Date, Type, Merchant/Description, Debit/Credit,
/// Balance`, parsed by [`parse_midata`]): the new export drops the
/// transaction-type code and folds debit/credit into a single signed
/// `Amount`. So `tx_type` is `Unknown` (the bank no longer supplies it),
/// but the running `Balance` IS preserved — which is what distinguishes
/// this from the Visa 4-column schema (whose 4th column is a Reference and
/// is discarded; see [`parse_midata_visa`]). The caller supplies the
/// account label; `cmd_import` dispatches here vs `parse_midata` by
/// inspecting the header for a `Type` column.
pub fn parse_midata_current_4col<R: Read>(
    reader: R,
    account: Account,
) -> Result<Vec<Transaction>, ParseError> {
    let mut csv_reader = ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(reader);

    let mut transactions = Vec::new();
    // Count identical (date, 2dp-amount, description) rows in file order so each
    // gets a distinct occurrence index in its dedup id (see compute_import_id).
    let mut occ_counts: HashMap<String, usize> = HashMap::new();

    for (i, result) in csv_reader.records().enumerate() {
        let record = match result {
            Ok(r) => r,
            Err(_) => break, // malformed CSV structure => end of data / footer
        };

        // Footer begins at the first blank line.
        if record.get(0).map(|s| s.trim().is_empty()).unwrap_or(true) {
            break;
        }

        let date_str = record.get(0).unwrap_or("");
        // Date is the footer discriminator; a dated row with bad fields is
        // corruption, not footer — fail closed rather than silently truncate.
        let date = match NaiveDate::parse_from_str(date_str, "%d/%m/%Y") {
            Ok(d) => d,
            Err(_) => break,
        };

        if record.len() < 4 {
            return Err(ParseError::MissingField(format!(
                "row {} (date {date_str}): expected 4 columns, got {}",
                i + 2,
                record.len()
            )));
        }

        let description_str = record.get(1).unwrap_or("");
        let amount_str = record.get(2).unwrap_or("");
        let balance_str = record.get(3).unwrap_or("");

        let amount = parse_amount(amount_str).map_err(|_| {
            ParseError::InvalidAmount(amount_str.to_string(), format!("row {}", i + 2))
        })?;

        // Balance is the 4th column here (not a Reference); keep it.
        let balance = parse_amount(balance_str).ok();

        let description = clean_description(description_str);
        let raw_description = description_str.to_string();

        let occurrence = *occ_counts
            .entry(occurrence_key(&date, &amount, &raw_description))
            .and_modify(|c| *c += 1)
            .or_insert(0);
        let import_id = compute_import_id(&date, &amount, &raw_description, occurrence);

        transactions.push(Transaction {
            date,
            account,
            tx_type: TxType::Unknown(0),
            amount,
            description,
            raw_description,
            balance,
            tags: Vec::new(),
            import_id,
        });
    }

    Ok(transactions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    // Test fixtures use synthetic data only — fictional merchants and
    // round amounts. Public repo discipline: do not commit real
    // transaction data, even in tests.
    #[test]
    fn test_parse_midata_sample() {
        let csv_data = r#"Date,Type,Merchant/Description,Debit/Credit,Balance
01/01/2025,))),ACME COFFEE     Anytown,-£5.00,+£1000.00
01/01/2025,DD,EXAMPLE TELECOM,-£50.00,+£950.00"#;

        let transactions = parse_midata(csv_data.as_bytes(), Account::Current).unwrap();

        assert_eq!(transactions.len(), 2);

        let coffee = &transactions[0];
        assert_eq!(coffee.date, NaiveDate::from_ymd_opt(2025, 1, 1).unwrap());
        assert_eq!(coffee.tx_type, TxType::Contactless);
        assert_eq!(coffee.amount, Decimal::from_str("-5.00").unwrap());
        assert_eq!(coffee.description, "ACME COFFEE Anytown");

        let telecom = &transactions[1];
        assert_eq!(telecom.tx_type, TxType::DirectDebit);
    }

    #[test]
    fn test_parse_midata_visa_sample() {
        let csv_data = "Date,Description,Amount,Reference\n\
                        01/01/2025,Acme Widgets         Anytown  GB,-10.00,REF00000001\n\
                        02/01/2025,EXAMPLE INTEREST,-1.23,REF00000002";

        let transactions = parse_midata_visa(csv_data.as_bytes(), Account::Visa).unwrap();

        assert_eq!(transactions.len(), 2);
        let widget = &transactions[0];
        assert_eq!(widget.date, NaiveDate::from_ymd_opt(2025, 1, 1).unwrap());
        assert_eq!(widget.amount, Decimal::from_str("-10.00").unwrap());
        assert_eq!(widget.account, Account::Visa);
        assert!(widget.description.starts_with("Acme Widgets"));
    }

    #[test]
    fn test_parse_midata_current_4col_sample() {
        // New FD current-account midata: Date, Description, Amount, Balance.
        // Single signed Amount (no Debit/Credit column), no Type column.
        let csv_data = "Date,Description,Amount,Balance\n\
                        17/06/2026,ACME COFFEE     Anytown,-12.99,1000.00\n\
                        16/06/2026,EXAMPLE SALARY,2500.00,3500.00";

        let transactions =
            parse_midata_current_4col(csv_data.as_bytes(), Account::Current).unwrap();

        assert_eq!(transactions.len(), 2);

        let debit = &transactions[0];
        assert_eq!(debit.date, NaiveDate::from_ymd_opt(2026, 6, 17).unwrap());
        assert_eq!(debit.amount, Decimal::from_str("-12.99").unwrap());
        assert_eq!(debit.account, Account::Current);
        assert_eq!(debit.tx_type, TxType::Unknown(0));
        assert_eq!(debit.balance, Some(Decimal::from_str("1000.00").unwrap()));
        assert_eq!(debit.description, "ACME COFFEE Anytown");

        // A credit (income) stays positive and keeps its balance.
        let credit = &transactions[1];
        assert!(credit.amount.is_sign_positive());
        assert_eq!(credit.balance, Some(Decimal::from_str("3500.00").unwrap()));
    }

    #[test]
    fn footer_after_data_stops_cleanly_without_error() {
        // Data rows, then a First-Direct-style footer (blank line + overdraft
        // text). The non-date footer rows must end parsing WITHOUT an error.
        let csv_data = "Date,Type,Merchant/Description,Debit/Credit,Balance\n\
                        01/01/2025,))),ACME COFFEE,-£5.00,+£1000.00\n\
                        02/01/2025,DD,EXAMPLE TELECOM,-£50.00,+£950.00\n\
                        \n\
                        Arranged overdraft limit,£0.00,,,\n";
        let txns = parse_midata(csv_data.as_bytes(), Account::Current).unwrap();
        assert_eq!(txns.len(), 2);
    }

    #[test]
    fn corrupt_amount_midfile_aborts_not_truncates() {
        // Row 2 has a broken amount. The OLD code `break`d and silently returned
        // ONLY row 1 with a success — losing every later row. Now it fails
        // closed so the truncation can never masquerade as a complete import.
        let csv_data = "Date,Type,Merchant/Description,Debit/Credit,Balance\n\
                        01/01/2025,))),ACME COFFEE,-£5.00,+£1000.00\n\
                        02/01/2025,DD,EXAMPLE TELECOM,not-a-number,+£950.00\n\
                        03/01/2025,))),EXAMPLE SHOP,-£9.99,+£940.01\n";
        let err = parse_midata(csv_data.as_bytes(), Account::Current).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("amount") && msg.contains("row 3"),
            "expected a row-numbered amount error, got: {msg}"
        );
    }

    #[test]
    fn short_dated_row_aborts_not_truncates() {
        // A dated row missing columns is corruption, not a footer.
        let csv_data = "Date,Type,Merchant/Description,Debit/Credit,Balance\n\
                        01/01/2025,))),ACME COFFEE,-£5.00,+£1000.00\n\
                        02/01/2025,DD,ONLY THREE COLS\n";
        let err = parse_midata(csv_data.as_bytes(), Account::Current).unwrap_err();
        assert!(err.to_string().contains("row 3"), "got: {err}");
    }

    #[test]
    fn identical_same_day_rows_get_distinct_import_ids() {
        // Two identical contactless taps the same day are DISTINCT transactions
        // that must both survive import. The old (date+amount+desc) id collided,
        // silently dropping one on the next overlapping re-import.
        let csv_data = "Date,Type,Merchant/Description,Debit/Credit,Balance\n\
                        03/06/2026,))),ACME COFFEE,-£4.50,+£100.00\n\
                        03/06/2026,))),ACME COFFEE,-£4.50,+£95.50\n";
        let txns = parse_midata(csv_data.as_bytes(), Account::Current).unwrap();
        assert_eq!(txns.len(), 2);
        assert_ne!(
            txns[0].import_id, txns[1].import_id,
            "identical same-day rows must not collide"
        );
    }
}
