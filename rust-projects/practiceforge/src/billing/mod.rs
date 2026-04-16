//! Billing module — vendor-neutral, per-practitioner billing automation.
//!
//! Architecture: pluggable backends via traits.
//! - `AccountingProvider`: where invoices live (Xero, QuickBooks, Manual)
//! - `PaymentProvider`: how self-pay clients pay (Stripe, GoCardless, Manual)
//!
//! The Manual backend is the baseline — works without API keys.
//! API backends are opt-in enhancements configured per-practitioner.

pub mod config;
pub mod invoice;
pub mod manual;
pub mod remind;
pub mod status;
pub mod traits;

pub use config::BillingConfig;
pub use invoice::{BillTo, Invoice, InvoiceState, LineItem};
pub use manual::{ManualProvider, ReminderLogEntry};
pub use traits::{AccountingProvider, PaymentProvider};
