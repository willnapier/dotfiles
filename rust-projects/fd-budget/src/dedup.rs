use chrono::NaiveDate;
use rust_decimal::Decimal;
use sha2::{Sha256, Digest};

/// Compute the dedup ID for a transaction.
///
/// Inputs are DELIMITED, the amount is normalised to 2dp, and an `occurrence`
/// index is folded in — so two genuinely-distinct rows with the same
/// date/amount/description (two identical contactless taps, or two bare
/// `PAYPAL PAYMENT` rows the same day) get DISTINCT ids. The old scheme
/// (`date + amount + raw_description`, no delimiters, no occurrence) collided
/// on such rows, which silently dropped the duplicate on re-import and
/// collapsed two PayPal merchants onto one bank row. Delimiters also remove the
/// field-boundary ambiguity (`-5.0`+`0X` ≡ `-5.00`+`X`), and 2dp-normalisation
/// keeps the id stable if a future export renders `-5.0` vs `-5.00`.
///
/// `occurrence` is the 0-based index of this row among identical
/// (date, 2dp-amount, raw_description) rows within a single import, assigned in
/// file order (bank exports are chronologically ordered, so it is stable across
/// overlapping re-imports).
pub fn compute_import_id(
    date: &NaiveDate,
    amount: &Decimal,
    raw_description: &str,
    occurrence: usize,
) -> String {
    let mut hasher = Sha256::new();
    let key = format!("{date}|{:.2}|{raw_description}|{occurrence}", amount);
    hasher.update(key.as_bytes());
    let result = hasher.finalize();
    // Use first 16 hex chars (64 bits) - enough for uniqueness
    hex::encode(&result[..8])
}

/// The occurrence-grouping key: everything the id hashes EXCEPT the occurrence
/// index. Callers count identical keys in file order to assign `occurrence`.
/// Kept here so the parser's counting key can never drift from the hash input.
pub fn occurrence_key(date: &NaiveDate, amount: &Decimal, raw_description: &str) -> String {
    format!("{date}|{}|{raw_description}", amount.round_dp(2))
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

        let id1 = compute_import_id(&date, &amount, desc, 0);
        let id2 = compute_import_id(&date, &amount, desc, 0);

        // Same inputs should give same ID
        assert_eq!(id1, id2);

        // Different inputs should give different ID
        let id3 = compute_import_id(&date, &Decimal::from_str("-10.00").unwrap(), desc, 0);
        assert_ne!(id1, id3);
    }

    #[test]
    fn identical_rows_get_distinct_ids_by_occurrence() {
        // The exact collision the old scheme silently dropped: two identical
        // same-day/same-amount rows (worst case: bare "PAYPAL PAYMENT").
        let date = NaiveDate::from_ymd_opt(2026, 6, 3).unwrap();
        let amount = Decimal::from_str("-12.99").unwrap();
        let desc = "PAYPAL PAYMENT";
        let first = compute_import_id(&date, &amount, desc, 0);
        let second = compute_import_id(&date, &amount, desc, 1);
        assert_ne!(first, second);
    }

    #[test]
    fn amount_scale_is_normalised() {
        // -5.0 and -5.00 must produce the SAME id (fixes scale-sensitivity that
        // would otherwise re-key history on an export rendering change).
        let date = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        let a = compute_import_id(&date, &Decimal::from_str("-5.0").unwrap(), "X", 0);
        let b = compute_import_id(&date, &Decimal::from_str("-5.00").unwrap(), "X", 0);
        assert_eq!(a, b);
    }

    #[test]
    fn occurrence_key_matches_hash_inputs() {
        // The counting key must stay in lock-step with what the id hashes.
        let date = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
        let amount = Decimal::from_str("-5.00").unwrap();
        assert_eq!(occurrence_key(&date, &amount, "X"), "2025-01-01|-5.00|X");
    }
}
