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

pub mod airbnb_bookings;
pub mod amazon_orders;
pub mod booking_com_bookings;
pub mod generic_hotel;
pub mod tesla;
pub mod trainline_journeys;

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

    /// JSON Schema describing the LLM-fallback output shape for this
    /// vendor. When `Some`, the extractor framework will call
    /// `llm::extract_structured` with this schema for any required field
    /// that the deterministic `extract` left missing. Must align with the
    /// field names returned by `extract` so the cached LLM output merges
    /// cleanly into the same record.
    ///
    /// Default `None` = no LLM fallback for this vendor.
    fn llm_schema(&self) -> Option<&'static str> {
        None
    }

    /// Per-field validator. Called on every value the LLM proposes for a
    /// field belonging to this extractor; if it returns false, the field
    /// is rejected (treated as if the LLM hadn't returned it). Use this
    /// to catch hallucinations — e.g. for `order_id`, accept only the
    /// canonical `\d{3}-\d{7}-\d{7}` form. Default accepts any
    /// non-empty string.
    #[allow(unused_variables)] // default impl ignores both args
    fn validate_field(&self, field: &str, value: &Value) -> bool {
        match value {
            Value::String(s) => !s.is_empty(),
            Value::Null => false,
            _ => true,
        }
    }

    /// Decide whether to invoke the LLM fallback for this message,
    /// given the deterministic fields already extracted. Default impl
    /// fires when any `required_fields` entry is missing — the standard
    /// case for vendors like Amazon (orders) and Trainline (journeys)
    /// where a single set of required fields applies to every matched
    /// message.
    ///
    /// Override for vendors with heterogeneous email mixes (e.g. Tesla:
    /// auth codes, service appointments, supercharger receipts) where
    /// the relevant fields differ per email-type. Tesla overrides this
    /// to fire only on service/supercharger/subscription kinds when
    /// amount is missing.
    fn wants_llm_fallback(&self, deterministic_fields: &Map<String, Value>) -> bool {
        let required = self.required_fields();
        if required.is_empty() {
            return false;
        }
        required.iter().any(|f| {
            !is_populated(deterministic_fields.get(*f))
        })
    }
}

fn is_populated(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        _ => true,
    }
}

/// Dispatch a vendor module by name. Returns None for unknown names —
/// caller falls back to the generic FieldRule loop.
pub fn dispatch(name: &str) -> Option<Box<dyn VendorExtractor>> {
    match name {
        "amazon_orders" => Some(Box::new(amazon_orders::AmazonOrders)),
        "trainline_journeys" => Some(Box::new(trainline_journeys::TrainlineJourneys)),
        "airbnb_bookings" => Some(Box::new(airbnb_bookings::AirbnbBookings)),
        "tesla" => Some(Box::new(tesla::Tesla)),
        "booking_com_bookings" => Some(Box::new(booking_com_bookings::BookingComBookings)),
        "generic_hotel" => Some(Box::new(generic_hotel::GenericHotel)),
        _ => None,
    }
}

/// All known extractor names, for `coverage --list` and validation.
pub fn known_extractors() -> &'static [&'static str] {
    &[
        "amazon_orders",
        "trainline_journeys",
        "airbnb_bookings",
        "tesla",
        "booking_com_bookings",
        "generic_hotel",
    ]
}
