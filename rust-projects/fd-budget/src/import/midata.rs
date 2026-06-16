use crate::dedup::compute_import_id;
use crate::import::normalize::{clean_description, parse_amount};
use crate::{Account, Transaction, TxType};
use chrono::NaiveDate;
use csv::ReaderBuilder;
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
pub fn parse_midata<R: Read>(reader: R, account: Account) -> Result<Vec<Transaction>, ParseError> {
    let mut csv_reader = ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(reader);

    let mut transactions = Vec::new();

    for result in csv_reader.records() {
        let record = match result {
            Ok(r) => r,
            Err(_) => break, // Stop on malformed rows (footer section)
        };

        // Skip empty rows
        if record.len() < 5 || record.get(0).map(|s| s.trim().is_empty()).unwrap_or(true) {
            break;
        }

        let date_str = record.get(0).unwrap_or("");
        let tx_type_str = record.get(1).unwrap_or("");
        let description_str = record.get(2).unwrap_or("");
        let amount_str = record.get(3).unwrap_or("");
        let balance_str = record.get(4).unwrap_or("");

        // Parse date (DD/MM/YYYY) - stop if date doesn't parse (footer row)
        let date = match NaiveDate::parse_from_str(date_str, "%d/%m/%Y") {
            Ok(d) => d,
            Err(_) => break, // Footer section doesn't have valid dates
        };

        // Parse transaction type
        let tx_type = TxType::from_code(tx_type_str);

        // Parse amount
        let amount = match parse_amount(amount_str) {
            Ok(a) => a,
            Err(_) => break, // Footer section
        };

        // Parse balance (optional, might fail on some rows)
        let balance = parse_amount(balance_str).ok();

        // Clean description
        let description = clean_description(description_str);
        let raw_description = description_str.to_string();

        // Compute dedup ID
        let import_id = compute_import_id(&date, &amount, &raw_description);

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

    for result in csv_reader.records() {
        let record = match result {
            Ok(r) => r,
            Err(_) => break,
        };

        if record.len() < 3 || record.get(0).map(|s| s.trim().is_empty()).unwrap_or(true) {
            break;
        }

        let date_str = record.get(0).unwrap_or("");
        let description_str = record.get(1).unwrap_or("");
        let amount_str = record.get(2).unwrap_or("");

        let date = match NaiveDate::parse_from_str(date_str, "%d/%m/%Y") {
            Ok(d) => d,
            Err(_) => break,
        };

        let amount = match parse_amount(amount_str) {
            Ok(a) => a,
            Err(_) => break,
        };

        let description = clean_description(description_str);
        let raw_description = description_str.to_string();

        let import_id = compute_import_id(&date, &amount, &raw_description);

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
}
