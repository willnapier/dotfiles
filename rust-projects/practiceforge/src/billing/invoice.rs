//! Invoice data types and generation logic.
//!
//! Generates invoices from session data + identity.yaml metadata.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

use super::sessions::BillableSession;

/// A generated invoice ready to be persisted via an AccountingProvider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invoice {
    pub reference: String,
    pub client_id: String,
    pub client_name: String,
    pub bill_to: BillTo,
    pub line_items: Vec<LineItem>,
    pub issue_date: String,
    pub due_date: String,
    pub currency: String,
    pub state: InvoiceState,
    pub payment_link: Option<String>,
    pub notes: Option<String>,
}

impl Invoice {
    pub fn total(&self) -> f64 {
        self.line_items
            .iter()
            .map(|li| li.unit_amount * li.quantity as f64)
            .sum()
    }
}

/// Who the invoice is addressed to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BillTo {
    Client {
        name: String,
        email: Option<String>,
    },
    Insurer {
        name: String,
        contact: Option<String>,
        email: Option<String>,
        policy: Option<String>,
    },
}

impl BillTo {
    pub fn display_name(&self) -> &str {
        match self {
            BillTo::Client { name, .. } => name,
            BillTo::Insurer { name, .. } => name,
        }
    }

    pub fn email(&self) -> Option<&str> {
        match self {
            BillTo::Client { email, .. } => email.as_deref(),
            BillTo::Insurer { email, .. } => email.as_deref(),
        }
    }
}

/// A single line on an invoice.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineItem {
    pub description: String,
    pub session_date: String,
    pub quantity: u32,
    pub unit_amount: f64,
}

/// Invoice lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InvoiceState {
    Draft,
    Sent,
    Overdue,
    Paid,
    Cancelled,
}

impl std::fmt::Display for InvoiceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InvoiceState::Draft => write!(f, "draft"),
            InvoiceState::Sent => write!(f, "sent"),
            InvoiceState::Overdue => write!(f, "overdue"),
            InvoiceState::Paid => write!(f, "paid"),
            InvoiceState::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// Reference returned after creating an invoice.
#[derive(Debug, Clone)]
pub struct InvoiceRef {
    pub reference: String,
    pub file_path: Option<String>,
}

/// Extract the per-session rate from an identity.yaml funding.rate value.
///
/// The rate field is stored as a serde_yaml::Value which could be a number
/// or a string like "198" or "198.00". Returns None if not parseable.
pub fn parse_rate(rate: &serde_yaml::Value) -> Option<f64> {
    match rate {
        serde_yaml::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                Some(f)
            } else {
                n.as_u64().map(|u| u as f64)
            }
        }
        serde_yaml::Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

/// Legacy notes.md scraper — kept for backfill / fallback only.
///
/// **Do not use for new invoicing.** The session JSON loader in
/// `billing::sessions::billable_sessions_for_client` is the authoritative
/// source; it carries attendance status (DNA, cancelled, late-cancel)
/// that notes.md headers cannot express.
///
/// Returns dates in YYYY-MM-DD format, extracted from `### YYYY-MM-DD` headers.
/// Historically excluded DNA — this was a bug against policy (DNA bills)
/// but is preserved here because legacy-corpus notes.md files were written
/// with that assumption.
pub fn extract_session_dates(notes_content: &str) -> Vec<String> {
    let re = regex::Regex::new(r"^### (\d{4}-\d{2}-\d{2})(?:\s|$)").unwrap();
    let dna_re = regex::Regex::new(r"^### \d{4}-\d{2}-\d{2}\s+DNA").unwrap();

    notes_content
        .lines()
        .filter(|line| re.is_match(line) && !dna_re.is_match(line))
        .filter_map(|line| {
            re.captures(line)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string())
        })
        .collect()
}

/// Legacy filter — date-only. Session-JSON callers use
/// `billing::sessions::uninvoiced_billable` instead.
pub fn uninvoiced_sessions(
    all_sessions: &[String],
    invoiced_dates: &[String],
) -> Vec<String> {
    all_sessions
        .iter()
        .filter(|d| !invoiced_dates.contains(d))
        .cloned()
        .collect()
}

/// Build an Invoice from billable sessions and client metadata.
///
/// Each `BillableSession` carries a `BillReason` (Attended / Dna /
/// LateCancellation) that is appended to the line item description so
/// the recipient sees *why* a session was billed even though the client
/// did not attend.
///
/// Reads identity.yaml for funding info and calculates the due date.
pub fn build_invoice(
    reference: String,
    client_id: &str,
    identity_path: &Path,
    sessions: &[BillableSession],
    payment_terms_days: i64,
    currency: &str,
) -> Result<Invoice> {
    let content = std::fs::read_to_string(identity_path)
        .with_context(|| format!("Cannot read {}", identity_path.display()))?;

    let identity: serde_yaml::Value =
        serde_yaml::from_str(&content).context("Failed to parse identity.yaml")?;

    let name = identity
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();

    let funding = identity.get("funding");

    let rate = funding
        .and_then(|f| f.get("rate"))
        .and_then(|r| parse_rate(r))
        .unwrap_or(0.0);

    if rate == 0.0 {
        bail!(
            "No rate configured for client {} in identity.yaml",
            client_id
        );
    }

    let session_duration = funding
        .and_then(|f| f.get("session_duration"))
        .and_then(|v| v.as_u64())
        .unwrap_or(45);

    let funding_type = funding
        .and_then(|f| f.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("self-pay");

    let is_self_pay = funding_type.eq_ignore_ascii_case("self-pay")
        || funding_type.eq_ignore_ascii_case("self pay")
        || funding_type.is_empty();

    let bill_to = if is_self_pay {
        BillTo::Client {
            name: name.clone(),
            email: identity
                .get("email")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        }
    } else {
        BillTo::Insurer {
            name: funding_type.to_string(),
            contact: funding
                .and_then(|f| f.get("contact"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            email: funding
                .and_then(|f| f.get("email"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            policy: funding
                .and_then(|f| f.get("policy"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        }
    };

    let line_items: Vec<LineItem> = sessions
        .iter()
        .map(|s| LineItem {
            description: format!(
                "Clinical psychology session ({} min) — {}{}",
                session_duration,
                s.date,
                s.reason.line_item_tag()
            ),
            session_date: s.date.clone(),
            quantity: 1,
            unit_amount: rate,
        })
        .collect();

    let today = chrono::Local::now().date_naive();
    let due_date = today + chrono::Duration::days(payment_terms_days);

    Ok(Invoice {
        reference,
        client_id: client_id.to_string(),
        client_name: name,
        bill_to,
        line_items,
        issue_date: today.format("%Y-%m-%d").to_string(),
        due_date: due_date.format("%Y-%m-%d").to_string(),
        currency: currency.to_string(),
        state: InvoiceState::Draft,
        payment_link: None,
        notes: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_session_dates() {
        let notes = "### 2026-03-01\nSome notes...\n\n### 2026-03-08 DNA\n\n### 2026-03-15\nMore notes.\n";
        let dates = extract_session_dates(notes);
        assert_eq!(dates, vec!["2026-03-01", "2026-03-15"]);
    }

    #[test]
    fn test_uninvoiced_sessions() {
        let all = vec![
            "2026-03-01".to_string(),
            "2026-03-08".to_string(),
            "2026-03-15".to_string(),
        ];
        let invoiced = vec!["2026-03-01".to_string()];
        let result = uninvoiced_sessions(&all, &invoiced);
        assert_eq!(result, vec!["2026-03-08", "2026-03-15"]);
    }

    #[test]
    fn test_parse_rate_number() {
        let v = serde_yaml::Value::Number(serde_yaml::Number::from(198));
        assert_eq!(parse_rate(&v), Some(198.0));
    }

    #[test]
    fn test_parse_rate_string() {
        let v = serde_yaml::Value::String("175.50".to_string());
        assert_eq!(parse_rate(&v), Some(175.5));
    }

    #[test]
    fn test_invoice_total() {
        let inv = Invoice {
            reference: "TEST-001".to_string(),
            client_id: "JB92".to_string(),
            client_name: "Jane Bloggs".to_string(),
            bill_to: BillTo::Client {
                name: "Jane Bloggs".to_string(),
                email: None,
            },
            line_items: vec![
                LineItem {
                    description: "Session".to_string(),
                    session_date: "2026-03-01".to_string(),
                    quantity: 1,
                    unit_amount: 198.0,
                },
                LineItem {
                    description: "Session".to_string(),
                    session_date: "2026-03-08".to_string(),
                    quantity: 1,
                    unit_amount: 198.0,
                },
            ],
            issue_date: "2026-03-15".to_string(),
            due_date: "2026-03-29".to_string(),
            currency: "GBP".to_string(),
            state: InvoiceState::Draft,
            payment_link: None,
            notes: None,
        };
        assert_eq!(inv.total(), 396.0);
    }
}
