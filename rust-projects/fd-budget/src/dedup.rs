use chrono::NaiveDate;
use rust_decimal::Decimal;
use sha2::{Sha256, Digest};

/// Compute a unique ID for a transaction based on date, amount, and raw description.
/// This is used to detect duplicates when importing overlapping date ranges.
pub fn compute_import_id(date: &NaiveDate, amount: &Decimal, raw_description: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(date.to_string().as_bytes());
    hasher.update(amount.to_string().as_bytes());
    hasher.update(raw_description.as_bytes());
    let result = hasher.finalize();
    // Use first 16 hex chars (64 bits) - enough for uniqueness
    hex::encode(&result[..8])
}

/// Filter out transactions that already exist in the store
pub fn deduplicate(
    new_transactions: Vec<crate::Transaction>,
    existing_ids: &std::collections::HashSet<String>,
) -> Vec<crate::Transaction> {
    new_transactions
        .into_iter()
        .filter(|tx| !existing_ids.contains(&tx.import_id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn test_compute_import_id() {
        let date = NaiveDate::from_ymd_opt(2025, 11, 7).unwrap();
        let amount = Decimal::from_str("-9.00").unwrap();
        let desc = "PRET A MANGER London";

        let id1 = compute_import_id(&date, &amount, desc);
        let id2 = compute_import_id(&date, &amount, desc);

        // Same inputs should give same ID
        assert_eq!(id1, id2);

        // Different inputs should give different ID
        let id3 = compute_import_id(&date, &Decimal::from_str("-10.00").unwrap(), desc);
        assert_ne!(id1, id3);
    }
}
