//! Reminder system — template-based, tone-aware payment reminders.
//!
//! Four tone presets: sensitive, tentative, businesslike, assertive.
//! Configurable escalation sequence per practitioner.
//! Templates are deterministic (no inference).

use super::config::BillingConfig;
use super::traits::InvoiceSummary;

/// A rendered reminder ready to send.
#[derive(Debug, Clone)]
pub struct Reminder {
    pub subject: String,
    pub body: String,
    pub tone: String,
    pub to_email: Option<String>,
    pub to_name: String,
    pub invoice_reference: String,
}

/// Determine which reminder stage (if any) applies to an overdue invoice.
///
/// Returns the tone for the next unsent reminder, or None if all reminders
/// have been sent or the invoice isn't overdue enough.
pub fn next_reminder_tone(
    config: &BillingConfig,
    invoice: &InvoiceSummary,
) -> Option<String> {
    let sent = invoice.reminders_sent as usize;

    // Find the first reminder stage where days_overdue >= threshold
    // and that stage hasn't been sent yet.
    for (i, &threshold) in config.reminder_days.iter().enumerate() {
        if i < sent {
            continue; // already sent this stage
        }
        if invoice.days_overdue >= threshold {
            let tone = config
                .reminder_tones
                .get(i)
                .cloned()
                .unwrap_or_else(|| "businesslike".to_string());
            return Some(tone);
        }
    }

    None
}

/// Render a reminder for a self-pay client.
pub fn render_client_reminder(
    invoice: &InvoiceSummary,
    tone: &str,
    practitioner_name: &str,
    payment_details: &str,
) -> Reminder {
    let (subject, body) = match tone {
        "sensitive" => render_sensitive(invoice, practitioner_name, payment_details),
        "tentative" => render_tentative(invoice, practitioner_name, payment_details),
        "assertive" => render_assertive(invoice, practitioner_name, payment_details),
        _ => render_businesslike(invoice, practitioner_name, payment_details),
    };

    Reminder {
        subject,
        body,
        tone: tone.to_string(),
        to_email: invoice.payment_link.clone(), // will be populated from BillTo
        to_name: invoice.client_name.clone(),
        invoice_reference: invoice.reference.clone(),
    }
}

/// Render a reminder for an insurer.
/// Insurers always get businesslike tone regardless of config.
pub fn render_insurer_reminder(
    invoice: &InvoiceSummary,
    practitioner_name: &str,
) -> Reminder {
    let subject = format!(
        "Outstanding invoice {} — {}",
        invoice.reference, invoice.client_name
    );

    let body = format!(
        "Dear {},\n\n\
         For your records, invoice {} (issued {}) for {} {} \
         in respect of {} remains outstanding.\n\n\
         The invoice was due on {} and is now {} days overdue.\n\n\
         {}Please arrange payment at your earliest convenience.\n\n\
         Kind regards,\n{}",
        invoice.bill_to_name,
        invoice.reference,
        invoice.issue_date,
        invoice.currency,
        format_amount(invoice.total),
        invoice.client_name,
        invoice.due_date,
        invoice.days_overdue,
        policy_line(invoice),
        practitioner_name,
    );

    Reminder {
        subject,
        body,
        tone: "businesslike".to_string(),
        to_email: None, // populated from BillTo.email
        to_name: invoice.bill_to_name.clone(),
        invoice_reference: invoice.reference.clone(),
    }
}

// ---------------------------------------------------------------------------
// Tone templates — self-pay clients
// ---------------------------------------------------------------------------

fn render_sensitive(
    inv: &InvoiceSummary,
    practitioner: &str,
    payment_details: &str,
) -> (String, String) {
    let subject = format!("A note about your account — {}", inv.reference);
    let body = format!(
        "Dear {},\n\n\
         I wanted to check in regarding invoice {} (issued {} for {} {}).\n\n\
         I appreciate that these things can sometimes slip through, \
         and there may be circumstances I'm not aware of. \
         If there are any difficulties, please don't hesitate to let me know — \
         I'm happy to discuss.\n\n\
         {}\n\n\
         With warm regards,\n{}",
        inv.client_name,
        inv.reference,
        inv.issue_date,
        inv.currency,
        format_amount(inv.total),
        payment_section(payment_details, &inv.payment_link),
        practitioner,
    );
    (subject, body)
}

fn render_tentative(
    inv: &InvoiceSummary,
    practitioner: &str,
    payment_details: &str,
) -> (String, String) {
    let subject = format!("Invoice {} — gentle reminder", inv.reference);
    let body = format!(
        "Dear {},\n\n\
         I'm conscious this may have already been attended to, but I wanted \
         to flag that invoice {} ({} {}, issued {}) appears to still be outstanding. \
         It was due on {}.\n\n\
         If payment is already on its way, please disregard this message.\n\n\
         {}\n\n\
         Kind regards,\n{}",
        inv.client_name,
        inv.reference,
        inv.currency,
        format_amount(inv.total),
        inv.issue_date,
        inv.due_date,
        payment_section(payment_details, &inv.payment_link),
        practitioner,
    );
    (subject, body)
}

fn render_businesslike(
    inv: &InvoiceSummary,
    practitioner: &str,
    payment_details: &str,
) -> (String, String) {
    let subject = format!("Payment reminder — invoice {}", inv.reference);
    let body = format!(
        "Dear {},\n\n\
         This is a reminder that invoice {} for {} {} \
         (issued {}) remains unpaid. The payment was due on {} \
         and is now {} days overdue.\n\n\
         {}\n\n\
         Please arrange payment at your earliest convenience.\n\n\
         Kind regards,\n{}",
        inv.client_name,
        inv.reference,
        inv.currency,
        format_amount(inv.total),
        inv.issue_date,
        inv.due_date,
        inv.days_overdue,
        payment_section(payment_details, &inv.payment_link),
        practitioner,
    );
    (subject, body)
}

fn render_assertive(
    inv: &InvoiceSummary,
    practitioner: &str,
    payment_details: &str,
) -> (String, String) {
    let subject = format!("Urgent: overdue invoice {}", inv.reference);
    let body = format!(
        "Dear {},\n\n\
         Invoice {} for {} {} (issued {}) is now {} days overdue \
         and requires immediate attention.\n\n\
         Previous reminders have been sent regarding this balance. \
         I would appreciate your prompt response to arrange payment.\n\n\
         {}\n\n\
         If there are circumstances preventing payment, please contact me \
         directly so we can discuss a way forward.\n\n\
         Regards,\n{}",
        inv.client_name,
        inv.reference,
        inv.currency,
        format_amount(inv.total),
        inv.issue_date,
        inv.days_overdue,
        payment_section(payment_details, &inv.payment_link),
        practitioner,
    );
    (subject, body)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn format_amount(amount: f64) -> String {
    format!("{:.2}", amount)
}

fn payment_section(bank_details: &str, payment_link: &Option<String>) -> String {
    let mut section = String::new();
    if let Some(link) = payment_link {
        section.push_str(&format!("You can pay online at: {}\n\n", link));
    }
    if !bank_details.is_empty() {
        section.push_str(&format!("Bank transfer details:\n{}", bank_details));
    }
    if section.is_empty() {
        "Please contact me for payment arrangements.".to_string()
    } else {
        section
    }
}

fn policy_line(_inv: &InvoiceSummary) -> String {
    // Would include policy number for insurers, but we don't store it in the summary.
    // The caller should enrich this from identity.yaml if needed.
    String::new()
}

/// List all invoices that are due a reminder, with their recommended tone.
pub fn due_reminders(
    config: &BillingConfig,
    overdue_invoices: &[InvoiceSummary],
) -> Vec<(InvoiceSummary, String)> {
    overdue_invoices
        .iter()
        .filter_map(|inv| {
            next_reminder_tone(config, inv).map(|tone| (inv.clone(), tone))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::invoice::InvoiceState;

    fn sample_summary(days_overdue: i64, reminders_sent: u32) -> InvoiceSummary {
        InvoiceSummary {
            reference: "INV-2026-0001".to_string(),
            client_id: "JB92".to_string(),
            client_name: "Jane Bloggs".to_string(),
            bill_to_name: "Jane Bloggs".to_string(),
            total: 198.0,
            currency: "GBP".to_string(),
            issue_date: "2026-04-01".to_string(),
            due_date: "2026-04-15".to_string(),
            state: InvoiceState::Overdue,
            days_overdue,
            payment_link: None,
            reminders_sent,
            last_reminder: None,
        }
    }

    #[test]
    fn test_first_reminder_at_7_days() {
        let config = BillingConfig::default();
        let inv = sample_summary(7, 0);
        let tone = next_reminder_tone(&config, &inv);
        assert_eq!(tone, Some("sensitive".to_string()));
    }

    #[test]
    fn test_second_reminder_at_14_days() {
        let config = BillingConfig::default();
        let inv = sample_summary(14, 1);
        let tone = next_reminder_tone(&config, &inv);
        assert_eq!(tone, Some("businesslike".to_string()));
    }

    #[test]
    fn test_third_reminder_at_28_days() {
        let config = BillingConfig::default();
        let inv = sample_summary(28, 2);
        let tone = next_reminder_tone(&config, &inv);
        assert_eq!(tone, Some("assertive".to_string()));
    }

    #[test]
    fn test_no_reminder_when_all_sent() {
        let config = BillingConfig::default();
        let inv = sample_summary(30, 3);
        let tone = next_reminder_tone(&config, &inv);
        assert_eq!(tone, None);
    }

    #[test]
    fn test_no_reminder_when_not_overdue_enough() {
        let config = BillingConfig::default();
        let inv = sample_summary(3, 0);
        let tone = next_reminder_tone(&config, &inv);
        assert_eq!(tone, None);
    }

    #[test]
    fn test_render_sensitive_tone() {
        let inv = sample_summary(7, 0);
        let reminder = render_client_reminder(&inv, "sensitive", "Dr Smith", "Sort: 12-34-56\nAcct: 87654321");
        assert!(reminder.body.contains("slip through"));
        assert!(reminder.body.contains("Dr Smith"));
        assert_eq!(reminder.tone, "sensitive");
    }

    #[test]
    fn test_render_assertive_tone() {
        let inv = sample_summary(28, 2);
        let reminder = render_client_reminder(&inv, "assertive", "Dr Smith", "");
        assert!(reminder.body.contains("immediate attention"));
        assert!(reminder.body.contains("28 days overdue"));
    }

    #[test]
    fn test_insurer_always_businesslike() {
        let mut inv = sample_summary(14, 0);
        inv.bill_to_name = "AXA Health".to_string();
        let reminder = render_insurer_reminder(&inv, "Dr Smith");
        assert_eq!(reminder.tone, "businesslike");
        assert!(reminder.body.contains("For your records"));
    }
}
