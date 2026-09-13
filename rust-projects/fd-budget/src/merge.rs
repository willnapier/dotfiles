//! Merge a second machine's store into this one.
//!
//! The two stores are the same bank data captured on two machines. Rows are
//! matched by `import_id` first. Rows that exist on both sides but with
//! DIFFERENT ids — the same transaction imported from two First Direct export
//! schemas (the June 2026 4-column export carries payees and card numbers;
//! the August 2026 5-column export masks them but carries the type) — are
//! paired as "twins" by account, date, 2dp amount and running balance, and
//! kept once. Tags are unioned; an `Unknown` tx_type is filled from the other
//! side. Rows without a balance (Visa) are never twinned, only id-matched.
//!
//! Pure: no I/O here. The CLI (`fd-budget merge`) snapshots and rewrites.

use crate::tags::rules::{Rule, TagRules};
use crate::{Transaction, TxType};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MergeReport {
    pub local_rows: usize,
    pub other_rows: usize,
    /// Same import_id on both sides
    pub shared: usize,
    /// Rows (shared or twin) that received tags from the other side
    pub tags_merged: usize,
    /// Rows whose Unknown tx_type was filled from the other side
    pub tx_type_filled: usize,
    /// Other-side rows kept once because a local row with a different id is
    /// the same transaction (account, date, amount, balance)
    pub twins: usize,
    /// Other-side rows appended as genuinely new
    pub added: usize,
    /// (local id, other id) for each twin, for the operator to eyeball
    pub twin_pairs: Vec<(String, String)>,
}

fn twin_key(tx: &Transaction) -> Option<String> {
    let bal = tx.balance?;
    Some(format!("{}|{}|{:.2}|{:.2}", tx.account, tx.date, tx.amount, bal))
}

fn merge_tags(into: &mut Vec<String>, from: &[String]) -> bool {
    let mut changed = false;
    for t in from {
        if !into.iter().any(|x| x == t) {
            into.push(t.clone());
            changed = true;
        }
    }
    changed
}

fn fill_tx_type(local: &mut Transaction, other: &Transaction) -> bool {
    if matches!(local.tx_type, TxType::Unknown(_)) && !matches!(other.tx_type, TxType::Unknown(_)) {
        local.tx_type = other.tx_type;
        true
    } else {
        false
    }
}

/// Merge `other` into `local`. Returns the merged rows (local order, then the
/// genuinely new rows in the other store's order) and a report.
pub fn merge_stores(mut local: Vec<Transaction>, other: Vec<Transaction>) -> (Vec<Transaction>, MergeReport) {
    let mut report = MergeReport { local_rows: local.len(), other_rows: other.len(), ..Default::default() };
    let other_ids: HashSet<&str> = other.iter().map(|t| t.import_id.as_str()).collect();
    let by_id: HashMap<String, usize> = local.iter().enumerate().map(|(i, t)| (t.import_id.clone(), i)).collect();
    // Twin candidates: local rows the other side does not have by id, keyed by
    // account/date/amount/balance, consumed in store order so identical taps
    // pair one-to-one.
    let mut twin_index: HashMap<String, VecDeque<usize>> = HashMap::new();
    for (i, t) in local.iter().enumerate() {
        if !other_ids.contains(t.import_id.as_str()) {
            if let Some(k) = twin_key(t) {
                twin_index.entry(k).or_default().push_back(i);
            }
        }
    }
    let mut added = vec![];
    for o in other {
        let target = if let Some(&i) = by_id.get(&o.import_id) {
            report.shared += 1;
            Some(i)
        } else if let Some(i) = twin_key(&o).and_then(|k| twin_index.get_mut(&k)).and_then(|q| q.pop_front()) {
            report.twins += 1;
            report.twin_pairs.push((local[i].import_id.clone(), o.import_id.clone()));
            Some(i)
        } else {
            None
        };
        match target {
            Some(i) => {
                if merge_tags(&mut local[i].tags, &o.tags) {
                    report.tags_merged += 1;
                }
                if fill_tx_type(&mut local[i], &o) {
                    report.tx_type_filled += 1;
                }
            }
            None => {
                report.added += 1;
                added.push(o);
            }
        }
    }
    local.extend(added);
    (local, report)
}

fn rule_key(r: &Rule) -> String {
    format!("{}|{:?}|{:?}|{:?}|{:?}|{:?}", r.pattern.to_lowercase(), r.amount, r.min_amount, r.max_amount, r.day_of_month, r.day_window)
}

/// Append the other store's rules whose (pattern, amount bounds, day window)
/// this store lacks. Rules present on both sides keep the LOCAL tags — a
/// deliberate non-decision: conflicting edits to the same rule are for a
/// person to reconcile (`tag rename` / editing rules.toml), not a merge.
/// Returns the appended rules and the shared patterns whose tags differ.
pub fn merge_rules(local: &mut TagRules, other: &TagRules) -> (Vec<Rule>, Vec<(String, Vec<String>, Vec<String>)>) {
    let mut seen: HashMap<String, usize> = local.rules.iter().enumerate().map(|(i, r)| (rule_key(r), i)).collect();
    let mut appended = vec![];
    let mut conflicts = vec![];
    for r in &other.rules {
        match seen.get(&rule_key(r)) {
            Some(&i) => {
                if local.rules[i].tags != r.tags {
                    conflicts.push((r.pattern.clone(), local.rules[i].tags.clone(), r.tags.clone()));
                }
            }
            None => {
                seen.insert(rule_key(r), local.rules.len());
                local.rules.push(r.clone());
                appended.push(r.clone());
            }
        }
    }
    (appended, conflicts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Account;
    use chrono::NaiveDate;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn tx(id: &str, date: &str, amount: &str, balance: Option<&str>, desc: &str, ty: TxType, tags: &[&str]) -> Transaction {
        Transaction {
            date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            account: Account::Current,
            tx_type: ty,
            amount: Decimal::from_str(amount).unwrap(),
            description: desc.into(),
            raw_description: desc.into(),
            balance: balance.map(|b| Decimal::from_str(b).unwrap()),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            import_id: id.into(),
        }
    }

    #[test]
    fn shared_ids_union_tags_and_fill_unknown_tx_type() {
        let local = vec![tx("a", "2026-05-01", "-10.00", Some("100.00"), "X", TxType::Unknown(0), &["food"])];
        let other = vec![tx("a", "2026-05-01", "-10.00", Some("100.00"), "X", TxType::Contactless, &["workfood", "food"])];
        let (rows, rep) = merge_stores(local, other);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tags, vec!["food", "workfood"]);
        assert_eq!(rows[0].tx_type, TxType::Contactless);
        assert_eq!((rep.shared, rep.tags_merged, rep.tx_type_filled, rep.twins, rep.added), (1, 1, 1, 0, 0));
    }

    #[test]
    fn twins_with_different_ids_are_kept_once_and_local_description_wins() {
        // Same £20k payment: June export names the payee, August export masks it.
        let local = vec![tx("m1", "2026-05-29", "-20000.00", Some("4910.90"), "NAPIER PSYCHOLOGY DIRECTORS LOAN", TxType::Unknown(0), &["transfer"])];
        let other = vec![tx("n1", "2026-05-29", "-20000.00", Some("4910.90"), "************", TxType::BankPayment, &[])];
        let (rows, rep) = merge_stores(local, other);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].import_id, "m1");
        assert_eq!(rows[0].description, "NAPIER PSYCHOLOGY DIRECTORS LOAN");
        assert_eq!(rows[0].tx_type, TxType::BankPayment);
        assert_eq!(rep.twins, 1);
        assert_eq!(rep.twin_pairs, vec![("m1".to_string(), "n1".to_string())]);
        assert_eq!(rep.added, 0);
    }

    #[test]
    fn identical_taps_pair_one_to_one_in_order_and_the_rest_are_added() {
        let local = vec![
            tx("m1", "2026-05-01", "-4.90", Some("100.00"), "SAINSBURYS 2210", TxType::Unknown(0), &[]),
            tx("m2", "2026-05-01", "-4.90", Some("95.10"), "SAINSBURYS 2210", TxType::Unknown(0), &[]),
        ];
        let other = vec![
            tx("n1", "2026-05-01", "-4.90", Some("100.00"), "SAINSBURYS ****", TxType::Contactless, &["groceries"]),
            tx("n2", "2026-05-01", "-4.90", Some("95.10"), "SAINSBURYS ****", TxType::Contactless, &["groceries"]),
            tx("n3", "2026-07-01", "-4.90", Some("90.20"), "SAINSBURYS ****", TxType::Contactless, &["groceries"]),
        ];
        let (rows, rep) = merge_stores(local, other);
        assert_eq!(rows.len(), 3);
        assert_eq!(rep.twins, 2);
        assert_eq!(rep.added, 1);
        assert_eq!(rows[2].import_id, "n3");
        assert!(rows.iter().all(|r| r.tags == vec!["groceries"]));
    }

    #[test]
    fn rows_without_a_balance_are_never_twinned() {
        let local = vec![tx("m1", "2026-05-01", "-4.90", None, "A", TxType::Unknown(0), &[])];
        let other = vec![tx("n1", "2026-05-01", "-4.90", None, "A", TxType::Contactless, &[])];
        let (rows, rep) = merge_stores(local, other);
        assert_eq!(rows.len(), 2);
        assert_eq!(rep.twins, 0);
        assert_eq!(rep.added, 1);
    }

    #[test]
    fn a_local_row_the_other_side_has_by_id_is_not_a_twin_candidate() {
        // Both sides hold "a"; the other side also has a second row with the same
        // key but a new id — that is a genuinely new row (an occurrence), not a twin.
        let local = vec![tx("a", "2026-05-01", "-4.90", Some("100.00"), "A", TxType::Contactless, &[])];
        let other = vec![
            tx("a", "2026-05-01", "-4.90", Some("100.00"), "A", TxType::Contactless, &[]),
            tx("b", "2026-05-01", "-4.90", Some("100.00"), "A", TxType::Contactless, &[]),
        ];
        let (rows, rep) = merge_stores(local, other);
        assert_eq!(rows.len(), 2);
        assert_eq!((rep.shared, rep.twins, rep.added), (1, 0, 1));
    }

    #[test]
    fn rules_merge_appends_missing_patterns_and_reports_conflicts_without_changing_local() {
        let mk = |p: &str, tags: &[&str]| Rule { pattern: p.into(), tags: tags.iter().map(|s| s.to_string()).collect(), amount: None, min_amount: None, max_amount: None, day_of_month: None, day_window: None };
        let mut local = TagRules { rules: vec![mk("ECOTRICITY", &["transport", "ev-charging"]), mk("TFL ", &["transport"])] };
        let other = TagRules { rules: vec![mk("ecotricity", &["bills", "utilities"]), mk("Max Pt", &["gym", "pt"])] };
        let (appended, conflicts) = merge_rules(&mut local, &other);
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].pattern, "Max Pt");
        assert_eq!(local.rules.len(), 3);
        assert_eq!(local.rules[0].tags, vec!["transport", "ev-charging"]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].0, "ecotricity");
    }
}
