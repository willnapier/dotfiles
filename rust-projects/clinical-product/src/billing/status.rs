//! Status display — show outstanding and overdue invoices.

use super::invoice::InvoiceState;
use super::traits::{AccountingProvider, InvoiceFilter};
use anyhow::Result;

/// Display all outstanding invoices.
pub fn show_status(provider: &dyn AccountingProvider, overdue_only: bool) -> Result<()> {
    let filter = InvoiceFilter {
        overdue_only,
        ..Default::default()
    };

    let invoices = provider.list_invoices(filter)?;

    if invoices.is_empty() {
        if overdue_only {
            println!("No overdue invoices.");
        } else {
            println!("No outstanding invoices.");
        }
        return Ok(());
    }

    // Header
    println!(
        "{:<16} {:<8} {:<20} {:<10} {:>10} {:>8}  {}",
        "REFERENCE", "CLIENT", "BILL TO", "STATUS", "AMOUNT", "OVERDUE", "DUE"
    );
    println!("{}", "-".repeat(90));

    let mut total_outstanding = 0.0;
    let mut total_overdue = 0.0;

    for inv in &invoices {
        let status_display = match &inv.state {
            InvoiceState::Draft => "draft",
            InvoiceState::Sent => "sent",
            InvoiceState::Overdue => "OVERDUE",
            InvoiceState::Paid => "paid",
            InvoiceState::Cancelled => "cancelled",
        };

        let overdue_display = if inv.days_overdue > 0 {
            format!("{}d", inv.days_overdue)
        } else {
            String::new()
        };

        println!(
            "{:<16} {:<8} {:<20} {:<10} {:>7} {:>3} {:>8}  {}",
            truncate(&inv.reference, 16),
            inv.client_id,
            truncate(&inv.bill_to_name, 20),
            status_display,
            inv.currency,
            format!("{:.0}", inv.total),
            overdue_display,
            inv.due_date,
        );

        if inv.state != InvoiceState::Paid && inv.state != InvoiceState::Cancelled {
            total_outstanding += inv.total;
        }
        if inv.state == InvoiceState::Overdue {
            total_overdue += inv.total;
        }
    }

    println!("{}", "-".repeat(90));
    println!(
        "  {} invoice(s) shown. Outstanding: {:.2}",
        invoices.len(),
        total_outstanding
    );
    if total_overdue > 0.0 {
        println!("  Overdue: {:.2}", total_overdue);
    }

    Ok(())
}

/// Display a compact summary suitable for DayPage or quick check.
pub fn compact_summary(provider: &dyn AccountingProvider) -> Result<String> {
    let all = provider.list_invoices(InvoiceFilter::default())?;

    let outstanding: Vec<_> = all
        .iter()
        .filter(|i| i.state != InvoiceState::Paid && i.state != InvoiceState::Cancelled)
        .collect();

    let overdue: Vec<_> = outstanding
        .iter()
        .filter(|i| i.state == InvoiceState::Overdue)
        .collect();

    let total_outstanding: f64 = outstanding.iter().map(|i| i.total).sum();
    let total_overdue: f64 = overdue.iter().map(|i| i.total).sum();

    let mut summary = format!(
        "{} outstanding ({:.0})",
        outstanding.len(),
        total_outstanding
    );

    if !overdue.is_empty() {
        summary.push_str(&format!(
            ", {} overdue ({:.0})",
            overdue.len(),
            total_overdue
        ));
    }

    Ok(summary)
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n.saturating_sub(1)])
    }
}
