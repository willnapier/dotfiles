//! Invoice PDF rendering and email delivery.
//!
//! Renders a professional invoice as a LaTeX PDF on the practitioner's
//! letterhead and sends it as an email attachment via the configured
//! SMTP server. Same toolchain as clinical letters (lualatex).
//!
//! PDF is saved to the client's admin directory so there is a permanent
//! record of exactly what was sent.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;

use super::invoice::{BillTo, Invoice};
use super::practitioner::PractitionerConfig;
use crate::email::EmailConfig;

// ---------------------------------------------------------------------------
// LaTeX rendering
// ---------------------------------------------------------------------------

/// Render a complete invoice as a LaTeX source string.
pub fn render_invoice_latex(inv: &Invoice, prac: &PractitionerConfig) -> String {
    let display_name = esc_tex(prac.display_name());
    let company_legal = if prac.trading_name.is_some() {
        format!(r"\small\color{{gray}}{}", esc_tex(&prac.company_name))
    } else {
        String::new()
    };

    let address_line = prac
        .address
        .as_deref()
        .map(|a| format!(r"\\ \small\color{{gray}}{}", esc_tex(a)))
        .unwrap_or_default();

    let contact_line = {
        let mut parts = Vec::new();
        if let Some(p) = &prac.phone { parts.push(esc_tex(p)); }
        if let Some(e) = &prac.email {
            parts.push(format!(r"\href{{mailto:{}}}{{{}}}", e, esc_tex(e)));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!(r"\\ \small\color{{gray}}{}", parts.join(r" \textbar\ "))
        }
    };

    // Bill-to block
    let (bill_to_name, bill_to_extra) = match &inv.bill_to {
        BillTo::Client { name, email } => {
            let extra = email
                .as_deref()
                .map(|e| format!("\\\\ \\small\\color{{gray}}{}", esc_tex(e)))
                .unwrap_or_default();
            (esc_tex(name), extra)
        }
        BillTo::Insurer { name, policy, contact, .. } => {
            let mut extra = String::new();
            if let Some(c) = contact {
                extra.push_str(&format!("\\\\ \\small\\color{{gray}}{}", esc_tex(c)));
            }
            if let Some(p) = policy {
                extra.push_str(&format!("\\\\ \\small\\color{{gray}}Policy: {}", esc_tex(p)));
            }
            (esc_tex(name), extra)
        }
    };

    // Line items rows
    let rows: String = inv.line_items.iter().map(|li| {
        format!(
            "    {} & {} & {} {:.2} \\\\\n",
            esc_tex(&li.session_date),
            esc_tex(&li.description),
            esc_tex(&inv.currency),
            li.unit_amount * li.quantity as f64,
        )
    }).collect();

    let total = inv.total();

    // Payment section
    let payment_section = if prac.has_bank_details() {
        let bank = prac.bank_name.as_deref().unwrap_or("");
        let sort = prac.sort_code.as_deref().unwrap_or("");
        let acc = prac.account_number.as_deref().unwrap_or("");
        let acc_name = prac.account_name.as_deref().unwrap_or(&prac.company_name);

        let bank_row = if bank.is_empty() {
            String::new()
        } else {
            format!("    \\textcolor{{gray}}{{Bank}} & {} \\\\\n", esc_tex(bank))
        };

        let payment_link_block = inv.payment_link.as_deref().map(|link| {
            format!(
                "\n\\vspace{{8pt}}\n\\href{{{link}}}{{\\colorbox{{navy}}{{\\textcolor{{white}}{{\\textbf{{Pay online}}}}}}}}\n"
            )
        }).unwrap_or_default();

        format!(
            r"\vspace{{12pt}}
\colorbox{{lightgray}}{{\begin{{minipage}}{{\linewidth}}
\vspace{{6pt}}
{{\small\textbf{{Payment details}}}}\\[4pt]
\begin{{tabular}}{{ll}}
{bank_row}    \textcolor{{gray}}{{Account name}} & \textbf{{{acc_name}}} \\
    \textcolor{{gray}}{{Sort code}} & {sort} \\
    \textcolor{{gray}}{{Account number}} & {acc} \\
    \textcolor{{gray}}{{Reference}} & {reference} \\
\end{{tabular}}
{payment_link_block}\vspace{{4pt}}
\end{{minipage}}}}",
            bank_row = bank_row,
            acc_name = esc_tex(acc_name),
            sort = esc_tex(sort),
            acc = esc_tex(acc),
            reference = esc_tex(&inv.reference),
            payment_link_block = payment_link_block,
        )
    } else if let Some(link) = &inv.payment_link {
        format!(
            "\n\\vspace{{12pt}}\n\\href{{{link}}}{{\\colorbox{{navy}}{{\\textcolor{{white}}{{\\textbf{{Pay online}}}}}}}}\n"
        )
    } else {
        format!(
            "\n\\vspace{{12pt}}\n\\small\\color{{gray}}Please quote invoice reference \\textbf{{{}}} on payment.\n",
            esc_tex(&inv.reference)
        )
    };

    // Footer (company reg / VAT)
    let footer_parts: Vec<String> = [
        prac.company_reg.as_deref().map(|r| format!("Company reg: {}", esc_tex(r))),
        prac.vat_number.as_deref().map(|v| format!("VAT: {}", esc_tex(v))),
    ].into_iter().flatten().collect();

    let footer = if footer_parts.is_empty() {
        String::new()
    } else {
        format!(
            "\n\\vspace{{12pt}}\\noindent\\rule{{\\linewidth}}{{0.4pt}}\\\\\n\\tiny\\color{{gray}}{}\n",
            footer_parts.join(" \\textperiodcentered\\ ")
        )
    };

    let notes_block = inv.notes.as_deref().filter(|n| !n.is_empty()).map(|n| {
        format!("\n\\vspace{{8pt}}\n\\small\\textit{{\\color{{gray}}{}}}\n", esc_tex(n))
    }).unwrap_or_default();

    format!(
        r"\documentclass[a4paper,11pt]{{article}}
\usepackage{{fontspec}}
\usepackage[margin=2.5cm]{{geometry}}
\usepackage{{xcolor}}
\usepackage{{hyperref}}
\usepackage{{booktabs}}
\usepackage{{array}}
\usepackage{{tabularx}}
\usepackage{{parskip}}
\usepackage{{colortbl}}

\definecolor{{navy}}{{HTML}}{{1a1a2e}}
\definecolor{{gray}}{{HTML}}{{666666}}
\definecolor{{lightgray}}{{HTML}}{{f5f5f5}}

\hypersetup{{
  colorlinks=true,
  linkcolor=navy,
  urlcolor=navy,
}}

\setlength{{\parindent}}{{0pt}}

\begin{{document}}
\pagestyle{{empty}}

% Top colour bar
{{\color{{navy}}\rule{{\linewidth}}{{6pt}}}}

\vspace{{12pt}}

% Letterhead
\begin{{minipage}}[t]{{0.6\linewidth}}
{{\large\textbf{{\color{{navy}}{display_name}}}}}\\
{company_legal}{address_line}{contact_line}
\end{{minipage}}%
\hfill
\begin{{minipage}}[t]{{0.35\linewidth}}
\raggedleft
{{\fontsize{{28}}{{34}}\selectfont\color{{lightgray!80!gray}}\textbf{{INVOICE}}}}
\end{{minipage}}

\vspace{{8pt}}
{{\color{{navy}}\rule{{\linewidth}}{{1.5pt}}}}
\vspace{{8pt}}

% Invoice meta + Bill To
\begin{{minipage}}[t]{{0.5\linewidth}}
\begin{{tabular}}{{ll}}
  \textcolor{{gray}}{{Invoice}} & \textbf{{{reference}}} \\
  \textcolor{{gray}}{{Date}}    & {issue_date} \\
  \textcolor{{gray}}{{Due}}     & {due_date} \\
\end{{tabular}}
\end{{minipage}}%
\hfill
\begin{{minipage}}[t]{{0.45\linewidth}}
\raggedleft
{{\small\color{{gray}}BILL TO}}\\
\textbf{{{bill_to_name}}}{bill_to_extra}
\end{{minipage}}

\vspace{{16pt}}

% Line items
\begin{{tabularx}}{{\linewidth}}{{lXr}}
\toprule
\rowcolor{{lightgray}}
\small\textcolor{{gray}}{{DATE}} & \small\textcolor{{gray}}{{DESCRIPTION}} & \small\textcolor{{gray}}{{AMOUNT}} \\
\midrule
{rows}\bottomrule
\addlinespace[4pt]
\multicolumn{{2}}{{r}}{{\textbf{{TOTAL}}}} & \textbf{{{currency} {total:.2}}} \\
\end{{tabularx}}

{payment_section}
{notes_block}
{footer}

% Bottom colour bar
\vfill
{{\color{{navy}}\rule{{\linewidth}}{{4pt}}}}

\end{{document}}
",
        display_name = display_name,
        company_legal = company_legal,
        address_line = address_line,
        contact_line = contact_line,
        reference = esc_tex(&inv.reference),
        issue_date = esc_tex(&inv.issue_date),
        due_date = esc_tex(&inv.due_date),
        bill_to_name = bill_to_name,
        bill_to_extra = bill_to_extra,
        rows = rows,
        currency = esc_tex(&inv.currency),
        total = total,
        payment_section = payment_section,
        notes_block = notes_block,
        footer = footer,
    )
}

// ---------------------------------------------------------------------------
// PDF build
// ---------------------------------------------------------------------------

/// Compile LaTeX source to PDF. Returns path to the generated PDF.
///
/// Stores the PDF in `~/Clinical/clients/<client_id>/admin/` so there is
/// a permanent record of what was sent. The .tex source is written alongside
/// it (useful for debugging or manual re-renders).
pub fn build_invoice_pdf(inv: &Invoice, prac: &PractitionerConfig) -> Result<PathBuf> {
    let clients_dir = crate::config::clients_dir();
    let admin_dir = clients_dir.join(&inv.client_id).join("admin");
    std::fs::create_dir_all(&admin_dir)
        .with_context(|| format!("Could not create {}", admin_dir.display()))?;

    let stem = inv.reference.replace('/', "-");
    let tex_path = admin_dir.join(format!("{}.tex", stem));
    let pdf_path = admin_dir.join(format!("{}.pdf", stem));

    let source = render_invoice_latex(inv, prac);
    std::fs::write(&tex_path, &source)
        .with_context(|| format!("Could not write {}", tex_path.display()))?;

    let output = std::process::Command::new("lualatex")
        .args([
            "--interaction=nonstopmode",
            &format!("--output-directory={}", admin_dir.display()),
            tex_path.to_str().unwrap(),
        ])
        .output()
        .context("lualatex not found — install texlive-luatex")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stdout); // lualatex writes errors to stdout
        bail!(
            "lualatex failed for {}:\n{}",
            inv.reference,
            &stderr.lines().filter(|l| l.contains("Error") || l.contains("error") || l.starts_with('!')).collect::<Vec<_>>().join("\n")
        );
    }

    // Clean up lualatex auxiliary files
    for ext in &["aux", "log", "out"] {
        let _ = std::fs::remove_file(admin_dir.join(format!("{}.{}", stem, ext)));
    }

    Ok(pdf_path)
}

// ---------------------------------------------------------------------------
// Email delivery
// ---------------------------------------------------------------------------

/// Build a PDF invoice and email it to the bill-to address.
pub fn send_invoice(
    inv: &Invoice,
    prac: &PractitionerConfig,
    email_cfg: &EmailConfig,
) -> Result<PathBuf> {
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

    let pdf_path = build_invoice_pdf(inv, prac)?;

    let subject = format!(
        "Invoice {} from {} — {} {:.2}",
        inv.reference,
        prac.display_name(),
        inv.currency,
        inv.total()
    );

    let body = format!(
        "Dear {},\n\nPlease find attached invoice {} for {} {:.2}, due {}.\n\n{}\n",
        inv.bill_to.display_name(),
        inv.reference,
        inv.currency,
        inv.total(),
        inv.due_date,
        if inv.payment_link.is_some() { "You can pay online using the link in the attached invoice." } else { "" },
    );

    // Use practitioner's own email as from-address if configured.
    // Invoices come from the practitioner's company, not COHS.
    let mut invoice_email_cfg = email_cfg.clone();
    if let Some(prac_email) = &prac.email {
        invoice_email_cfg.from_email = prac_email.clone();
        invoice_email_cfg.from_name = prac.display_name().to_string();
    }

    crate::email::send_email(
        &invoice_email_cfg,
        to_email,
        to_name,
        &subject,
        &body,
        Some(&pdf_path),
        None,
    )?;

    Ok(pdf_path)
}

// ---------------------------------------------------------------------------
// LaTeX escaping
// ---------------------------------------------------------------------------

fn esc_tex(s: &str) -> String {
    s.replace('\\', r"\textbackslash{}")
        .replace('{', r"\{")
        .replace('}', r"\}")
        .replace('&', r"\&")
        .replace('%', r"\%")
        .replace('$', r"\$")
        .replace('#', r"\#")
        .replace('_', r"\_")
        .replace('^', r"\textasciicircum{}")
        .replace('~', r"\textasciitilde{}")
}
