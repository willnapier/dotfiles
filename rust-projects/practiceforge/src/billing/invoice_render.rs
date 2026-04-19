//! Invoice HTML rendering and email delivery.
//!
//! Renders a professional HTML invoice on the practitioner's letterhead
//! and sends it via the configured SMTP server.
//!
//! No external template engine or PDF library — pure Rust string rendering.
//! HTML uses inline styles for maximum email-client compatibility.

use anyhow::{bail, Result};

use super::invoice::{BillTo, Invoice};
use super::practitioner::PractitionerConfig;
use crate::email::EmailConfig;

// ---------------------------------------------------------------------------
// HTML rendering
// ---------------------------------------------------------------------------

/// Render a complete invoice as a self-contained HTML string.
pub fn render_invoice_html(inv: &Invoice, prac: &PractitionerConfig) -> String {
    let display_name = prac.display_name();
    let company_legal = if prac.trading_name.is_some() {
        format!(
            r#"<div style="color:#555;font-size:13px;margin-top:2px;">{}</div>"#,
            esc(&prac.company_name)
        )
    } else {
        String::new()
    };

    let address_html = prac
        .address
        .as_deref()
        .map(|a| format!(r#"<div style="color:#555;font-size:13px;margin-top:4px;">{}</div>"#, esc(a)))
        .unwrap_or_default();

    let contact_html = {
        let mut parts = Vec::new();
        if let Some(p) = &prac.phone {
            parts.push(esc(p));
        }
        if let Some(e) = &prac.email {
            parts.push(format!(r#"<a href="mailto:{e}" style="color:#555;">{}</a>"#, esc(e)));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!(
                r#"<div style="color:#555;font-size:13px;margin-top:2px;">{}</div>"#,
                parts.join(" &nbsp;|&nbsp; ")
            )
        }
    };

    // Bill-to block
    let (bill_to_name, bill_to_extra) = match &inv.bill_to {
        BillTo::Client { name, email } => {
            let extra = email
                .as_deref()
                .map(|e| format!(r#"<div style="font-size:13px;color:#555;">{}</div>"#, esc(e)))
                .unwrap_or_default();
            (esc(name), extra)
        }
        BillTo::Insurer {
            name,
            policy,
            contact,
            ..
        } => {
            let mut extra = String::new();
            if let Some(c) = contact {
                extra.push_str(&format!(
                    r#"<div style="font-size:13px;color:#555;">{}</div>"#,
                    esc(c)
                ));
            }
            if let Some(p) = policy {
                extra.push_str(&format!(
                    r#"<div style="font-size:13px;color:#555;">Policy: {}</div>"#,
                    esc(p)
                ));
            }
            (esc(name), extra)
        }
    };

    // Line items table rows
    let rows: String = inv
        .line_items
        .iter()
        .map(|li| {
            format!(
                r#"<tr>
  <td style="padding:8px 10px;border-bottom:1px solid #eee;color:#555;font-size:14px;">{}</td>
  <td style="padding:8px 10px;border-bottom:1px solid #eee;font-size:14px;">{}</td>
  <td style="padding:8px 10px;border-bottom:1px solid #eee;font-size:14px;text-align:right;">{} {:.2}</td>
</tr>"#,
                esc(&li.session_date),
                esc(&li.description),
                esc(&inv.currency),
                li.unit_amount * li.quantity as f64,
            )
        })
        .collect();

    let total = inv.total();

    // Payment section
    let payment_section = if prac.has_bank_details() {
        let bank = prac.bank_name.as_deref().unwrap_or("");
        let sort = prac.sort_code.as_deref().unwrap_or("");
        let acc = prac.account_number.as_deref().unwrap_or("");
        let acc_name = prac
            .account_name
            .as_deref()
            .unwrap_or(&prac.company_name);

        let payment_link_html = inv
            .payment_link
            .as_deref()
            .map(|link| {
                format!(
                    r#"<p style="margin:12px 0 4px 0;"><a href="{link}" style="display:inline-block;background:#1a1a2e;color:#fff;padding:10px 20px;border-radius:4px;text-decoration:none;font-size:14px;">Pay online</a></p>"#
                )
            })
            .unwrap_or_default();

        format!(
            r#"<div style="background:#f9f9f9;border:1px solid #e0e0e0;border-radius:4px;padding:16px;margin-top:24px;">
  <h3 style="margin:0 0 10px 0;font-size:15px;color:#1a1a2e;">Payment details</h3>
  <table cellpadding="0" cellspacing="0" style="font-size:14px;">
    {bank_row}
    <tr><td style="color:#888;padding:2px 16px 2px 0;">Account name</td><td><strong>{}</strong></td></tr>
    <tr><td style="color:#888;padding:2px 16px 2px 0;">Sort code</td><td>{}</td></tr>
    <tr><td style="color:#888;padding:2px 16px 2px 0;">Account number</td><td>{}</td></tr>
    <tr><td style="color:#888;padding:2px 16px 2px 0;">Reference</td><td>{}</td></tr>
  </table>
  {payment_link_html}
</div>"#,
            esc(acc_name),
            esc(sort),
            esc(acc),
            esc(&inv.reference),
            bank_row = if bank.is_empty() {
                String::new()
            } else {
                format!(r#"<tr><td style="color:#888;padding:2px 16px 2px 0;">Bank</td><td>{}</td></tr>"#, esc(bank))
            },
        )
    } else if let Some(link) = &inv.payment_link {
        format!(
            r#"<div style="margin-top:24px;">
  <a href="{link}" style="display:inline-block;background:#1a1a2e;color:#fff;padding:10px 20px;border-radius:4px;text-decoration:none;font-size:14px;">Pay online</a>
</div>"#
        )
    } else {
        // Insurer or no payment config — generic reference note
        format!(
            r#"<p style="color:#555;font-size:13px;margin-top:16px;">Please quote invoice reference <strong>{}</strong> on payment.</p>"#,
            esc(&inv.reference)
        )
    };

    // Footer (company reg / VAT)
    let footer_parts: Vec<String> = [
        prac.company_reg
            .as_deref()
            .map(|r| format!("Company reg: {}", esc(r))),
        prac.vat_number
            .as_deref()
            .map(|v| format!("VAT: {}", esc(v))),
    ]
    .into_iter()
    .flatten()
    .collect();

    let footer_html = if footer_parts.is_empty() {
        String::new()
    } else {
        format!(
            r#"<p style="color:#aaa;font-size:11px;margin-top:24px;border-top:1px solid #eee;padding-top:12px;">{}</p>"#,
            footer_parts.join(" &nbsp;&middot;&nbsp; ")
        )
    };

    let notes_html = inv
        .notes
        .as_deref()
        .filter(|n| !n.is_empty())
        .map(|n| {
            format!(
                r#"<p style="font-size:13px;color:#666;margin-top:16px;font-style:italic;">{}</p>"#,
                esc(n)
            )
        })
        .unwrap_or_default();

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"></head>
<body style="margin:0;padding:0;background:#f0f0f0;font-family:Arial,Helvetica,sans-serif;">
<table width="100%" cellpadding="0" cellspacing="0" style="background:#f0f0f0;"><tr><td align="center" style="padding:30px 10px;">
<table width="660" cellpadding="0" cellspacing="0" style="background:#fff;border-radius:6px;overflow:hidden;box-shadow:0 1px 4px rgba(0,0,0,.1);">

  <!-- Header bar -->
  <tr><td style="background:#1a1a2e;padding:0;height:6px;"></td></tr>

  <!-- Letterhead -->
  <tr><td style="padding:28px 36px 0 36px;">
    <table width="100%" cellpadding="0" cellspacing="0"><tr>
      <td>
        <div style="font-size:22px;font-weight:bold;color:#1a1a2e;">{display_name}</div>
        {company_legal}
        {address_html}
        {contact_html}
      </td>
      <td align="right" valign="top">
        <div style="font-size:30px;font-weight:bold;color:#888;letter-spacing:2px;">INVOICE</div>
      </td>
    </tr></table>
  </td></tr>

  <!-- Divider -->
  <tr><td style="padding:20px 36px 0 36px;"><hr style="border:none;border-top:2px solid #1a1a2e;margin:0;"></td></tr>

  <!-- Invoice meta -->
  <tr><td style="padding:16px 36px;">
    <table width="100%" cellpadding="0" cellspacing="0"><tr>
      <td style="font-size:14px;color:#333;">
        <table cellpadding="0" cellspacing="0">
          <tr><td style="color:#888;padding-right:16px;">Invoice</td><td><strong>{reference}</strong></td></tr>
          <tr><td style="color:#888;padding-right:16px;">Date</td><td>{issue_date}</td></tr>
          <tr><td style="color:#888;padding-right:16px;">Due</td><td>{due_date}</td></tr>
        </table>
      </td>
      <td align="right" valign="top" style="font-size:14px;color:#333;">
        <div style="color:#888;font-size:12px;margin-bottom:4px;">BILL TO</div>
        <div style="font-weight:bold;">{bill_to_name}</div>
        {bill_to_extra}
      </td>
    </tr></table>
  </td></tr>

  <!-- Line items -->
  <tr><td style="padding:0 36px;">
    <table width="100%" cellpadding="0" cellspacing="0" style="border-collapse:collapse;">
      <thead>
        <tr style="background:#f5f5f5;">
          <th style="padding:8px 10px;text-align:left;font-size:12px;color:#888;text-transform:uppercase;letter-spacing:.5px;border-bottom:2px solid #e0e0e0;">Date</th>
          <th style="padding:8px 10px;text-align:left;font-size:12px;color:#888;text-transform:uppercase;letter-spacing:.5px;border-bottom:2px solid #e0e0e0;">Description</th>
          <th style="padding:8px 10px;text-align:right;font-size:12px;color:#888;text-transform:uppercase;letter-spacing:.5px;border-bottom:2px solid #e0e0e0;">Amount</th>
        </tr>
      </thead>
      <tbody>{rows}</tbody>
      <tfoot>
        <tr>
          <td colspan="2" style="padding:12px 10px;text-align:right;font-weight:bold;font-size:15px;border-top:2px solid #333;">TOTAL</td>
          <td style="padding:12px 10px;text-align:right;font-weight:bold;font-size:15px;border-top:2px solid #333;">{currency} {total:.2}</td>
        </tr>
      </tfoot>
    </table>
  </td></tr>

  <!-- Payment + notes -->
  <tr><td style="padding:0 36px 28px 36px;">
    {payment_section}
    {notes_html}
    {footer_html}
  </td></tr>

  <!-- Footer bar -->
  <tr><td style="background:#1a1a2e;padding:0;height:4px;"></td></tr>

</table>
</td></tr></table>
</body>
</html>"#,
        display_name = esc(display_name),
        reference = esc(&inv.reference),
        issue_date = esc(&inv.issue_date),
        due_date = esc(&inv.due_date),
        currency = esc(&inv.currency),
        total = total,
    )
}

/// Send an invoice email to the bill-to address.
///
/// Requires practitioner config (for letterhead) and email config (SMTP).
pub fn send_invoice(
    inv: &Invoice,
    prac: &PractitionerConfig,
    email_cfg: &EmailConfig,
) -> Result<()> {
    let to_email = inv
        .bill_to
        .email()
        .ok_or_else(|| anyhow::anyhow!(
            "No email address for bill-to party on invoice {}.\n\
             Add an email to the client's identity.yaml or insurer contact.",
            inv.reference
        ))?;

    let to_name = inv.bill_to.display_name();

    if !prac.is_configured() {
        bail!(
            "No [practitioner] config found — cannot render invoice letterhead.\n\
             Add a [practitioner] section to ~/.config/practiceforge/config.toml.\n\
             Minimum required: company_name = \"Your Company\""
        );
    }

    let html = render_invoice_html(inv, prac);
    let subject = format!(
        "Invoice {} from {} — {} {:.2}",
        inv.reference,
        prac.display_name(),
        inv.currency,
        inv.total()
    );

    // Use practitioner's own email as the from-address if configured,
    // falling back to the [email] config (which may be the COHS address).
    // Invoices come from the practitioner's company, not COHS.
    let mut invoice_email_cfg = email_cfg.clone();
    if let Some(prac_email) = &prac.email {
        invoice_email_cfg.from_email = prac_email.clone();
        invoice_email_cfg.from_name = prac.display_name().to_string();
    }

    crate::email::send_html_email(&invoice_email_cfg, to_email, to_name, &subject, &html)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// HTML escaping (no external dep needed)
// ---------------------------------------------------------------------------

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
