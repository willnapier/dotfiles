//! Xero AccountingProvider — invoice management via the Xero API.
//!
//! Authentication: OAuth2 PKCE flow. Credentials stored in secrets.toml.
//!
//! HTTP threading: reqwest::blocking panics inside a Tokio context, so all
//! HTTP calls are dispatched onto a dedicated OS thread via std::thread::spawn.

use anyhow::{Context, Result};
use chrono::{Local, NaiveDate};
use data_encoding::BASE64URL_NOPAD;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::invoice::{Invoice, InvoiceRef, InvoiceState};
use super::secrets::BillingSecrets;
use super::traits::{AccountingProvider, InvoiceFilter, InvoiceSummary};

// ---------------------------------------------------------------------------
// Xero API constants
// ---------------------------------------------------------------------------

const AUTH_URL: &str = "https://login.xero.com/identity/connect/authorize";
const TOKEN_URL: &str = "https://identity.xero.com/connect/token";
const CONNECTIONS_URL: &str = "https://api.xero.com/connections";
const API_BASE: &str = "https://api.xero.com/api.xro/2.0";
const REDIRECT_URI: &str = "http://localhost:8765/callback";

// ---------------------------------------------------------------------------
// HTTP helpers — all HTTP is moved off the Tokio thread
// ---------------------------------------------------------------------------

fn http_get(url: &str, auth: &str, tenant_id: Option<&str>) -> Result<String> {
    let (url, auth, tenant_id) = (
        url.to_string(),
        auth.to_string(),
        tenant_id.map(|s| s.to_string()),
    );
    std::thread::spawn(move || -> anyhow::Result<String> {
        let mut req = reqwest::blocking::Client::new()
            .get(&url)
            .header("Authorization", &auth)
            .header("Accept", "application/json");
        if let Some(tid) = &tenant_id {
            req = req.header("Xero-Tenant-Id", tid);
        }
        let resp = req.send()?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Xero GET {} {}: {}", url, status, body);
        }
        Ok(resp.text()?)
    })
    .join()
    .map_err(|_| anyhow::anyhow!("HTTP thread panicked"))?
}

fn http_post(url: &str, auth: &str, tenant_id: Option<&str>, body: &str) -> Result<String> {
    let (url, auth, tenant_id, body) = (
        url.to_string(),
        auth.to_string(),
        tenant_id.map(|s| s.to_string()),
        body.to_string(),
    );
    std::thread::spawn(move || -> anyhow::Result<String> {
        let mut req = reqwest::blocking::Client::new()
            .post(&url)
            .header("Authorization", &auth)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .body(body);
        if let Some(tid) = &tenant_id {
            req = req.header("Xero-Tenant-Id", tid);
        }
        let resp = req.send()?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Xero POST {} {}: {}", url, status, body);
        }
        Ok(resp.text()?)
    })
    .join()
    .map_err(|_| anyhow::anyhow!("HTTP thread panicked"))?
}

fn http_put(url: &str, auth: &str, tenant_id: Option<&str>, body: &str) -> Result<String> {
    let (url, auth, tenant_id, body) = (
        url.to_string(),
        auth.to_string(),
        tenant_id.map(|s| s.to_string()),
        body.to_string(),
    );
    std::thread::spawn(move || -> anyhow::Result<String> {
        let mut req = reqwest::blocking::Client::new()
            .put(&url)
            .header("Authorization", &auth)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .body(body);
        if let Some(tid) = &tenant_id {
            req = req.header("Xero-Tenant-Id", tid);
        }
        let resp = req.send()?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Xero PUT {} {}: {}", url, status, body);
        }
        Ok(resp.text()?)
    })
    .join()
    .map_err(|_| anyhow::anyhow!("HTTP thread panicked"))?
}

/// POST form-encoded data to a URL with HTTP Basic auth.
fn http_post_form_basic(url: &str, client_id: &str, client_secret: &str, body: &str) -> Result<String> {
    let (url, client_id, client_secret, body) = (
        url.to_string(),
        client_id.to_string(),
        client_secret.to_string(),
        body.to_string(),
    );
    std::thread::spawn(move || -> anyhow::Result<String> {
        let resp = reqwest::blocking::Client::new()
            .post(&url)
            .basic_auth(&client_id, Some(&client_secret))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Token endpoint {} {}: {}", url, status, body);
        }
        Ok(resp.text()?)
    })
    .join()
    .map_err(|_| anyhow::anyhow!("HTTP thread panicked"))?
}

// ---------------------------------------------------------------------------
// Xero API types (serde only)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct XeroContact {
    #[serde(skip_serializing_if = "Option::is_none")]
    ContactID: Option<String>,
    Name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    EmailAddress: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(non_snake_case)]
struct XeroLineItem {
    Description: String,
    Quantity: f64,
    UnitAmount: f64,
    AccountCode: String,
}

#[derive(Debug, Serialize)]
#[allow(non_snake_case)]
struct XeroInvoiceCreate {
    #[serde(rename = "Type")]
    Type_: String,
    Contact: XeroContact,
    Date: String,
    DueDate: String,
    InvoiceNumber: String,
    CurrencyCode: String,
    Status: String,
    LineItems: Vec<XeroLineItem>,
    Reference: String,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct XeroInvoiceResponse {
    Invoices: Vec<XeroInvoiceRecord>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct XeroInvoiceRecord {
    InvoiceID: Option<String>,
    InvoiceNumber: Option<String>,
    Status: Option<String>,
    Contact: Option<XeroContact>,
    LineItems: Option<Vec<XeroLineItemRecord>>,
    DateString: Option<String>,
    DueDateString: Option<String>,
    AmountDue: Option<f64>,
    CurrencyCode: Option<String>,
    Reference: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct XeroLineItemRecord {
    Description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct XeroContactsResponse {
    Contacts: Vec<XeroContact>,
}

#[derive(Debug, Deserialize)]
struct XeroTenant {
    #[serde(rename = "tenantId")]
    tenant_id: String,
    #[serde(rename = "tenantName")]
    tenant_name: String,
}

#[derive(Debug, Deserialize)]
struct XeroTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

// ---------------------------------------------------------------------------
// XeroProvider
// ---------------------------------------------------------------------------

/// AccountingProvider backed by the Xero API.
pub struct XeroProvider {
    tenant_id: String,
    access_token: String,
    client_id: String,
    client_secret: String,
}

impl XeroProvider {
    /// Load from secrets.toml, refreshing the token if expired.
    pub fn new() -> Result<Self> {
        let mut secrets = BillingSecrets::load()?;
        let xero = &secrets.xero;

        let client_id = xero
            .client_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Xero client_id not configured. Run 'billing xero-setup'."))?
            .to_string();
        let client_secret = xero
            .client_secret
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Xero client_secret not configured."))?
            .to_string();
        let tenant_id = xero
            .tenant_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Xero tenant_id not configured. Run 'billing xero-auth'."))?
            .to_string();
        let refresh_token = xero
            .refresh_token
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Xero refresh_token not found. Run 'billing xero-auth'."))?
            .to_string();

        // Check if token is expired (or close to expiry — within 60 seconds)
        let needs_refresh = match &xero.token_expires_at {
            Some(exp) => {
                match chrono::DateTime::parse_from_rfc3339(exp) {
                    Ok(expiry) => {
                        let now = chrono::Utc::now();
                        expiry.with_timezone(&chrono::Utc) < now + chrono::Duration::seconds(60)
                    }
                    Err(_) => true, // treat parse failure as expired
                }
            }
            None => true, // no expiry recorded — refresh to be safe
        };

        let access_token = if needs_refresh {
            let body = format!(
                "grant_type=refresh_token&refresh_token={}",
                url_encode(&refresh_token)
            );
            let resp_text = http_post_form_basic(TOKEN_URL, &client_id, &client_secret, &body)?;
            let token_resp: XeroTokenResponse =
                serde_json::from_str(&resp_text).context("Failed to parse token refresh response")?;

            // Persist refreshed tokens
            secrets.xero.access_token = Some(token_resp.access_token.clone());
            if let Some(rt) = &token_resp.refresh_token {
                secrets.xero.refresh_token = Some(rt.clone());
            }
            if let Some(expires_in) = token_resp.expires_in {
                let expiry = chrono::Utc::now() + chrono::Duration::seconds(expires_in as i64);
                secrets.xero.token_expires_at = Some(expiry.to_rfc3339());
            }
            secrets.save()?;

            token_resp.access_token
        } else {
            xero.access_token
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("Xero access_token missing"))?
                .to_string()
        };

        Ok(Self {
            tenant_id,
            access_token,
            client_id,
            client_secret,
        })
    }

    /// Generate `(auth_url, state, verifier)` for the PKCE authorization flow.
    pub fn auth_url(client_id: &str) -> Result<(String, String, String)> {
        // Generate code_verifier: 64 random bytes, base64url-encoded (no padding)
        let mut verifier_bytes = vec![0u8; 64];
        rand::thread_rng().fill_bytes(&mut verifier_bytes);
        let verifier = BASE64URL_NOPAD.encode(&verifier_bytes);

        // code_challenge = SHA256(verifier bytes), base64url-encoded
        let challenge_bytes = Sha256::digest(verifier.as_bytes());
        let challenge = BASE64URL_NOPAD.encode(&challenge_bytes);

        let state = uuid::Uuid::new_v4().to_string();

        let auth_url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
            AUTH_URL,
            url_encode(client_id),
            url_encode(REDIRECT_URI),
            url_encode("openid profile email accounting.transactions accounting.contacts offline_access"),
            url_encode(&state),
            url_encode(&challenge),
        );

        Ok((auth_url, state, verifier))
    }

    /// Exchange auth code for tokens, fetch tenants, save to secrets.
    /// Returns the connected tenant name.
    pub fn auth_complete(
        code: &str,
        verifier: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<String> {
        let body = format!(
            "grant_type=authorization_code&code={}&redirect_uri={}&code_verifier={}",
            url_encode(code),
            url_encode(REDIRECT_URI),
            url_encode(verifier),
        );

        let resp_text = http_post_form_basic(TOKEN_URL, client_id, client_secret, &body)?;
        let token_resp: XeroTokenResponse =
            serde_json::from_str(&resp_text).context("Failed to parse token exchange response")?;

        // Fetch tenant list
        let bearer = format!("Bearer {}", token_resp.access_token);
        let tenants_text = http_get(CONNECTIONS_URL, &bearer, None)?;
        let tenants: Vec<XeroTenant> =
            serde_json::from_str(&tenants_text).context("Failed to parse Xero tenants")?;

        let tenant = tenants
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No Xero tenants found for this account"))?;

        // Persist everything
        let mut secrets = BillingSecrets::load()?;
        secrets.xero.access_token = Some(token_resp.access_token);
        if let Some(rt) = token_resp.refresh_token {
            secrets.xero.refresh_token = Some(rt);
        }
        if let Some(expires_in) = token_resp.expires_in {
            let expiry = chrono::Utc::now() + chrono::Duration::seconds(expires_in as i64);
            secrets.xero.token_expires_at = Some(expiry.to_rfc3339());
        }
        secrets.xero.tenant_id = Some(tenant.tenant_id);
        secrets.save()?;

        Ok(tenant.tenant_name)
    }

    /// Authorization header value.
    fn bearer(&self) -> String {
        format!("Bearer {}", self.access_token)
    }

    /// Find an existing Xero contact by name, or create one. Returns ContactID.
    fn find_or_create_contact(
        &self,
        name: &str,
        email: Option<&str>,
    ) -> Result<String> {
        // Search by name
        let search_url = format!(
            "{}/Contacts?where=Name%3D%3D%22{}%22",
            API_BASE,
            url_encode(name)
        );
        let resp_text = http_get(&search_url, &self.bearer(), Some(&self.tenant_id))?;
        let contacts: XeroContactsResponse =
            serde_json::from_str(&resp_text).context("Failed to parse Xero Contacts response")?;

        if let Some(contact) = contacts.Contacts.into_iter().next() {
            if let Some(id) = contact.ContactID {
                return Ok(id);
            }
        }

        // Create new contact
        let new_contact = XeroContact {
            ContactID: None,
            Name: name.to_string(),
            EmailAddress: email.map(|s| s.to_string()),
        };
        let body = serde_json::to_string(&new_contact)?;
        let create_url = format!("{}/Contacts", API_BASE);
        let resp_text = http_post(&create_url, &self.bearer(), Some(&self.tenant_id), &body)?;
        let contacts: XeroContactsResponse =
            serde_json::from_str(&resp_text).context("Failed to parse create Contact response")?;

        contacts
            .Contacts
            .into_iter()
            .next()
            .and_then(|c| c.ContactID)
            .ok_or_else(|| anyhow::anyhow!("Xero did not return a ContactID after creation"))
    }

    /// Map a Xero invoice record to an InvoiceSummary.
    fn record_to_summary(&self, rec: &XeroInvoiceRecord) -> Option<InvoiceSummary> {
        let reference = rec.InvoiceNumber.as_deref().unwrap_or("").to_string();
        if reference.is_empty() {
            return None;
        }

        let issue_date = rec.DateString.as_deref().unwrap_or("").to_string();
        let due_date = rec.DueDateString.as_deref().unwrap_or("").to_string();
        let total = rec.AmountDue.unwrap_or(0.0);
        let currency = rec.CurrencyCode.as_deref().unwrap_or("GBP").to_string();
        let client_id = rec.Reference.as_deref().unwrap_or("").to_string();
        let client_name = rec
            .Contact
            .as_ref()
            .map(|c| c.Name.clone())
            .unwrap_or_default();
        let bill_to_email = rec
            .Contact
            .as_ref()
            .and_then(|c| c.EmailAddress.clone());

        let today = Local::now().date_naive();
        let due_naive = NaiveDate::parse_from_str(&due_date, "%Y-%m-%d").unwrap_or(today);
        let days_overdue = if today > due_naive {
            (today - due_naive).num_days()
        } else {
            0
        };

        let xero_status = rec.Status.as_deref().unwrap_or("");
        let state = xero_status_to_state(xero_status, days_overdue);

        Some(InvoiceSummary {
            reference,
            client_id,
            client_name: client_name.clone(),
            bill_to_name: client_name,
            bill_to_email,
            total,
            currency,
            issue_date,
            due_date,
            state,
            days_overdue,
            payment_link: None,
            reminders_sent: 0,
            last_reminder: None,
        })
    }
}

/// Map Xero status string + overdue state to InvoiceState.
fn xero_status_to_state(status: &str, days_overdue: i64) -> InvoiceState {
    match status {
        "DRAFT" => InvoiceState::Draft,
        "SUBMITTED" | "AUTHORISED" => {
            if days_overdue > 0 {
                InvoiceState::Overdue
            } else {
                InvoiceState::Sent
            }
        }
        "PAID" => InvoiceState::Paid,
        "VOIDED" | "DELETED" => InvoiceState::Cancelled,
        _ => InvoiceState::Draft,
    }
}

/// Minimal percent-encoding for URL query parameters.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", b));
            }
        }
    }
    out
}

impl AccountingProvider for XeroProvider {
    fn create_invoice(&self, invoice: &Invoice) -> Result<InvoiceRef> {
        let contact_id = self.find_or_create_contact(
            &invoice.client_name,
            invoice.bill_to.email(),
        )?;

        let line_items: Vec<XeroLineItem> = invoice
            .line_items
            .iter()
            .map(|li| XeroLineItem {
                Description: li.description.clone(),
                Quantity: li.quantity as f64,
                UnitAmount: li.unit_amount,
                AccountCode: "200".to_string(), // default sales account
            })
            .collect();

        let xero_inv = XeroInvoiceCreate {
            Type_: "ACCREC".to_string(),
            Contact: XeroContact {
                ContactID: Some(contact_id),
                Name: invoice.client_name.clone(),
                EmailAddress: None,
            },
            Date: invoice.issue_date.clone(),
            DueDate: invoice.due_date.clone(),
            InvoiceNumber: invoice.reference.clone(),
            CurrencyCode: invoice.currency.clone(),
            Status: "AUTHORISED".to_string(),
            LineItems: line_items,
            Reference: invoice.client_id.clone(), // store client_id for later retrieval
        };

        let body = serde_json::to_string(&xero_inv)?;
        let url = format!("{}/Invoices", API_BASE);
        let resp_text = http_post(&url, &self.bearer(), Some(&self.tenant_id), &body)?;
        let resp: XeroInvoiceResponse =
            serde_json::from_str(&resp_text).context("Failed to parse create Invoice response")?;

        let xero_id = resp
            .Invoices
            .into_iter()
            .next()
            .and_then(|r| r.InvoiceID)
            .ok_or_else(|| anyhow::anyhow!("Xero did not return an InvoiceID"))?;

        Ok(InvoiceRef {
            reference: invoice.reference.clone(),
            file_path: Some(xero_id),
        })
    }

    fn get_invoice(&self, reference: &str) -> Result<Option<InvoiceSummary>> {
        let url = format!("{}/Invoices/{}", API_BASE, url_encode(reference));
        let resp_text = match http_get(&url, &self.bearer(), Some(&self.tenant_id)) {
            Ok(t) => t,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("404") {
                    return Ok(None);
                }
                return Err(e);
            }
        };
        let resp: XeroInvoiceResponse =
            serde_json::from_str(&resp_text).context("Failed to parse get Invoice response")?;

        Ok(resp
            .Invoices
            .iter()
            .next()
            .and_then(|r| self.record_to_summary(r)))
    }

    fn list_invoices(&self, filter: InvoiceFilter) -> Result<Vec<InvoiceSummary>> {
        let url = format!(
            "{}/Invoices?where=Type%3D%3D%22ACCREC%22&page=1",
            API_BASE
        );
        let resp_text = http_get(&url, &self.bearer(), Some(&self.tenant_id))?;
        let resp: XeroInvoiceResponse =
            serde_json::from_str(&resp_text).context("Failed to parse list Invoices response")?;

        let summaries: Vec<InvoiceSummary> = resp
            .Invoices
            .iter()
            .filter_map(|r| self.record_to_summary(r))
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

    fn mark_paid(&self, reference: &str, date: &str, amount: Option<f64>) -> Result<()> {
        // First, look up the InvoiceID
        let url = format!("{}/Invoices/{}", API_BASE, url_encode(reference));
        let resp_text = http_get(&url, &self.bearer(), Some(&self.tenant_id))?;
        let resp: XeroInvoiceResponse =
            serde_json::from_str(&resp_text).context("Failed to parse Invoice response for mark_paid")?;

        let rec = resp
            .Invoices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("Invoice {} not found in Xero", reference))?;

        let invoice_id = rec
            .InvoiceID
            .ok_or_else(|| anyhow::anyhow!("No InvoiceID for {}", reference))?;

        let total = rec.AmountDue.unwrap_or(0.0);
        let payment_amount = amount.unwrap_or(total);

        let body = serde_json::json!({
            "Invoice": { "InvoiceID": invoice_id },
            "Account": { "Code": "090" },
            "Date": date,
            "Amount": payment_amount
        })
        .to_string();

        let url = format!("{}/Payments", API_BASE);
        http_post(&url, &self.bearer(), Some(&self.tenant_id), &body)?;

        Ok(())
    }

    fn cancel_invoice(&self, reference: &str, _reason: &str) -> Result<()> {
        // Look up InvoiceID
        let url = format!("{}/Invoices/{}", API_BASE, url_encode(reference));
        let resp_text = http_get(&url, &self.bearer(), Some(&self.tenant_id))?;
        let resp: XeroInvoiceResponse =
            serde_json::from_str(&resp_text).context("Failed to parse Invoice for cancel")?;

        let rec = resp
            .Invoices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("Invoice {} not found in Xero", reference))?;

        let invoice_id = rec
            .InvoiceID
            .ok_or_else(|| anyhow::anyhow!("No InvoiceID for {}", reference))?;

        let body = serde_json::json!({
            "Invoices": [{ "InvoiceID": invoice_id, "Status": "VOIDED" }]
        })
        .to_string();

        let url = format!("{}/Invoices/{}", API_BASE, url_encode(&invoice_id));
        http_put(&url, &self.bearer(), Some(&self.tenant_id), &body)?;

        Ok(())
    }

    fn next_invoice_number(&self) -> Result<String> {
        let year = Local::now().format("%Y");
        let prefix = format!("INV-{}-", year);

        let url = format!("{}/Invoices?where=Type%3D%3D%22ACCREC%22&page=1", API_BASE);
        let resp_text = http_get(&url, &self.bearer(), Some(&self.tenant_id))?;
        let resp: XeroInvoiceResponse =
            serde_json::from_str(&resp_text).context("Failed to parse Invoices for number generation")?;

        let max_num = resp
            .Invoices
            .iter()
            .filter_map(|r| r.InvoiceNumber.as_deref())
            .filter(|n| n.starts_with(&prefix))
            .filter_map(|n| n.strip_prefix(&prefix).and_then(|s| s.parse::<u32>().ok()))
            .max()
            .unwrap_or(0);

        Ok(format!("{}{:04}", prefix, max_num + 1))
    }

    fn invoiced_dates_for_client(&self, client_id: &str) -> Result<Vec<String>> {
        // Filter by Reference field (which stores our client_id)
        let url = format!(
            "{}/Invoices?where=Reference%3D%3D%22{}%22&page=1",
            API_BASE,
            url_encode(client_id)
        );
        let resp_text = http_get(&url, &self.bearer(), Some(&self.tenant_id))?;
        let resp: XeroInvoiceResponse =
            serde_json::from_str(&resp_text).context("Failed to parse Invoices for invoiced_dates")?;

        // Xero doesn't store our internal session_date per line item —
        // extract description dates from line items using the same format
        // as LineItem::description ("... — YYYY-MM-DD")
        let dates: Vec<String> = resp
            .Invoices
            .iter()
            .filter(|r| {
                // Exclude voided/deleted
                !matches!(r.Status.as_deref(), Some("VOIDED") | Some("DELETED"))
            })
            .flat_map(|r| {
                r.LineItems.as_deref().unwrap_or(&[]).iter().filter_map(|li| {
                    li.Description.as_deref().and_then(|desc| {
                        // Extract date from "... — YYYY-MM-DD"
                        desc.rsplit(" — ").next().and_then(|d| {
                            if NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d").is_ok() {
                                Some(d.trim().to_string())
                            } else {
                                None
                            }
                        })
                    })
                })
            })
            .collect();

        Ok(dates)
    }
}

// ---------------------------------------------------------------------------
// Local OAuth2 callback server
// ---------------------------------------------------------------------------

/// Bind a TCP listener, accept one HTTP request, extract `?code=`, return it.
pub fn run_oauth_callback_server(port: u16) -> Result<String> {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind(format!("127.0.0.1:{}", port))
        .with_context(|| format!("Cannot bind to port {}", port))?;

    println!("Waiting for OAuth2 callback on http://localhost:{}/callback ...", port);

    let (mut stream, _addr) = listener.accept().context("Failed to accept connection")?;

    let mut reader = BufReader::new(&stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    // GET /callback?code=xxx&state=yyy HTTP/1.1
    let code = request_line
        .split_whitespace()
        .nth(1) // path + query string
        .and_then(|path| {
            path.find('?')
                .map(|pos| &path[pos + 1..])
                .and_then(|query| {
                    query.split('&').find_map(|param| {
                        let mut parts = param.splitn(2, '=');
                        if parts.next() == Some("code") {
                            parts.next().map(|v| v.to_string())
                        } else {
                            None
                        }
                    })
                })
        })
        .ok_or_else(|| anyhow::anyhow!("No 'code' parameter in OAuth2 callback"))?;

    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n\
        <html><body><h1>Authorization complete</h1>\
        <p>You can close this tab and return to the terminal.</p>\
        </body></html>";
    stream.write_all(response.as_bytes())?;

    Ok(code)
}
