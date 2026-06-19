use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RulesError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("TOML serialize error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// Pattern to match against description (case-insensitive substring)
    pub pattern: String,
    /// Tags to apply when pattern matches
    pub tags: Vec<String>,
    /// Exact amount to match (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<Decimal>,
    /// Minimum amount for range match (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_amount: Option<Decimal>,
    /// Maximum amount for range match (optional)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_amount: Option<Decimal>,
    /// Target day of month to match (1-31)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day_of_month: Option<u32>,
    /// Tolerance in days around day_of_month (default 0)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day_window: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TagRules {
    #[serde(default)]
    pub rules: Vec<Rule>,
}

/// Returns the number of days in the month of the given date
fn last_day_of_month(date: NaiveDate) -> u32 {
    let (y, m) = if date.month() == 12 {
        (date.year() + 1, 1)
    } else {
        (date.year(), date.month() + 1)
    };
    NaiveDate::from_ymd_opt(y, m, 1)
        .unwrap()
        .pred_opt()
        .unwrap()
        .day()
}

impl TagRules {
    /// Load rules from a TOML file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, RulesError> {
        if !path.as_ref().exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(path)?;
        let rules: TagRules = toml::from_str(&content)?;
        Ok(rules)
    }

    /// Save rules to a TOML file
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), RulesError> {
        let content = toml::to_string_pretty(self)?;
        fs::write(path, content)?;
        Ok(())
    }

    /// Add a new rule (or update existing pattern)
    pub fn add_rule(
        &mut self,
        pattern: &str,
        tags: Vec<String>,
        amount: Option<Decimal>,
        min_amount: Option<Decimal>,
        max_amount: Option<Decimal>,
        day_of_month: Option<u32>,
        day_window: Option<u32>,
    ) {
        // Check if pattern already exists with same conditions
        if let Some(existing) = self.rules.iter_mut().find(|r| {
            r.pattern == pattern
                && r.amount == amount
                && r.min_amount == min_amount
                && r.max_amount == max_amount
                && r.day_of_month == day_of_month
                && r.day_window == day_window
        }) {
            // Add new tags, avoiding duplicates
            for tag in tags {
                if !existing.tags.contains(&tag) {
                    existing.tags.push(tag);
                }
            }
        } else {
            self.rules.push(Rule {
                pattern: pattern.to_string(),
                tags,
                amount,
                min_amount,
                max_amount,
                day_of_month,
                day_window,
            });
        }
    }

    /// Remove tags from a pattern
    pub fn remove_tags(&mut self, pattern: &str, tags: &[String]) {
        if let Some(rule) = self.rules.iter_mut().find(|r| r.pattern == pattern) {
            rule.tags.retain(|t| !tags.contains(t));
        }
        // Remove rule if no tags left
        self.rules.retain(|r| !r.tags.is_empty());
    }

    /// Find all matching rules for a description, amount, and date
    pub fn find_matches(&self, description: &str, amount: Decimal, date: NaiveDate) -> Vec<&Rule> {
        let desc_lower = description.to_lowercase();
        self.rules
            .iter()
            .filter(|rule| {
                // Description must match
                if !desc_lower.contains(&rule.pattern.to_lowercase()) {
                    return false;
                }
                // Check amount conditions (all are ANDed)
                if let Some(exact) = rule.amount {
                    if amount != exact {
                        return false;
                    }
                }
                if let Some(min) = rule.min_amount {
                    if amount < min {
                        return false;
                    }
                }
                if let Some(max) = rule.max_amount {
                    if amount > max {
                        return false;
                    }
                }
                // Check day-of-month condition
                if let Some(target_day) = rule.day_of_month {
                    let window = rule.day_window.unwrap_or(0);
                    let tx_day = date.day();
                    let days_in_month = last_day_of_month(date);
                    // Clamp target to month length (day 31 in Feb -> 28)
                    let target = target_day.min(days_in_month);
                    let diff = (tx_day as i32 - target as i32).unsigned_abs();
                    let circular_diff = diff.min(days_in_month - diff);
                    if circular_diff > window {
                        return false;
                    }
                }
                true
            })
            .collect()
    }

    /// Get all tags that match a description, amount, and date
    pub fn get_tags(&self, description: &str, amount: Decimal, date: NaiveDate) -> Vec<String> {
        let mut tags: Vec<String> = self
            .find_matches(description, amount, date)
            .into_iter()
            .flat_map(|r| r.tags.clone())
            .collect();

        // Deduplicate while preserving order
        let mut seen = std::collections::HashSet::new();
        tags.retain(|t| seen.insert(t.clone()));

        tags
    }

    /// Rename tag `old` to `new` across all rule definitions, de-duplicating
    /// within each rule's tag list. When `merchant` is `Some`, only rules whose
    /// `pattern` contains that substring (case-insensitive) are touched — used
    /// to scope a rename to one merchant (e.g. `BT GROUP` phone→internet without
    /// disturbing `VODAFONE`'s `phone`). Returns the number of rules changed.
    pub fn rename_tag(&mut self, old: &str, new: &str, merchant: Option<&str>) -> usize {
        let needle = merchant.map(|s| s.to_lowercase());
        let mut changed = 0;
        for rule in self.rules.iter_mut() {
            if let Some(ref m) = needle {
                if !rule.pattern.to_lowercase().contains(m) {
                    continue;
                }
            }
            if rename_tag_in_list(&mut rule.tags, old, new) {
                changed += 1;
            }
        }
        changed
    }
}

/// Replace tag `old` with `new` in a tag list, preserving order and
/// de-duplicating: if `new` is already present the `old` entry is simply
/// dropped (a merge). Returns `true` if the list changed.
pub fn rename_tag_in_list(tags: &mut Vec<String>, old: &str, new: &str) -> bool {
    if !tags.iter().any(|t| t == old) {
        return false;
    }
    let mut out: Vec<String> = Vec::with_capacity(tags.len());
    for t in tags.iter() {
        let mapped = if t == old { new } else { t.as_str() };
        if !out.iter().any(|x| x == mapped) {
            out.push(mapped.to_string());
        }
    }
    let changed = out != *tags;
    *tags = out;
    changed
}

/// Move `tag` to the front (the `stats --by-category` primary position) of a
/// tag list. Returns `true` if it was present and not already first.
pub fn promote_tag_in_list(tags: &mut Vec<String>, tag: &str) -> bool {
    match tags.iter().position(|t| t == tag) {
        Some(0) | None => false,
        Some(pos) => {
            let t = tags.remove(pos);
            tags.insert(0, t);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_tag_in_list_replaces_preserving_order() {
        let mut t = vec!["a".to_string(), "workfood".to_string(), "b".to_string()];
        assert!(rename_tag_in_list(&mut t, "workfood", "food"));
        assert_eq!(t, vec!["a", "food", "b"]);
    }

    #[test]
    fn rename_tag_in_list_dedups_when_new_already_present() {
        let mut t = vec![
            "subscription".to_string(),
            "subscriptions".to_string(),
            "digital".to_string(),
        ];
        assert!(rename_tag_in_list(&mut t, "subscriptions", "subscription"));
        assert_eq!(t, vec!["subscription", "digital"]);
    }

    #[test]
    fn rename_tag_in_list_noop_when_absent() {
        let mut t = vec!["food".to_string()];
        assert!(!rename_tag_in_list(&mut t, "workfood", "food"));
        assert_eq!(t, vec!["food"]);
    }

    #[test]
    fn promote_tag_moves_to_front() {
        let mut t = vec![
            "subscription".to_string(),
            "transport".to_string(),
            "taxi".to_string(),
        ];
        assert!(promote_tag_in_list(&mut t, "transport"));
        assert_eq!(t, vec!["transport", "subscription", "taxi"]);
    }

    #[test]
    fn promote_tag_noop_when_already_primary_or_absent() {
        let mut already = vec!["transport".to_string(), "taxi".to_string()];
        assert!(!promote_tag_in_list(&mut already, "transport"));
        let mut absent = vec!["food".to_string()];
        assert!(!promote_tag_in_list(&mut absent, "transport"));
    }

    #[test]
    fn rename_tag_scoped_by_merchant_leaves_others() {
        let mut rules = TagRules::default();
        rules.add_rule(
            "BT GROUP",
            vec!["bills".into(), "phone".into()],
            None, None, None, None, None,
        );
        rules.add_rule(
            "VODAFONE",
            vec!["bills".into(), "phone".into()],
            None, None, None, None, None,
        );
        assert_eq!(rules.rename_tag("phone", "internet", Some("BT GROUP")), 1);
        let bt = rules.rules.iter().find(|r| r.pattern == "BT GROUP").unwrap();
        assert_eq!(bt.tags, vec!["bills", "internet"]);
        let vf = rules.rules.iter().find(|r| r.pattern == "VODAFONE").unwrap();
        assert_eq!(vf.tags, vec!["bills", "phone"]);
    }
    use std::str::FromStr;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn test_rule_matching() {
        let mut rules = TagRules::default();
        rules.add_rule("TESCO", vec!["groceries".to_string(), "food".to_string()], None, None, None, None, None);
        rules.add_rule("PRET", vec!["food".to_string(), "lunch".to_string()], None, None, None, None, None);

        let amount = Decimal::from_str("-10.00").unwrap();
        let d = date(2025, 1, 15);

        let tags = rules.get_tags("TESCO STORES 1234 LONDON", amount, d);
        assert!(tags.contains(&"groceries".to_string()));
        assert!(tags.contains(&"food".to_string()));

        let tags = rules.get_tags("PRET A MANGER London", amount, d);
        assert!(tags.contains(&"food".to_string()));
        assert!(tags.contains(&"lunch".to_string()));

        // No match
        let tags = rules.get_tags("NETFLIX.COM", amount, d);
        assert!(tags.is_empty());
    }

    #[test]
    fn test_case_insensitive() {
        let mut rules = TagRules::default();
        rules.add_rule("netflix", vec!["subscription".to_string()], None, None, None, None, None);

        let amount = Decimal::from_str("-10.00").unwrap();
        let tags = rules.get_tags("NETFLIX.COM LONDON", amount, date(2025, 1, 15));
        assert!(tags.contains(&"subscription".to_string()));
    }

    #[test]
    fn test_exact_amount_match() {
        let mut rules = TagRules::default();
        rules.add_rule(
            "****",
            vec!["income".to_string(), "salary".to_string()],
            Some(Decimal::from_str("10000").unwrap()),
            None,
            None,
            None,
            None,
        );

        let d = date(2025, 1, 15);

        // Matches: pattern + exact amount
        let tags = rules.get_tags("****", Decimal::from_str("10000").unwrap(), d);
        assert!(tags.contains(&"income".to_string()));
        assert!(tags.contains(&"salary".to_string()));

        // No match: wrong amount
        let tags = rules.get_tags("****", Decimal::from_str("5000").unwrap(), d);
        assert!(tags.is_empty());

        // No match: wrong description
        let tags = rules.get_tags("TESCO", Decimal::from_str("10000").unwrap(), d);
        assert!(tags.is_empty());
    }

    #[test]
    fn test_amount_range_match() {
        let mut rules = TagRules::default();
        rules.add_rule(
            "****",
            vec!["large-transfer".to_string()],
            None,
            Some(Decimal::from_str("5000").unwrap()),
            Some(Decimal::from_str("15000").unwrap()),
            None,
            None,
        );

        let d = date(2025, 1, 15);

        // In range
        let tags = rules.get_tags("****", Decimal::from_str("10000").unwrap(), d);
        assert!(tags.contains(&"large-transfer".to_string()));

        // Below range
        let tags = rules.get_tags("****", Decimal::from_str("1000").unwrap(), d);
        assert!(tags.is_empty());

        // Above range
        let tags = rules.get_tags("****", Decimal::from_str("20000").unwrap(), d);
        assert!(tags.is_empty());
    }

    #[test]
    fn test_day_of_month_exact() {
        let mut rules = TagRules::default();
        rules.add_rule(
            "****",
            vec!["salary".to_string()],
            Some(Decimal::from_str("10000").unwrap()),
            None,
            None,
            Some(28),
            None, // window defaults to 0
        );

        let amount = Decimal::from_str("10000").unwrap();

        // Exact day match
        let tags = rules.get_tags("****", amount, date(2025, 1, 28));
        assert!(tags.contains(&"salary".to_string()));

        // Wrong day
        let tags = rules.get_tags("****", amount, date(2025, 1, 15));
        assert!(tags.is_empty());
    }

    #[test]
    fn test_day_of_month_with_window() {
        let mut rules = TagRules::default();
        rules.add_rule(
            "****",
            vec!["salary".to_string()],
            None,
            None,
            None,
            Some(28),
            Some(3),
        );

        let amount = Decimal::ZERO;

        // Within window: day 25 (diff=3)
        let tags = rules.get_tags("****", amount, date(2025, 1, 25));
        assert!(tags.contains(&"salary".to_string()));

        // Within window: day 31 (diff=3)
        let tags = rules.get_tags("****", amount, date(2025, 1, 31));
        assert!(tags.contains(&"salary".to_string()));

        // Outside window: day 24 (diff=4)
        let tags = rules.get_tags("****", amount, date(2025, 1, 24));
        assert!(tags.is_empty());
    }

    #[test]
    fn test_day_of_month_wrapping() {
        let mut rules = TagRules::default();
        rules.add_rule(
            "****",
            vec!["salary".to_string()],
            None,
            None,
            None,
            Some(1),
            Some(2),
        );

        let amount = Decimal::ZERO;

        // Day 1 — exact match
        let tags = rules.get_tags("****", amount, date(2025, 1, 1));
        assert!(tags.contains(&"salary".to_string()));

        // Day 30 in a 31-day month — circular diff = min(29, 31-29) = 2
        let tags = rules.get_tags("****", amount, date(2025, 1, 30));
        assert!(tags.contains(&"salary".to_string()));

        // Day 31 — circular diff = min(30, 31-30) = 1
        let tags = rules.get_tags("****", amount, date(2025, 1, 31));
        assert!(tags.contains(&"salary".to_string()));

        // Day 28 in a 31-day month — circular diff = min(27, 31-27) = 4
        let tags = rules.get_tags("****", amount, date(2025, 1, 28));
        assert!(tags.is_empty());
    }

    #[test]
    fn test_day_of_month_clamped_to_short_month() {
        // day_of_month=31 in February should clamp to 28
        let mut rules = TagRules::default();
        rules.add_rule(
            "****",
            vec!["end-of-month".to_string()],
            None,
            None,
            None,
            Some(31),
            Some(1),
        );

        let amount = Decimal::ZERO;

        // Feb 28 — target clamped to 28, diff=0
        let tags = rules.get_tags("****", amount, date(2025, 2, 28));
        assert!(tags.contains(&"end-of-month".to_string()));

        // Feb 27 — target clamped to 28, diff=1
        let tags = rules.get_tags("****", amount, date(2025, 2, 27));
        assert!(tags.contains(&"end-of-month".to_string()));

        // Feb 15 — target clamped to 28, diff=13
        let tags = rules.get_tags("****", amount, date(2025, 2, 15));
        assert!(tags.is_empty());
    }
}
