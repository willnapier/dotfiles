use crate::tags::TagRules;
use crate::Transaction;

/// Apply tag rules to a list of transactions
/// Updates tags in-place based on matching rules
pub fn apply_rules(transactions: &mut [Transaction], rules: &TagRules) {
    for tx in transactions.iter_mut() {
        let new_tags = rules.get_tags(&tx.raw_description, tx.amount, tx.date);
        for tag in new_tags {
            if !tx.tags.contains(&tag) {
                tx.tags.push(tag);
            }
        }
    }
}

/// Re-apply all rules from scratch (clears existing tags first)
pub fn reapply_rules(transactions: &mut [Transaction], rules: &TagRules) {
    for tx in transactions.iter_mut() {
        tx.tags.clear();
        tx.tags = rules.get_tags(&tx.raw_description, tx.amount, tx.date);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Account, TxType};
    use chrono::NaiveDate;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn make_tx(raw_desc: &str) -> Transaction {
        Transaction {
            date: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            account: Account::Current,
            tx_type: TxType::Contactless,
            amount: Decimal::from_str("-10.00").unwrap(),
            description: raw_desc.to_string(),
            raw_description: raw_desc.to_string(),
            balance: None,
            tags: Vec::new(),
            import_id: "test123".to_string(),
        }
    }

    #[test]
    fn test_apply_rules() {
        let mut rules = TagRules::default();
        rules.add_rule(
            "TESCO",
            vec!["groceries".to_string()],
            None,
            None,
            None,
            None,
            None,
        );

        let mut transactions = vec![make_tx("TESCO STORES LONDON"), make_tx("PRET A MANGER")];

        apply_rules(&mut transactions, &rules);

        assert_eq!(transactions[0].tags, vec!["groceries"]);
        assert!(transactions[1].tags.is_empty());
    }
}
