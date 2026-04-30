//! Airbnb bookings extractor.
//!
//! Reservation-shaped subjects ("Reservation for X, DD–DD Mmm") encode
//! check-in / check-out dates compactly, often more reliably than the body
//! HTML. Body extraction supplements when the subject lacks dates.

use anyhow::Result;
use mailparse::{MailHeaderMap, ParsedMail};
use regex::Regex;
use scraper::Html;
use serde_json::{Map, Value};
use std::sync::OnceLock;

use super::currency;
use super::VendorExtractor;

const AIRBNB_LLM_SCHEMA: &str = r#"{
  "checkin": "string|null — check-in date as written, e.g. \"Friday 24 April 2026\" or \"24 Apr 2026\"",
  "checkin_time": "string|null — arrive-after time on checkin day, HH:MM e.g. \"16:00\". Often shown as \"After 16:00\" or \"4:00 PM\".",
  "checkout": "string|null — check-out date, same format as checkin",
  "checkout_time": "string|null — depart-by time, HH:MM e.g. \"10:00\". Often shown as \"Before 10:00\" or \"10:00 AM\".",
  "guests": "string|null — guest count summary, e.g. \"5 adults\", \"2 adults, 1 child\"",
  "nights": "string|null — number of nights stayed if mentioned, just the integer e.g. \"3\"",
  "host": "string|null — host's first name",
  "property": "string|null — listing name (e.g. \"Brook Cottage\", \"Carninney Lane\")",
  "property_url": "string|null — direct link to the listing page on Airbnb, e.g. \"https://www.airbnb.co.uk/rooms/12345678\". Strip query parameters.",
  "booking_ref": "string|null — Airbnb confirmation code, typically HMxxxxxxxx",
  "total": "string|null — total paid as decimal e.g. \"450.00\". DO NOT include currency symbol — populate `currency` separately.",
  "currency": "string|null — ISO 4217 currency code: GBP, EUR, USD, etc. Detect from £/€/$ markers in the email; default GBP for UK hosts.",
  "cleaning_fee": "string|null — cleaning fee as decimal if itemised separately (same currency as total)",
  "service_fee": "string|null — Airbnb service fee as decimal if itemised separately"
}"#;

pub struct AirbnbBookings;

impl VendorExtractor for AirbnbBookings {
    fn name(&self) -> &'static str {
        "airbnb_bookings"
    }

    fn required_fields(&self) -> &'static [&'static str] {
        // Subjects with date ranges are the most reliable signal; everything
        // else is supplementary.
        &["checkin", "checkout"]
    }

    fn llm_schema(&self) -> Option<&'static str> {
        Some(AIRBNB_LLM_SCHEMA)
    }

    fn validate_field(&self, field: &str, value: &Value) -> bool {
        match (field, value) {
            ("booking_ref", Value::String(s)) => {
                static R: OnceLock<Regex> = OnceLock::new();
                let re = R.get_or_init(|| Regex::new(r"^HM[A-Z0-9]{6,10}$").unwrap());
                re.is_match(s)
            }
            ("total", Value::String(s)) => {
                s.parse::<f64>().map(|f| (0.0..=100_000.0).contains(&f)).unwrap_or(false)
            }
            ("host" | "property", Value::String(s)) => {
                !s.is_empty() && s.len() < 80 && s.chars().any(|c| c.is_alphabetic())
            }
            ("checkin" | "checkout", Value::String(s)) => {
                !s.is_empty() && s.chars().any(|c| c.is_ascii_digit())
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

        // Try subject for compact "DD–DD Mmm" (check-in / check-out).
        if let Some((ci, co)) = parse_subject_date_range(&subject) {
            out.insert("checkin".into(), Value::String(ci));
            out.insert("checkout".into(), Value::String(co));
        }

        // Property / listing name from subject "Reservation for <NAME>,"
        if let Some(prop) = property_from_subject(&subject) {
            out.insert("property".into(), Value::String(prop));
        }

        if !html.is_empty() {
            // Extract property URL from raw HTML (before strip) since
            // the URL is in href attributes, not text content.
            if let Some(url) = first_capture(&airbnb_property_url_re(), html) {
                out.insert("property_url".into(), Value::String(url));
            }
            let text = strip_to_text(html);
            if let Some(host) = first_capture(&host_re(), &text) {
                out.insert("host".into(), Value::String(host));
            }
            if let Some(refn) = first_capture(&booking_ref_re(), &text) {
                out.insert("booking_ref".into(), Value::String(refn));
            }
            // Currency-aware money extraction (replaces the GBP-only
            // total_re path). Falls back to free-text find_money when
            // no Total/Total cost label is found.
            if let Some((c, v)) = currency::find_money(&text) {
                out.insert("total".into(), Value::String(v));
                out.insert("currency".into(), Value::String(c));
            }
            if let Some(g) = first_capture(&guests_re(), &text) {
                out.insert("guests".into(), Value::String(g));
            }
            // Body has the canonical compound check-in/checkout lines:
            //   "Check-in Friday 24 April 2026 16:00"
            //   "Checkout Monday 27 April 2026 10:00"
            // Extract date and time separately so the CLI can render
            // both. If subject already populated checkin (the compact
            // "DD–DD Mmm" form), prefer the body's richer form when
            // available.
            if let Some(caps) = checkin_compound_re().captures(&text) {
                if let Some(date) = caps.get(1).map(|m| m.as_str().trim().to_string()) {
                    out.insert("checkin".into(), Value::String(date));
                }
                if let Some(time) = caps.get(2).map(|m| m.as_str().to_string()) {
                    out.insert("checkin_time".into(), Value::String(time));
                }
            }
            if let Some(caps) = checkout_compound_re().captures(&text) {
                if let Some(date) = caps.get(1).map(|m| m.as_str().trim().to_string()) {
                    out.insert("checkout".into(), Value::String(date));
                }
                if let Some(time) = caps.get(2).map(|m| m.as_str().to_string()) {
                    out.insert("checkout_time".into(), Value::String(time));
                }
            }
            // Long-form check-in / check-out lines (legacy fallback).
            if !out.contains_key("checkin") {
                if let Some(d) = first_capture(&checkin_re(), &text) {
                    out.insert("checkin".into(), Value::String(d));
                }
            }
            if !out.contains_key("checkout") {
                if let Some(d) = first_capture(&checkout_re(), &text) {
                    out.insert("checkout".into(), Value::String(d));
                }
            }
        }

        Ok(out)
    }
}

/// "Reservation for X, NN–NN Mmm" → ("NN Mmm", "NN Mmm")
fn parse_subject_date_range(subject: &str) -> Option<(String, String)> {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| {
        // En-dash, em-dash, hyphen all valid separators.
        Regex::new(r"(\d{1,2})\s*[\u{2013}\u{2014}\-]\s*(\d{1,2})\s+([A-Za-z]+)")
            .unwrap()
    });
    let caps = re.captures(subject)?;
    let d1 = caps.get(1)?.as_str();
    let d2 = caps.get(2)?.as_str();
    let mon = caps.get(3)?.as_str();
    Some((format!("{d1} {mon}"), format!("{d2} {mon}")))
}

fn property_from_subject(subject: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| {
        Regex::new(r"(?i)Reservation\s+for\s+([^,]+?)(?:,|$)").unwrap()
    });
    re.captures(subject)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
}

fn airbnb_property_url_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // Capture group 1 = full URL (no query params). Listing URLs look
    // like https://www.airbnb.co.uk/rooms/12345678.
    R.get_or_init(|| {
        Regex::new(r"(https?://(?:www\.)?airbnb\.[a-z\.]+/rooms/\d+)").unwrap()
    })
}

fn host_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)hosted by\s+([A-Z][\w]+)").unwrap())
}

fn booking_ref_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // Airbnb confirmation codes: HMxxxxxxxx (10 alphanumeric).
    R.get_or_init(|| Regex::new(r"\b(HM[A-Z0-9]{8,10})\b").unwrap())
}

fn total_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)(?:Total|Total cost|You paid|Grand total)[:\s]*£\s*(\d+(?:\.\d{2})?)")
            .unwrap()
    })
}

fn checkin_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)Check[\-\s]?in[:\s]+([\w\s,]+?\d{4})").unwrap()
    })
}

fn checkout_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)Check[\-\s]?out[:\s]+([\w\s,]+?\d{4})").unwrap()
    })
}

/// Modern Airbnb body has compound lines:
///   "Check-in Friday 24 April 2026 16:00"
/// Captures (date, time) — date includes day-of-week + day + month + year.
fn checkin_compound_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)Check[\-\s]?in\s+((?:Mon|Tue|Wed|Thu|Fri|Sat|Sun)[a-z]*\s+\d{1,2}\s+\w+\s+\d{4})\s+(\d{2}:\d{2})",
        )
        .unwrap()
    })
}

fn checkout_compound_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)Check[\-\s]?out\s+((?:Mon|Tue|Wed|Thu|Fri|Sat|Sun)[a-z]*\s+\d{1,2}\s+\w+\s+\d{4})\s+(\d{2}:\d{2})",
        )
        .unwrap()
    })
}

fn guests_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // "Guests 5 adults", "Guests 2 adults, 1 child", "Guests 4 adults, 2 children"
    R.get_or_init(|| {
        Regex::new(r"Guests\s+(\d+\s+[\w,\s]+?)(?:\s+Get\s+the\s+app|\s+Airbnb|$|\s{2,})").unwrap()
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
    fn checkin_checkout_from_subject_dash() {
        let raw =
            "Subject: Reservation for Brook Cottage, 3 bed holiday home in Carbis Bay, 24–27 Apr\nFrom: a\n\n";
        let parsed = parse_test(raw);
        let r = AirbnbBookings.extract(&parsed, "").unwrap();
        assert_eq!(r.get("checkin").and_then(|v| v.as_str()), Some("24 Apr"));
        assert_eq!(r.get("checkout").and_then(|v| v.as_str()), Some("27 Apr"));
        assert_eq!(
            r.get("property").and_then(|v| v.as_str()),
            Some("Brook Cottage")
        );
    }

    #[test]
    fn host_from_html() {
        let html = r#"<p>Welcome! Hosted by Suzy</p>"#;
        let raw = "Subject: x\n\n";
        let parsed = parse_test(raw);
        let r = AirbnbBookings.extract(&parsed, html).unwrap();
        assert_eq!(r.get("host").and_then(|v| v.as_str()), Some("Suzy"));
    }
}
