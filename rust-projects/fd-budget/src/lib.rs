pub mod dedup;
pub mod enrich;
pub mod import;
pub mod paypal;
pub mod query;
pub mod store;
pub mod tags;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Account {
    Current,
    Visa,
}

impl std::fmt::Display for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Account::Current => write!(f, "current"),
            Account::Visa => write!(f, "visa"),
        }
    }
}

impl std::str::FromStr for Account {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "current" => Ok(Account::Current),
            "visa" => Ok(Account::Visa),
            _ => Err(format!("unknown account: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxType {
    Contactless,   // )))
    Mastercard,    // MAS
    DirectDebit,   // DD
    BankPayment,   // BP
    StandingOrder, // SO
    Transfer,      // TFR
    Atm,           // ATM
    Unknown(u8),   // Fallback
}

impl TxType {
    pub fn from_code(code: &str) -> Self {
        match code.trim() {
            ")))" => TxType::Contactless,
            "MAS" => TxType::Mastercard,
            "DD" => TxType::DirectDebit,
            "BP" => TxType::BankPayment,
            "SO" => TxType::StandingOrder,
            "TFR" => TxType::Transfer,
            "ATM" => TxType::Atm,
            _ => TxType::Unknown(0),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            TxType::Contactless => "contactless",
            TxType::Mastercard => "mastercard",
            TxType::DirectDebit => "direct_debit",
            TxType::BankPayment => "bank_payment",
            TxType::StandingOrder => "standing_order",
            TxType::Transfer => "transfer",
            TxType::Atm => "atm",
            TxType::Unknown(_) => "unknown",
        }
    }
}

impl std::fmt::Display for TxType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub date: NaiveDate,
    pub account: Account,
    pub tx_type: TxType,
    pub amount: Decimal,
    pub description: String,
    pub raw_description: String,
    pub balance: Option<Decimal>,
    pub tags: Vec<String>,
    pub import_id: String,
}

impl Transaction {
    /// Tags that mark a row as NOT part of the recurring **personal** spend
    /// floor: internal transfers, income, tax payments, one-off / lumpy
    /// discretionary outgoings, and **business** / professional costs (practice
    /// expenses funded before the personal draw — PA fees, conference fees, etc.;
    /// not personal living cost). Stripped from the floor so the recurring
    /// personal-cost figure is trustworthy (the figure the forward "pots" layer
    /// allocates against). Matched case-insensitively. Rides the existing `tags`
    /// column — no schema change.
    /// `fdvisa` = a payment to the First Direct Visa card: an internal transfer,
    /// excluded because the Visa *purchases* are itemised separately (counting
    /// the payment too would double-count).
    pub const NONSPEND_TAGS: &'static [&'static str] =
        &["transfer", "income", "tax", "one-off", "business", "fdvisa"];

    pub fn is_debit(&self) -> bool {
        self.amount.is_sign_negative()
    }

    pub fn is_credit(&self) -> bool {
        self.amount.is_sign_positive()
    }

    /// True if the row carries any reserved non-spend tag (case-insensitive).
    pub fn is_nonspend(&self) -> bool {
        self.tags.iter().any(|t| {
            Self::NONSPEND_TAGS
                .iter()
                .any(|n| t.eq_ignore_ascii_case(n))
        })
    }

    /// True if the row is a business / professional cost (tag "business",
    /// case-insensitive). A subset of `is_nonspend` — broken out so reporting
    /// can show professional costs as their own line.
    pub fn is_business(&self) -> bool {
        self.tags.iter().any(|t| t.eq_ignore_ascii_case("business"))
    }

    /// True when the row is recurring personal spend: a debit not carrying a
    /// non-spend tag. Credits (income/refunds) and non-spend-tagged debits
    /// (transfers, tax, one-offs, business) are excluded.
    pub fn counts_as_spend(&self) -> bool {
        self.is_debit() && !self.is_nonspend()
    }
}

#[cfg(test)]
mod transaction_tests {
    use super::*;
    use std::str::FromStr;

    fn tx(amount: &str, tags: &[&str]) -> Transaction {
        Transaction {
            date: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            account: Account::Current,
            tx_type: TxType::Contactless,
            amount: Decimal::from_str(amount).unwrap(),
            description: "x".into(),
            raw_description: "x".into(),
            balance: None,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            import_id: "id".into(),
        }
    }

    #[test]
    fn debit_without_nonspend_tag_counts_as_spend() {
        assert!(tx("-12.50", &[]).counts_as_spend());
        assert!(tx("-12.50", &["groceries"]).counts_as_spend());
    }

    #[test]
    fn credit_is_never_spend() {
        assert!(!tx("100.00", &[]).counts_as_spend());
        assert!(!tx("100.00", &["income"]).counts_as_spend());
    }

    #[test]
    fn nonspend_tagged_debit_is_excluded() {
        for t in Transaction::NONSPEND_TAGS {
            assert!(
                !tx("-500.00", &[t]).counts_as_spend(),
                "tag {t} should exclude"
            );
            assert!(tx("-500.00", &[t]).is_nonspend());
        }
    }

    #[test]
    fn nonspend_match_is_case_insensitive() {
        assert!(tx("-500.00", &["Transfer"]).is_nonspend());
        assert!(tx("-500.00", &["TAX"]).is_nonspend());
    }

    #[test]
    fn one_off_excludes_lumpy_discretionary() {
        // e.g. a gym block / AWS — tagged one-off, dropped from the spend floor.
        let lump = tx("-2550.00", &["one-off", "gym"]);
        assert!(lump.is_nonspend());
        assert!(!lump.counts_as_spend());
    }

    #[test]
    fn business_is_excluded_from_personal_floor() {
        // Professional cost (e.g. PA fees, conference) — not personal living.
        let b = tx("-370.00", &["business"]);
        assert!(b.is_business());
        assert!(b.is_nonspend());
        assert!(!b.counts_as_spend());
        // case-insensitive
        assert!(tx("-1.00", &["Business"]).is_business());
    }
}
