pub mod import;
pub mod store;
pub mod tags;
pub mod dedup;

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
    Contactless,    // )))
    Mastercard,     // MAS
    DirectDebit,    // DD
    BankPayment,    // BP
    StandingOrder,  // SO
    Transfer,       // TFR
    Atm,            // ATM
    Unknown(u8),    // Fallback
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
    pub fn is_debit(&self) -> bool {
        self.amount.is_sign_negative()
    }

    pub fn is_credit(&self) -> bool {
        self.amount.is_sign_positive()
    }
}
