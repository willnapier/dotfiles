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
pub mod invoice_render;
pub mod manual;
pub mod practitioner;
pub mod remind;
pub mod secrets;
pub mod sessions;
pub mod status;
pub mod stripe;
pub mod traits;
pub mod xero;

pub use config::BillingConfig;
pub use invoice::{BillTo, Invoice, InvoiceState, LineItem};
pub use manual::{ManualProvider, ReminderLogEntry};
pub use secrets::BillingSecrets;
pub use sessions::{billable_sessions_for_client, uninvoiced_billable, BillReason, BillableSession};
pub use stripe::StripeProvider;
pub use traits::{AccountingProvider, PaymentProvider};
pub use xero::XeroProvider;
