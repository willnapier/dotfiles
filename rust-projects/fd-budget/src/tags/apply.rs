use crate::paypal::RecoveryIndex;
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

/// Apply tag rules, additionally matching rules against the **recovered PayPal
/// merchant** for bare `PAYPAL PAYMENT` rows.
///
/// The bank `raw_description` for a PayPal purchase is just `PAYPAL PAYMENT` —
/// it carries no merchant, so a rule like `pattern = "Streamflix"` can never
/// fire on it. This pass looks up the recovered merchant (from
/// `paypal_matches.jsonl`) by `import_id` and runs the SAME rule engine against
/// that recovered text, so an existing Streamflix rule tags the recovered
/// PAYPAL row. It is ADDITIVE (manual tags and `raw_description` matches are
/// preserved); only NEW rule matches are appended.
///
/// Callers that persist the result MUST snapshot `transactions.csv` first
/// (this is a legitimate tag mutation — see the build primer's snapshot rule).
pub fn apply_rules_with_recovery(
    transactions: &mut [Transaction],
    rules: &TagRules,
    recoveries: &RecoveryIndex,
) {
    for tx in transactions.iter_mut() {
        // 1. Normal pass: rules against the bank raw_description.
        let mut new_tags = rules.get_tags(&tx.raw_description, tx.amount, tx.date);

        // 2. Recovery pass: for rows with a recovered merchant, also match rules
        //    against the recovered text (same amount/date, so amount/day rules
        //    still apply correctly).
        if let Some(merchant) = recoveries.recovered_merchant_for(&tx.import_id) {
            if !merchant.trim().is_empty() {
                new_tags.extend(rules.get_tags(merchant, tx.amount, tx.date));
            }
        }

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

    #[test]
    fn apply_rules_is_additive_preserves_manual_tags() {
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
        rules.add_rule(
            "TESCO",
            vec!["groceries".to_string()],
            None,
            None,
            None,
            None,
            None,
        );

        let mut tx = make_tx("TESCO STORES LONDON");
        tx.tags = vec!["manual-tag".to_string()];
        let mut transactions = vec![tx];

        reapply_rules(&mut transactions, &rules);

        // clear-then-rebuild: the manual tag is dropped, only the rule remains
        assert!(!transactions[0].tags.contains(&"manual-tag".to_string()));
        assert_eq!(transactions[0].tags, vec!["groceries".to_string()]);
    }

    fn recovery_index(pairs: &[(&str, &str)]) -> RecoveryIndex {
        let rows = pairs
            .iter()
            .map(|(id, merchant)| crate::paypal::RecoveryRow {
                bank_import_id: id.to_string(),
                recovered_merchant: merchant.to_string(),
                currency: "GBP".to_string(),
                leg: "direct-gbp".to_string(),
            })
            .collect();
        RecoveryIndex::from_rows(rows)
    }

    #[test]
    fn recovery_pass_tags_bare_paypal_row_via_recovered_merchant() {
        // A "Streamflix" rule cannot match the bare bank description
        // "PAYPAL PAYMENT" — but it SHOULD match the recovered merchant.
        let mut rules = TagRules::default();
        rules.add_rule(
            "Streamflix",
            vec!["subscription".to_string()],
            None,
            None,
            None,
            None,
            None,
        );

        let mut tx = make_tx("PAYPAL PAYMENT");
        tx.import_id = "bank-1".to_string();
        let mut transactions = vec![tx];

        // Without recovery: no match (proves the recovery pass is load-bearing).
        apply_rules(&mut transactions, &rules);
        assert!(transactions[0].tags.is_empty());

        // With recovery: the Streamflix rule fires on the recovered merchant.
        let idx = recovery_index(&[("bank-1", "Streamflix Monthly")]);
        apply_rules_with_recovery(&mut transactions, &rules, &idx);
        assert!(transactions[0].tags.contains(&"subscription".to_string()));
    }

    #[test]
    fn recovery_pass_still_matches_raw_description() {
        // A row with no recovery still gets normal raw_description matching.
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
        let mut transactions = vec![make_tx("TESCO STORES LONDON")];
        let idx = recovery_index(&[]); // empty
        apply_rules_with_recovery(&mut transactions, &rules, &idx);
        assert!(transactions[0].tags.contains(&"groceries".to_string()));
    }

    #[test]
    fn recovery_pass_is_additive() {
        let mut rules = TagRules::default();
        rules.add_rule(
            "Streamflix",
            vec!["subscription".to_string()],
            None,
            None,
            None,
            None,
            None,
        );
        let mut tx = make_tx("PAYPAL PAYMENT");
        tx.import_id = "bank-1".to_string();
        tx.tags = vec!["manual".to_string()];
        let mut transactions = vec![tx];
        let idx = recovery_index(&[("bank-1", "Streamflix")]);
        apply_rules_with_recovery(&mut transactions, &rules, &idx);
        assert!(transactions[0].tags.contains(&"manual".to_string()));
        assert!(transactions[0].tags.contains(&"subscription".to_string()));
    }
}
