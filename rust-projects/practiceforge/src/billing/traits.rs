//! Provider traits for pluggable billing backends.

use anyhow::Result;

use super::invoice::{Invoice, InvoiceState, InvoiceRef};

/// Where invoices live — the accounting ledger.
///
/// Implementations: ManualProvider (file-based), future Xero/QuickBooks.
pub trait AccountingProvider: Send + Sync {
    /// Create an invoice in the ledger. Returns a reference for tracking.
    fn create_invoice(&self, invoice: &Invoice) -> Result<InvoiceRef>;

    /// Get the current state of an invoice by reference.
    fn get_invoice(&self, reference: &str) -> Result<Option<InvoiceSummary>>;

    /// List all invoices matching a filter.
    fn list_invoices(&self, filter: InvoiceFilter) -> Result<Vec<InvoiceSummary>>;

    /// Mark an invoice as paid.
    fn mark_paid(&self, reference: &str, date: &str, amount: Option<f64>) -> Result<()>;

    /// Mark an invoice as cancelled/void.
    fn cancel_invoice(&self, reference: &str, reason: &str) -> Result<()>;

    /// Get the next invoice number (auto-incrementing).
    fn next_invoice_number(&self) -> Result<String>;
}

/// How self-pay clients pay.
///
/// Implementations: ManualProvider (no automation), future Stripe/GoCardless.
pub trait PaymentProvider: Send + Sync {
    /// Create a payment link for an invoice. Returns None if the provider
    /// doesn't support payment links (e.g. Manual = bank transfer).
    fn create_payment_link(&self, invoice: &Invoice) -> Result<Option<String>>;

    /// Provider name for display (e.g. "Stripe", "Manual (bank transfer)").
    fn provider_name(&self) -> &str;
}

/// Summary of an invoice's current state, returned by list/get operations.
#[derive(Debug, Clone)]
pub struct InvoiceSummary {
    pub reference: String,
    pub client_id: String,
    pub client_name: String,
    pub bill_to_name: String,
    pub bill_to_email: Option<String>,
    pub total: f64,
    pub currency: String,
    pub issue_date: String,
    pub due_date: String,
    pub state: InvoiceState,
    pub days_overdue: i64,
    pub payment_link: Option<String>,
    pub reminders_sent: u32,
    pub last_reminder: Option<String>,
}

/// Filter for listing invoices.
#[derive(Debug, Clone, Default)]
pub struct InvoiceFilter {
    pub client_id: Option<String>,
    pub state: Option<InvoiceState>,
    pub overdue_only: bool,
}
