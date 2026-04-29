//! Per-vendor structured HTML extractors.
//!
//! Vendor modules implement [`VendorExtractor`] to pull structured fields
//! from HTML emails using CSS selectors / DOM walks. This is more robust
//! than regex on stripped text for vendors with consistent template
//! structure (Amazon, Airbnb, Trainline, Tesla).
//!
//! Policies opt in via `vendor_module = "amazon_orders"` in policies.toml.
//! When set, the module's fields are merged into the extracted record
//! BEFORE the generic FieldRule loop runs. Generic rules can still
//! supplement with literal/header values.
//!
//! Required-field reporting feeds the `coverage` subcommand: the per-policy
//! population rate of [`VendorExtractor::required_fields`] tells us when a
//! vendor has changed their template (drift detection — see Session 3).

use anyhow::Result;
use mailparse::ParsedMail;
use serde_json::{Map, Value};

pub mod amazon_orders;

/// Trait every vendor extractor implements.
///
/// `parsed`: the full RFC822 message (for header access).
/// `html`: the raw, undecoded HTML body if the message has a `text/html`
/// part, empty otherwise. Quoted-printable / base64 transfer encodings are
/// already decoded by `mailparse` upstream — but `=\n` soft-line-breaks may
/// still need to be handled depending on the parser. Use the `scraper` crate
/// for DOM access; fall back to regex on `html` for fields that are easier
/// to anchor textually.
pub trait VendorExtractor {
    /// Module name as referenced in `policies.toml::vendor_module`.
    fn name(&self) -> &'static str;

    /// Fields this extractor commits to populating. The `coverage`
    /// subcommand reports the % of extracted records that have ALL of
    /// these populated. Drift = sustained drop in this rate.
    fn required_fields(&self) -> &'static [&'static str];

    /// Pull structured fields. Return `Ok(Map)` even on partial failure;
    /// missing fields are simply absent from the map. Errors should be
    /// reserved for unrecoverable cases (malformed input that crashes the
    /// HTML parser, etc.) — extractor batches must not stop on one bad
    /// message.
    fn extract(&self, parsed: &ParsedMail, html: &str) -> Result<Map<String, Value>>;
}

/// Dispatch a vendor module by name. Returns None for unknown names —
/// caller falls back to the generic FieldRule loop.
pub fn dispatch(name: &str) -> Option<Box<dyn VendorExtractor>> {
    match name {
        "amazon_orders" => Some(Box::new(amazon_orders::AmazonOrders)),
        _ => None,
    }
}

/// All known extractor names, for `coverage --list` and validation.
pub fn known_extractors() -> &'static [&'static str] {
    &["amazon_orders"]
}
