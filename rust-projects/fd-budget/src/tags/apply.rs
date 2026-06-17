use crate::Transaction;
use crate::tags::TagRules;

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
    use chrono::NaiveDate;
    use rust_decimal::Decimal;
    use std::str::FromStr;
    use crate::{Account, TxType};

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
        rules.add_rule("TESCO", vec!["groceries".to_string()], None, None, None, None, None);

        let mut transactions = vec![
            make_tx("TESCO STORES LONDON"),
            make_tx("PRET A MANGER"),
        ];

        apply_rules(&mut transactions, &rules);

        assert_eq!(transactions[0].tags, vec!["groceries"]);
        assert!(transactions[1].tags.is_empty());
    }

    #[test]
    fn apply_rules_is_additive_preserves_manual_tags() {
        let mut rules = TagRules::default();
        rules.add_rule("TESCO", vec!["groceries".to_string()], None, None, None, None, None);

        let mut tx = make_tx("TESCO STORES LONDON");
        tx.tags = vec!["manual-tag".to_string()]; // e.g. applied via `categorize`
        let mut transactions = vec![tx];

        apply_rules(&mut transactions, &rules);

        // manual tag survives AND the rule match is appended
        assert!(transactions[0].tags.contains(&"manual-tag".to_string()));
        assert!(transactions[0].tags.contains(&"groceries".to_string()));
    }

    #[test]
    fn reapply_rules_clears_then_rebuilds() {
        let mut rules = TagRules::default();
        rules.add_rule("TESCO", vec!["groceries".to_string()], None, None, None, None, None);

        let mut tx = make_tx("TESCO STORES LONDON");
        tx.tags = vec!["manual-tag".to_string()];
        let mut transactions = vec![tx];

        reapply_rules(&mut transactions, &rules);

        // clear-then-rebuild: the manual tag is dropped, only the rule remains
        assert!(!transactions[0].tags.contains(&"manual-tag".to_string()));
        assert_eq!(transactions[0].tags, vec!["groceries".to_string()]);
    }
}
