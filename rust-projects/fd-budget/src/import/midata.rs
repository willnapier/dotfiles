use crate::{Account, Transaction, TxType};
use crate::dedup::compute_import_id;
use crate::import::normalize::{clean_description, parse_amount};
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
pub fn parse_midata<R: Read>(
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

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn test_parse_midata_sample() {
        let csv_data = r#"Date,Type,Merchant/Description,Debit/Credit,Balance
07/11/2025,))),PRET A MANGER     London,-£9.00,+£8478.33
07/11/2025,DD,VIRGIN MEDIA PYMTS,-£70.46,-£1373.78"#;

        let transactions = parse_midata(csv_data.as_bytes(), Account::Current).unwrap();

        assert_eq!(transactions.len(), 2);

        let pret = &transactions[0];
        assert_eq!(pret.date, NaiveDate::from_ymd_opt(2025, 11, 7).unwrap());
        assert_eq!(pret.tx_type, TxType::Contactless);
        assert_eq!(pret.amount, Decimal::from_str("-9.00").unwrap());
        assert_eq!(pret.description, "PRET A MANGER London");

        let virgin = &transactions[1];
        assert_eq!(virgin.tx_type, TxType::DirectDebit);
    }
}
