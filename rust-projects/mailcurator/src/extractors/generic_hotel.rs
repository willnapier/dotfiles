//! Generic hotel booking extractor for vendors with similar structure.
//!
//! Used by Marriott, Premier Inn, Travelodge, Hotels.com, Agoda, Expedia.
//! Covers most chain-hotel email layouts where the data is reasonably
//! standardised across the industry: check-in / check-out / property /
//! confirmation number / total. Vendors with distinctive layouts get
//! their own dedicated module (booking_com, airbnb).
//!
//! The vendor name is set per-policy via a `[[policy.extractor.field]]`
//! literal field — extractor itself is vendor-agnostic.

use anyhow::Result;
use mailparse::{MailHeaderMap, ParsedMail};
use regex::Regex;
use scraper::Html;
use serde_json::{Map, Value};
use std::sync::OnceLock;

use super::currency;
use super::VendorExtractor;

const GENERIC_HOTEL_LLM_SCHEMA: &str = r#"{
  "checkin": "string|null — check-in date as written, e.g. \"24 April 2026\" or \"Saturday, 4 July 2026\"",
  "checkin_time": "string|null — check-in time HH:MM if shown",
  "checkout": "string|null — check-out date in the same format",
  "checkout_time": "string|null — check-out time HH:MM if shown",
  "guests": "string|null — guest count, e.g. \"2 adults\", \"1 adult, 2 children\"",
  "nights": "string|null — number of nights as integer",
  "rooms": "string|null — number of rooms as integer",
  "property": "string|null — hotel/inn name (often in subject)",
  "location": "string|null — city/town",
  "booking_ref": "string|null — confirmation / reservation / itinerary number",
  "total": "string|null — total cost as decimal. DO NOT include currency symbol — populate `currency` separately.",
  "currency": "string|null — ISO 4217 currency code: GBP, EUR, USD, etc. Detect from £/€/$ markers; default GBP for UK chains.",
  "property_url": "string|null — direct link to the hotel page (any URL in the email that points at the hotel/listing on the vendor site, with no query params if possible)"
}"#;

pub struct GenericHotel;

impl VendorExtractor for GenericHotel {
    fn name(&self) -> &'static str {
        "generic_hotel"
    }

    fn required_fields(&self) -> &'static [&'static str] {
        &["checkin", "checkout"]
    }

    fn llm_schema(&self) -> Option<&'static str> {
        Some(GENERIC_HOTEL_LLM_SCHEMA)
    }

    fn validate_field(&self, field: &str, value: &Value) -> bool {
        match (field, value) {
            ("booking_ref", Value::String(s)) => {
                // Hotel reservation numbers vary widely (Marriott uses
                // 9-digit numerics, others alphanumeric). Just bound length.
                !s.is_empty() && s.len() <= 30 && s.chars().any(|c| c.is_alphanumeric())
            }
            ("total", Value::String(s)) => {
                s.parse::<f64>().map(|f| (0.0..=100_000.0).contains(&f)).unwrap_or(false)
            }
            ("checkin_time" | "checkout_time", Value::String(s)) => {
                static R: OnceLock<Regex> = OnceLock::new();
                let re = R.get_or_init(|| Regex::new(r"^\d{2}:\d{2}$").unwrap());
                re.is_match(s)
            }
            ("property" | "location", Value::String(s)) => {
                !s.is_empty() && s.len() < 120 && s.chars().any(|c| c.is_alphabetic())
            }
            _ => match value {
                Value::String(s) => !s.is_empty(),
                Value::Null => false,
                _ => true,
            },
        }
    }

    fn extract(&self, parsed: &ParsedMail, html: &str) -> Result<Map<String, Value>> {
        let mut out = Map::new();
        let subject = parsed.headers.get_first_value("Subject").unwrap_or_default();

        // Property name is often "<Hotel Name> - <Subject suffix>" or
        // just appears at the start of the subject for chain hotels.
        if let Some(p) = property_from_subject(&subject) {
            out.insert("property".into(), Value::String(p));
        }

        if !html.is_empty() {
            // Property URL extraction works on raw HTML (href values).
            // Hotel-page URLs vary per vendor but reliably contain
            // /hotel /hotels /property /rooms /stays in the path AND
            // exclude tracking subdomains (click, link, e, t, mail).
            if let Some(url) = find_hotel_url(html) {
                out.insert("property_url".into(), Value::String(url));
            }
            let text = strip_to_text(html);

            // Check-in / Check-out variations:
            //   "Check-in: Friday, 24 April 2026"
            //   "Arriving 24 April 2026"
            //   "Check in 24 Apr 2026, 15:00"
            if let Some(d) = first_capture(&checkin_re(), &text) {
                out.insert("checkin".into(), Value::String(d));
            }
            if let Some(d) = first_capture(&checkout_re(), &text) {
                out.insert("checkout".into(), Value::String(d));
            }
            if let Some((c, v)) = currency::find_money(&text) {
                out.insert("total".into(), Value::String(v));
                out.insert("currency".into(), Value::String(c));
            }
            if let Some(c) = first_capture(&confirmation_re(), &text) {
                out.insert("booking_ref".into(), Value::String(c));
            }
            if let Some(g) = first_capture(&guests_re(), &text) {
                out.insert("guests".into(), Value::String(g));
            }
            if let Some(n) = first_capture(&nights_re(), &text) {
                out.insert("nights".into(), Value::String(n));
            }
        }

        Ok(out)
    }
}

fn property_from_subject(subject: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| {
        // Common chain-hotel subject patterns:
        //   "Leeds Marriott Hotel Copy of Stay Folio for 21-12-23"  → Leeds Marriott Hotel
        //   "Booking confirmation: <Hotel>"                          → <Hotel>
        //   "Your reservation at <Hotel>"                            → <Hotel>
        //   "<Hotel>: Your stay confirmation"                        → <Hotel>
        Regex::new(r"(?i)(?:reservation at|booking at|confirmation:|stay at|your stay at|booking confirmation for|confirmed at)\s+(.+?)(?:\s*$|[,\.])")
            .unwrap()
    });
    if let Some(m) = re.captures(subject).and_then(|c| c.get(1)) {
        return Some(m.as_str().trim().to_string());
    }
    // Fallback heuristic: subject starts with hotel name followed by
    // "Hotel" and a colon/dash separator.
    static FALLBACK_RE: OnceLock<Regex> = OnceLock::new();
    let fb = FALLBACK_RE.get_or_init(|| {
        Regex::new(r"^([\w\s'&]+?(?:Hotel|Inn|Lodge|Resort|Suites|Manor|House|B&B))\b")
            .unwrap()
    });
    fb.captures(subject)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
}

fn checkin_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)(?:Check[\-\s]?in|Arrival|Arriving)[:\s]+([\w\s,]+?\d{4})",
        )
        .unwrap()
    })
}

fn checkout_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)(?:Check[\-\s]?out|Departure|Departing|Leaving)[:\s]+([\w\s,]+?\d{4})",
        )
        .unwrap()
    })
}

fn total_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)(?:Total|Grand Total|Total cost|Total price|You paid)[:\s]+£\s*(\d+(?:\.\d{2})?)")
            .unwrap()
    })
}

fn confirmation_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        // "Confirmation: ABC123", "Reservation 12345678", "Itinerary #12345",
        // "Booking ref ABC123"
        Regex::new(
            r"(?i)(?:Confirmation\s*[#:]?|Reservation\s*[#:]?|Itinerary\s*[#:]?|Booking\s+(?:ref(?:erence)?|number)\s*[:#]?)\s*([A-Z0-9-]{6,20})",
        )
        .unwrap()
    })
}

fn guests_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(\d+\s+(?:adult|guest)s?(?:[,\s]+\d+\s+(?:child|children))?)").unwrap()
    })
}

fn nights_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(\d+)\s+nights?\b").unwrap())
}

/// Find the most likely hotel-page URL in raw HTML. Strategy:
/// 1. Extract all URLs from href attributes.
/// 2. Filter out tracking/promotional/asset URLs (click., link., e., t.,
///    mail., utm_*, .png, .gif, .jpg, mailto:, tel:).
/// 3. Prefer URLs whose path contains hotel-shaped tokens
///    (`/hotel/`, `/hotels/`, `/property/`, `/rooms/`, `/stays/`,
///    `/lodging/`, `/inn/`, `/resort/`).
/// 4. Among matches, prefer the shortest (typically the canonical
///    listing URL without tracking parameters).
fn find_hotel_url(html: &str) -> Option<String> {
    static HREF_RE: OnceLock<Regex> = OnceLock::new();
    let href = HREF_RE.get_or_init(|| {
        Regex::new(r#"href\s*=\s*["']([^"']+)["']"#).unwrap()
    });
    static HOTEL_PATH_RE: OnceLock<Regex> = OnceLock::new();
    let hotel_path = HOTEL_PATH_RE.get_or_init(|| {
        Regex::new(r"(?i)/(hotel|hotels|property|properties|rooms?|stays?|lodging|inn|resort|listing)s?/").unwrap()
    });
    static TRACKING_HOST_RE: OnceLock<Regex> = OnceLock::new();
    let tracking_host = TRACKING_HOST_RE.get_or_init(|| {
        Regex::new(r"^https?://(?:click|link|t|e|mail|track|tracking|email|mailer|m|news|notify|comms|broadcast)\.").unwrap()
    });

    let mut candidates: Vec<&str> = Vec::new();
    for caps in href.captures_iter(html) {
        if let Some(m) = caps.get(1) {
            let url = m.as_str();
            // Reject obvious non-page URLs.
            if !url.starts_with("http") {
                continue;
            }
            if tracking_host.is_match(url) {
                continue;
            }
            // Reject image/asset URLs.
            if url.ends_with(".png")
                || url.ends_with(".gif")
                || url.ends_with(".jpg")
                || url.ends_with(".jpeg")
                || url.ends_with(".svg")
                || url.ends_with(".css")
                || url.ends_with(".js")
            {
                continue;
            }
            if !hotel_path.is_match(url) {
                continue;
            }
            candidates.push(url);
        }
    }
    // Prefer the shortest candidate (typically canonical, no tracking
    // query params).
    candidates.sort_by_key(|u| u.len());
    candidates.first().map(|s| {
        // Strip query params for cleanliness.
        s.split('?').next().unwrap_or(s).to_string()
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
    fn property_from_marriott_subject() {
        let raw = "Subject: Leeds Marriott Hotel Copy of Stay Folio for 21-12-23\n\n";
        let parsed = parse_test(raw);
        let r = GenericHotel.extract(&parsed, "").unwrap();
        assert_eq!(
            r.get("property").and_then(|v| v.as_str()),
            Some("Leeds Marriott Hotel")
        );
    }
}
