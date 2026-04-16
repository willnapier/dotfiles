//! Manual billing backend — file-based invoice storage, no API keys.
//!
//! This is the baseline provider that works for every practitioner
//! without any external service configuration. Invoices are stored
//! as JSON files in a local directory.

use anyhow::{bail, Context, Result};
use chrono::{Local, NaiveDate};
use std::fs;
use std::path::PathBuf;

use super::config::BillingConfig;
use super::invoice::{Invoice, InvoiceRef, InvoiceState};
use super::traits::{AccountingProvider, InvoiceFilter, InvoiceSummary, PaymentProvider};

/// File-based billing provider. Stores invoices as JSON in a local directory.
pub struct ManualProvider {
    storage_dir: PathBuf,
}

impl ManualProvider {
    pub fn new(config: &BillingConfig) -> Result<Self> {
        let storage_dir = config.resolve_storage_dir();
        fs::create_dir_all(&storage_dir)
            .with_context(|| format!("Cannot create billing dir: {}", storage_dir.display()))?;
        Ok(Self { storage_dir })
    }

    /// Path to the invoice index file.
    fn index_path(&self) -> PathBuf {
        self.storage_dir.join("invoices.json")
    }

    /// Load the invoice index (all invoices).
    fn load_index(&self) -> Result<Vec<Invoice>> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let data = fs::read_to_string(&path)
            .with_context(|| format!("Cannot read {}", path.display()))?;
        let invoices: Vec<Invoice> =
            serde_json::from_str(&data).context("Failed to parse invoice index")?;
        Ok(invoices)
    }

    /// Save the invoice index.
    fn save_index(&self, invoices: &[Invoice]) -> Result<()> {
        let path = self.index_path();
        let data = serde_json::to_string_pretty(invoices)?;
        fs::write(&path, data)
            .with_context(|| format!("Cannot write {}", path.display()))?;
        Ok(())
    }

    /// Convert an Invoice to an InvoiceSummary with computed overdue days.
    fn to_summary(invoice: &Invoice) -> InvoiceSummary {
        let today = Local::now().date_naive();
        let due = NaiveDate::parse_from_str(&invoice.due_date, "%Y-%m-%d")
            .unwrap_or(today);
        let days_overdue = if today > due {
            (today - due).num_days()
        } else {
            0
        };

        // Auto-promote sent → overdue if past due date
        let effective_state = if invoice.state == InvoiceState::Sent && days_overdue > 0 {
            InvoiceState::Overdue
        } else {
            invoice.state.clone()
        };

        InvoiceSummary {
            reference: invoice.reference.clone(),
            client_id: invoice.client_id.clone(),
            client_name: invoice.client_name.clone(),
            bill_to_name: invoice.bill_to.display_name().to_string(),
            total: invoice.total(),
            currency: invoice.currency.clone(),
            issue_date: invoice.issue_date.clone(),
            due_date: invoice.due_date.clone(),
            state: effective_state,
            days_overdue,
            payment_link: invoice.payment_link.clone(),
            reminders_sent: 0, // TODO: track from reminder log
            last_reminder: None,
        }
    }

    /// Get invoiced session dates for a specific client.
    /// Used to determine which sessions still need invoicing.
    pub fn invoiced_dates_for_client(&self, client_id: &str) -> Result<Vec<String>> {
        let invoices = self.load_index()?;
        let dates: Vec<String> = invoices
            .iter()
            .filter(|inv| inv.client_id == client_id && inv.state != InvoiceState::Cancelled)
            .flat_map(|inv| inv.line_items.iter().map(|li| li.session_date.clone()))
            .collect();
        Ok(dates)
    }
}

impl AccountingProvider for ManualProvider {
    fn create_invoice(&self, invoice: &Invoice) -> Result<InvoiceRef> {
        let mut invoices = self.load_index()?;

        // Check for duplicate reference
        if invoices.iter().any(|i| i.reference == invoice.reference) {
            bail!("Invoice {} already exists", invoice.reference);
        }

        let reference = invoice.reference.clone();
        invoices.push(invoice.clone());
        self.save_index(&invoices)?;

        // Also write individual invoice file for easy access
        let invoice_file = self
            .storage_dir
            .join(format!("{}.json", reference));
        let data = serde_json::to_string_pretty(invoice)?;
        fs::write(&invoice_file, data)?;

        Ok(InvoiceRef {
            reference,
            file_path: Some(invoice_file.to_string_lossy().to_string()),
        })
    }

    fn get_invoice(&self, reference: &str) -> Result<Option<InvoiceSummary>> {
        let invoices = self.load_index()?;
        Ok(invoices
            .iter()
            .find(|i| i.reference == reference)
            .map(Self::to_summary))
    }

    fn list_invoices(&self, filter: InvoiceFilter) -> Result<Vec<InvoiceSummary>> {
        let invoices = self.load_index()?;
        let summaries: Vec<InvoiceSummary> = invoices
            .iter()
            .map(Self::to_summary)
            .filter(|s| {
                if let Some(ref cid) = filter.client_id {
                    if &s.client_id != cid {
                        return false;
                    }
                }
                if let Some(ref state) = filter.state {
                    if &s.state != state {
                        return false;
                    }
                }
                if filter.overdue_only && s.state != InvoiceState::Overdue {
                    return false;
                }
                true
            })
            .collect();
        Ok(summaries)
    }

    fn mark_paid(&self, reference: &str, date: &str, _amount: Option<f64>) -> Result<()> {
        let mut invoices = self.load_index()?;
        let inv = invoices
            .iter_mut()
            .find(|i| i.reference == reference)
            .ok_or_else(|| anyhow::anyhow!("Invoice {} not found", reference))?;

        inv.state = InvoiceState::Paid;
        inv.notes = Some(format!(
            "{}Paid on {}",
            inv.notes.as_deref().map(|n| format!("{}\n", n)).unwrap_or_default(),
            date
        ));

        self.save_index(&invoices)?;
        Ok(())
    }

    fn cancel_invoice(&self, reference: &str, reason: &str) -> Result<()> {
        let mut invoices = self.load_index()?;
        let inv = invoices
            .iter_mut()
            .find(|i| i.reference == reference)
            .ok_or_else(|| anyhow::anyhow!("Invoice {} not found", reference))?;

        inv.state = InvoiceState::Cancelled;
        inv.notes = Some(format!(
            "{}Cancelled: {}",
            inv.notes.as_deref().map(|n| format!("{}\n", n)).unwrap_or_default(),
            reason
        ));

        self.save_index(&invoices)?;
        Ok(())
    }

    fn next_invoice_number(&self) -> Result<String> {
        let invoices = self.load_index()?;
        let year = Local::now().format("%Y");

        // Find the highest number for this year
        let prefix = format!("INV-{}-", year);
        let max_num = invoices
            .iter()
            .filter(|i| i.reference.starts_with(&prefix))
            .filter_map(|i| {
                i.reference
                    .strip_prefix(&prefix)
                    .and_then(|s| s.parse::<u32>().ok())
            })
            .max()
            .unwrap_or(0);

        Ok(format!("{}{:04}", prefix, max_num + 1))
    }
}

impl PaymentProvider for ManualProvider {
    fn create_payment_link(&self, _invoice: &Invoice) -> Result<Option<String>> {
        // Manual backend: no payment links. Client pays via bank transfer.
        Ok(None)
    }

    fn provider_name(&self) -> &str {
        "Manual (bank transfer)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::invoice::{BillTo, LineItem};
    use std::path::Path;
    use tempfile::TempDir;

    fn test_config(dir: &Path) -> BillingConfig {
        BillingConfig {
            storage_dir: Some(dir.to_string_lossy().to_string()),
            ..BillingConfig::default()
        }
    }

    fn sample_invoice(reference: &str) -> Invoice {
        Invoice {
            reference: reference.to_string(),
            client_id: "JB92".to_string(),
            client_name: "Jane Bloggs".to_string(),
            bill_to: BillTo::Client {
                name: "Jane Bloggs".to_string(),
                email: Some("jane@example.com".to_string()),
            },
            line_items: vec![LineItem {
                description: "Clinical psychology session (45 min) — 2026-04-01".to_string(),
                session_date: "2026-04-01".to_string(),
                quantity: 1,
                unit_amount: 198.0,
            }],
            issue_date: "2026-04-15".to_string(),
            due_date: "2026-04-29".to_string(),
            currency: "GBP".to_string(),
            state: InvoiceState::Draft,
            payment_link: None,
            notes: None,
        }
    }

    #[test]
    fn test_create_and_get_invoice() {
        let tmp = TempDir::new().unwrap();
        let provider = ManualProvider::new(&test_config(tmp.path())).unwrap();

        let inv = sample_invoice("INV-2026-0001");
        let result = provider.create_invoice(&inv).unwrap();
        assert_eq!(result.reference, "INV-2026-0001");

        let fetched = provider.get_invoice("INV-2026-0001").unwrap();
        assert!(fetched.is_some());
        let summary = fetched.unwrap();
        assert_eq!(summary.client_id, "JB92");
        assert_eq!(summary.total, 198.0);
    }

    #[test]
    fn test_duplicate_reference_rejected() {
        let tmp = TempDir::new().unwrap();
        let provider = ManualProvider::new(&test_config(tmp.path())).unwrap();

        let inv = sample_invoice("INV-2026-0001");
        provider.create_invoice(&inv).unwrap();
        let result = provider.create_invoice(&inv);
        assert!(result.is_err());
    }

    #[test]
    fn test_mark_paid() {
        let tmp = TempDir::new().unwrap();
        let provider = ManualProvider::new(&test_config(tmp.path())).unwrap();

        let inv = sample_invoice("INV-2026-0001");
        provider.create_invoice(&inv).unwrap();
        provider.mark_paid("INV-2026-0001", "2026-04-20", None).unwrap();

        let summary = provider.get_invoice("INV-2026-0001").unwrap().unwrap();
        assert_eq!(summary.state, InvoiceState::Paid);
    }

    #[test]
    fn test_next_invoice_number() {
        let tmp = TempDir::new().unwrap();
        let provider = ManualProvider::new(&test_config(tmp.path())).unwrap();

        let num1 = provider.next_invoice_number().unwrap();
        assert!(num1.ends_with("0001"));

        let inv = sample_invoice(&num1);
        provider.create_invoice(&inv).unwrap();

        let num2 = provider.next_invoice_number().unwrap();
        assert!(num2.ends_with("0002"));
    }

    #[test]
    fn test_list_overdue_only() {
        let tmp = TempDir::new().unwrap();
        let provider = ManualProvider::new(&test_config(tmp.path())).unwrap();

        // Create an invoice with a past due date
        let mut inv = sample_invoice("INV-2026-0001");
        inv.due_date = "2026-01-01".to_string(); // well in the past
        inv.state = InvoiceState::Sent;
        provider.create_invoice(&inv).unwrap();

        // Create a current invoice
        let mut inv2 = sample_invoice("INV-2026-0002");
        inv2.reference = "INV-2026-0002".to_string();
        inv2.due_date = "2099-12-31".to_string(); // far future
        inv2.state = InvoiceState::Sent;
        provider.create_invoice(&inv2).unwrap();

        let overdue = provider
            .list_invoices(InvoiceFilter {
                overdue_only: true,
                ..Default::default()
            })
            .unwrap();

        assert_eq!(overdue.len(), 1);
        assert_eq!(overdue[0].reference, "INV-2026-0001");
    }

    #[test]
    fn test_invoiced_dates_for_client() {
        let tmp = TempDir::new().unwrap();
        let provider = ManualProvider::new(&test_config(tmp.path())).unwrap();

        let inv = sample_invoice("INV-2026-0001");
        provider.create_invoice(&inv).unwrap();

        let dates = provider.invoiced_dates_for_client("JB92").unwrap();
        assert_eq!(dates, vec!["2026-04-01"]);

        let dates2 = provider.invoiced_dates_for_client("XX99").unwrap();
        assert!(dates2.is_empty());
    }
}
