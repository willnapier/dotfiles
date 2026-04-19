//! Stripe PaymentProvider — creates payment links for self-pay invoices.
//!
//! Uses the Stripe REST API directly via reqwest::blocking (off-Tokio-thread).

use anyhow::{Context, Result};
use serde::Deserialize;

use super::invoice::Invoice;
use super::secrets::BillingSecrets;
use super::traits::PaymentProvider;

// ---------------------------------------------------------------------------
// HTTP helper
// ---------------------------------------------------------------------------

fn stripe_post_form(url: &str, secret_key: &str, body: &str) -> Result<String> {
    let (url, secret_key, body) = (url.to_string(), secret_key.to_string(), body.to_string());
    std::thread::spawn(move || -> anyhow::Result<String> {
        let resp = reqwest::blocking::Client::new()
            .post(&url)
            .header("Authorization", format!("Bearer {}", secret_key))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            anyhow::bail!("Stripe {} {}: {}", url, status, body);
        }
        Ok(resp.text()?)
    })
    .join()
    .map_err(|_| anyhow::anyhow!("HTTP thread panicked"))?
}

// ---------------------------------------------------------------------------
// Stripe API response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct StripePriceResponse {
    id: String,
}

#[derive(Debug, Deserialize)]
struct StripePaymentLinkResponse {
    url: String,
}

// ---------------------------------------------------------------------------
// StripeProvider
// ---------------------------------------------------------------------------

/// PaymentProvider backed by Stripe.
pub struct StripeProvider {
    secret_key: String,
}

impl StripeProvider {
    /// Load secret key from secrets.toml.
    pub fn new() -> Result<Self> {
        let secrets = BillingSecrets::load()?;
        let secret_key = secrets
            .stripe
            .secret_key
            .ok_or_else(|| anyhow::anyhow!("Stripe secret_key not configured. Run 'billing stripe-key <key>'."))?;
        Ok(Self { secret_key })
    }
}

impl PaymentProvider for StripeProvider {
    fn create_payment_link(&self, invoice: &Invoice) -> Result<Option<String>> {
        let total_pence = (invoice.total() * 100.0).round() as u64;
        let currency = invoice.currency.to_lowercase();
        let product_name = format!(
            "Invoice {} — {}",
            invoice.reference, invoice.client_name
        );

        // Step 1: Create a price
        let price_body = format!(
            "unit_amount={}&currency={}&product_data[name]={}",
            total_pence,
            url_encode(&currency),
            url_encode(&product_name),
        );
        let price_resp_text = stripe_post_form(
            "https://api.stripe.com/v1/prices",
            &self.secret_key,
            &price_body,
        )?;
        let price: StripePriceResponse =
            serde_json::from_str(&price_resp_text).context("Failed to parse Stripe price response")?;

        // Step 2: Create a payment link
        let link_body = format!(
            "line_items[0][price]={}&line_items[0][quantity]=1",
            url_encode(&price.id),
        );
        let link_resp_text = stripe_post_form(
            "https://api.stripe.com/v1/payment_links",
            &self.secret_key,
            &link_body,
        )?;
        let link: StripePaymentLinkResponse =
            serde_json::from_str(&link_resp_text).context("Failed to parse Stripe payment link response")?;

        Ok(Some(link.url))
    }

    fn provider_name(&self) -> &str {
        "Stripe"
    }
}

/// Minimal percent-encoding for form values.
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
