use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PaypalError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),
    #[error("Parse error at line {line}: {message}")]
    Parse { line: usize, message: String },
}

/// A parsed PayPal transaction for matching
#[derive(Debug, Clone)]
pub struct PaypalTransaction {
    pub date: NaiveDate,
    pub amount: Decimal,
    pub name: String,
    pub item_title: Option<String>,
    pub tx_type: String,
}

/// Raw PayPal CSV row - handles the variable column format
#[derive(Debug, Deserialize)]
struct PaypalRow {
    #[serde(rename = "Date")]
    date: String,
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(alias = "Amount", alias = "Gross")]
    amount: Option<String>,
    #[serde(rename = "Type")]
    tx_type: Option<String>,
    #[serde(rename = "Item Title")]
    item_title: Option<String>,
    #[serde(rename = "Status")]
    status: Option<String>,
}

/// Parse a PayPal CSV export file
pub fn parse_paypal<P: AsRef<Path>>(path: P) -> Result<Vec<PaypalTransaction>, PaypalError> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .has_headers(true)
        .from_path(path)?;

    let mut transactions = Vec::new();

    for (idx, result) in reader.deserialize().enumerate() {
        let row: PaypalRow = match result {
            Ok(r) => r,
            Err(e) => {
                // Skip malformed rows but log
                eprintln!("Warning: skipping row {}: {}", idx + 2, e);
                continue;
            }
        };

        // Skip non-completed transactions
        if let Some(ref status) = row.status {
            if status != "Completed" {
                continue;
            }
        }

        // Skip certain transaction types (transfers, currency conversions)
        let tx_type = row.tx_type.unwrap_or_default();
        if tx_type.contains("Transfer")
            || tx_type.contains("Currency Conversion")
            || tx_type.contains("Authorization")
        {
            continue;
        }

        // Parse date (PayPal uses DD/MM/YYYY format in UK)
        let date = parse_paypal_date(&row.date).ok_or_else(|| PaypalError::Parse {
            line: idx + 2,
            message: format!("Invalid date: {}", row.date),
        })?;

        // Parse amount - skip if empty
        let amount_str = row.amount.unwrap_or_default();
        if amount_str.is_empty() {
            continue;
        }
        let amount = match parse_paypal_amount(&amount_str) {
            Some(a) => a,
            None => continue, // Skip unparseable amounts
        };

        // Get merchant name
        let name = row.name.unwrap_or_else(|| "Unknown".to_string());
        if name.is_empty() || name == "Unknown" {
            continue;
        }

        transactions.push(PaypalTransaction {
            date,
            amount,
            name,
            item_title: row.item_title,
            tx_type,
        });
    }

    Ok(transactions)
}

/// Parse PayPal date format (DD/MM/YYYY)
fn parse_paypal_date(s: &str) -> Option<NaiveDate> {
    // Try DD/MM/YYYY first
    if let Ok(date) = NaiveDate::parse_from_str(s, "%d/%m/%Y") {
        return Some(date);
    }
    // Try MM/DD/YYYY (US format)
    if let Ok(date) = NaiveDate::parse_from_str(s, "%m/%d/%Y") {
        return Some(date);
    }
    // Try YYYY-MM-DD
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(date);
    }
    None
}

/// Parse PayPal amount (handles currency symbols, commas, negatives)
fn parse_paypal_amount(s: &str) -> Option<Decimal> {
    use std::str::FromStr;

    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();

    Decimal::from_str(&cleaned).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_parse_amount() {
        assert_eq!(
            parse_paypal_amount("-£4.80"),
            Some(Decimal::from_str("-4.80").unwrap())
        );
        assert_eq!(
            parse_paypal_amount("£100.00"),
            Some(Decimal::from_str("100.00").unwrap())
        );
        assert_eq!(
            parse_paypal_amount("-4.80"),
            Some(Decimal::from_str("-4.80").unwrap())
        );
    }

    #[test]
    fn test_parse_date() {
        assert_eq!(
            parse_paypal_date("05/11/2025"),
            Some(NaiveDate::from_ymd_opt(2025, 11, 5).unwrap())
        );
    }
}
