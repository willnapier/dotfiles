//! Tesla email extractor.
//!
//! Tesla emails span auth flows, service appointments, supercharger
//! receipts, software-release announcements, and marketing. Most are
//! transient (auth codes, password resets) — the high-value subset is
//! service appointments and supercharger receipts where amounts and dates
//! matter for the paper trail.
//!
//! Tessie (lifetime subscription, see project memory) is the primary live
//! data source for vehicle records — this extractor just keeps the email
//! audit trail useful for years to come.

use anyhow::Result;
use mailparse::{MailHeaderMap, ParsedMail};
use regex::Regex;
use scraper::Html;
use serde_json::{Map, Value};
use std::sync::OnceLock;

use super::VendorExtractor;

/// LLM fallback schema. The Tesla email mix is heterogeneous (auth,
/// service, supercharger, software, marketing) so we don't gate on any
/// single field — the LLM is asked to populate whichever apply.
const TESLA_LLM_SCHEMA: &str = r#"{
  "amount": "string|null — pound amount as decimal e.g. \"24.99\". For supercharger receipts, service charges, subscription renewals.",
  "service_date": "string|null — appointment / scheduled date as written in the email",
  "location": "string|null — service centre or supercharger station name and city",
  "kwh": "string|null — energy delivered in kWh (supercharger sessions only)",
  "vehicle": "string|null — vehicle name or model identifier if mentioned"
}"#;

pub struct Tesla;

impl VendorExtractor for Tesla {
    fn name(&self) -> &'static str {
        "tesla"
    }

    fn required_fields(&self) -> &'static [&'static str] {
        // No single field is reliable across Tesla's email mix. Required-list
        // is intentionally empty — health is reported as "no module" rather
        // than fail-bait. Drift detection (Session 3) keys off any field's
        // population rate trend instead.
        &[]
    }

    fn llm_schema(&self) -> Option<&'static str> {
        // Even though required_fields is empty, the LLM fallback CAN still
        // fire — but only when the deterministic extractor produces a
        // record with all required fields missing (which is vacuously
        // true here). We gate on `kind`-being-non-trivial in extract()
        // so the LLM only triggers for service/supercharger/subscription
        // emails where there's something to extract.
        Some(TESLA_LLM_SCHEMA)
    }

    fn validate_field(&self, field: &str, value: &Value) -> bool {
        match (field, value) {
            ("amount", Value::String(s)) => {
                s.parse::<f64>().map(|f| (0.0..=100_000.0).contains(&f)).unwrap_or(false)
            }
            ("kwh", Value::String(s)) => {
                s.parse::<f64>().map(|f| (0.0..=200.0).contains(&f)).unwrap_or(false)
            }
            (_, Value::String(s)) => !s.is_empty() && s.len() < 200,
            (_, Value::Null) => false,
            _ => true,
        }
    }

    /// Tesla's email mix is heterogeneous; required_fields is empty.
    /// Fire LLM fallback selectively: only on service appointments,
    /// supercharger receipts, and subscription events where there's
    /// a financial or scheduling fact to capture. Auth codes /
    /// access-changes / software releases / "other" are skipped — the
    /// classifier-derived `kind` is sufficient for those.
    fn wants_llm_fallback(&self, det: &Map<String, Value>) -> bool {
        let kind = det.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let high_value_kind = matches!(kind, "service" | "supercharger" | "subscription");
        if !high_value_kind {
            return false;
        }
        // Already populated by deterministic? skip.
        let amount_known = det.get("amount").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty());
        let date_known = det.get("service_date").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty());
        !(amount_known && date_known)
    }

    fn extract(&self, parsed: &ParsedMail, html: &str) -> Result<Map<String, Value>> {
        let mut out = Map::new();
        // Tesla UK — payment receipts and supercharger charges in GBP.
        out.insert("currency".into(), Value::String("GBP".into()));
        let subject = parsed.headers.get_first_value("Subject").unwrap_or_default();

        // Classify the email by subject — gives downstream queries a
        // structured handle.
        let kind = classify(&subject);
        out.insert("kind".into(), Value::String(kind.to_string()));

        if !html.is_empty() {
            let text = strip_to_text(html);
            if let Some(amount) = first_capture(&amount_re(), &text) {
                out.insert("amount".into(), Value::String(amount));
            }
            if let Some(d) = first_capture(&date_re(), &text) {
                out.insert("service_date".into(), Value::String(d));
            }
            if let Some(loc) = first_capture(&location_re(), &text) {
                out.insert("location".into(), Value::String(loc));
            }
        }

        Ok(out)
    }
}

fn classify(subject: &str) -> &'static str {
    let s = subject.to_lowercase();
    if s.contains("verification code") || s.contains("password") {
        "auth"
    } else if s.contains("appointment") || s.contains("service") {
        "service"
    } else if s.contains("supercharg") {
        "supercharger"
    } else if s.contains("granted") || s.contains("removed") || s.contains("access") {
        "access-change"
    } else if s.contains("subscribed") || s.contains("subscription") {
        "subscription"
    } else if s.contains("update") || s.contains("software") || s.contains("release") {
        "software"
    } else {
        "other"
    }
}

fn amount_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"£\s*(\d+\.\d{2})").unwrap())
}

fn date_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"\b(\d{1,2}\s+(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{4})\b",
        )
        .unwrap()
    })
}

fn location_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)(?:Service Center|Service Centre|Location)[:\s]+([\w\s,]+?)(?:[\.\n]|$)")
            .unwrap()
    })
}

fn first_capture(re: &Regex, text: &str) -> Option<String> {
    re.captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
}

fn strip_to_text(html: &str) -> String {
    let doc = Html::parse_document(html);
    let mut out = String::new();
    for node in doc.root_element().text() {
        let t = node.trim();
        if !t.is_empty() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(t);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_test(raw: &str) -> ParsedMail<'_> {
        mailparse::parse_mail(raw.as_bytes()).unwrap()
    }

    #[test]
    fn classify_auth() {
        let raw = "Subject: Tesla Verification Code: 266264\nFrom: a@tesla.com\n\n";
        let parsed = parse_test(raw);
        let r = Tesla.extract(&parsed, "").unwrap();
        assert_eq!(r.get("kind").and_then(|v| v.as_str()), Some("auth"));
    }

    #[test]
    fn classify_subscription() {
        let raw = "Subject: You've Subscribed to Premium Connectivity\nFrom: a@tesla.com\n\n";
        let parsed = parse_test(raw);
        let r = Tesla.extract(&parsed, "").unwrap();
        assert_eq!(r.get("kind").and_then(|v| v.as_str()), Some("subscription"));
    }

    #[test]
    fn amount_extracted() {
        let html = r#"<p>You paid £24.99 for premium connectivity.</p>"#;
        let raw = "Subject: receipt\n\n";
        let parsed = parse_test(raw);
        let r = Tesla.extract(&parsed, html).unwrap();
        assert_eq!(r.get("amount").and_then(|v| v.as_str()), Some("24.99"));
    }
}
