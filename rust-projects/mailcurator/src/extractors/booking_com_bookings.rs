//! Booking.com hotel reservation extractor.
//!
//! Booking.com confirmation emails have well-structured data:
//!   "Check-in Saturday, 4 July 2026 (15:00 - 21:00)"
//!   "Check-out Sunday, 5 July 2026 (08:00 - 10:00)"
//!   "Total price £319.80"
//!   "1 night , 2 rooms"
//!   "4 adults"
//!   "Confirmation: 5022794464"
//!   "PIN: 2554 (Confidential)"
//!
//! Property name comes from the subject ("Your booking is confirmed at
//! <hotel>") or "<hotel> is expecting you" patterns in the body.
//! Output goes to the shared bookings.jsonl alongside Airbnb so
//! `mailcurator bookings list` shows everything mixed together.

use anyhow::Result;
use mailparse::{MailHeaderMap, ParsedMail};
use regex::Regex;
use scraper::Html;
use serde_json::{Map, Value};
use std::sync::OnceLock;

use super::VendorExtractor;

const BOOKING_COM_LLM_SCHEMA: &str = r#"{
  "checkin": "string|null — check-in date as written, e.g. \"Saturday, 4 July 2026\"",
  "checkin_time": "string|null — check-in window start time HH:MM, e.g. \"15:00\"",
  "checkout": "string|null — check-out date in the same format",
  "checkout_time": "string|null — check-out time HH:MM",
  "guests": "string|null — guest count, e.g. \"4 adults\", \"2 adults, 1 child\"",
  "nights": "string|null — number of nights as integer, e.g. \"1\"",
  "rooms": "string|null — number of rooms as integer, e.g. \"2\"",
  "property": "string|null — hotel/inn/B&B name (e.g. \"The Mounts Bay Inn\")",
  "location": "string|null — city/town/region (e.g. \"Mullion, United Kingdom\")",
  "booking_ref": "string|null — Booking.com confirmation number, typically 10 digits",
  "pin": "string|null — 4-digit PIN if shown",
  "total": "string|null — total in pounds as decimal, e.g. \"319.80\"",
  "property_url": "string|null — direct link to the hotel page on Booking.com, e.g. \"https://www.booking.com/hotel/gb/the-mounts-bay-inn.html\". Strip query parameters."
}"#;

pub struct BookingComBookings;

impl VendorExtractor for BookingComBookings {
    fn name(&self) -> &'static str {
        "booking_com_bookings"
    }

    fn required_fields(&self) -> &'static [&'static str] {
        &["property", "checkin", "checkout"]
    }

    fn llm_schema(&self) -> Option<&'static str> {
        Some(BOOKING_COM_LLM_SCHEMA)
    }

    fn validate_field(&self, field: &str, value: &Value) -> bool {
        match (field, value) {
            ("booking_ref", Value::String(s)) => {
                static R: OnceLock<Regex> = OnceLock::new();
                let re = R.get_or_init(|| Regex::new(r"^\d{8,12}$").unwrap());
                re.is_match(s)
            }
            ("pin", Value::String(s)) => {
                static R: OnceLock<Regex> = OnceLock::new();
                let re = R.get_or_init(|| Regex::new(r"^\d{4}$").unwrap());
                re.is_match(s)
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
        out.insert("vendor".into(), Value::String("Booking.com".into()));

        // Property from subject: "...is confirmed at <Property>" or
        // "...about your booking at <Property>" etc.
        if let Some(p) = property_from_subject(&subject) {
            out.insert("property".into(), Value::String(p));
        }

        if !html.is_empty() {
            // Property URL from raw HTML (href attribute, not text).
            if let Some(url) = first_capture(&booking_com_property_url_re(), html) {
                out.insert("property_url".into(), Value::String(url));
            }
            let text = strip_to_text(html);

            // Check-in / Check-out compound pattern with time window:
            //   "Check-in Saturday, 4 July 2026 (15:00 - 21:00)"
            if let Some(caps) = checkin_compound_re().captures(&text) {
                if let Some(d) = caps.get(1).map(|m| m.as_str().trim().to_string()) {
                    out.insert("checkin".into(), Value::String(d));
                }
                if let Some(t) = caps.get(2).map(|m| m.as_str().to_string()) {
                    out.insert("checkin_time".into(), Value::String(t));
                }
            }
            if let Some(caps) = checkout_compound_re().captures(&text) {
                if let Some(d) = caps.get(1).map(|m| m.as_str().trim().to_string()) {
                    out.insert("checkout".into(), Value::String(d));
                }
                if let Some(t) = caps.get(2).map(|m| m.as_str().to_string()) {
                    out.insert("checkout_time".into(), Value::String(t));
                }
            }

            if let Some(g) = first_capture(&guests_re(), &text) {
                out.insert("guests".into(), Value::String(g));
            }
            if let Some(n) = first_capture(&nights_re(), &text) {
                out.insert("nights".into(), Value::String(n));
            }
            if let Some(r) = first_capture(&rooms_re(), &text) {
                out.insert("rooms".into(), Value::String(r));
            }
            if let Some(t) = first_capture(&total_re(), &text) {
                out.insert("total".into(), Value::String(t));
            }
            if let Some(c) = first_capture(&confirmation_re(), &text) {
                out.insert("booking_ref".into(), Value::String(c));
            }
            if let Some(p) = first_capture(&pin_re(), &text) {
                out.insert("pin".into(), Value::String(p));
            }

            // Property from body if subject didn't yield ("<Property> is expecting you").
            if !out.contains_key("property") {
                if let Some(p) = first_capture(&property_body_re(), &text) {
                    out.insert("property".into(), Value::String(p));
                }
            }
        }

        Ok(out)
    }
}

fn property_from_subject(subject: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| {
        // "Your booking is confirmed at X", "We have a question about your booking at X",
        // "Booking cancelled for X", "X is waiting for your review", "Rate X"
        Regex::new(
            r"(?:confirmed at|booking at|cancelled for|Rate)\s+(.+?)(?:\s*$)",
        )
        .unwrap()
    });
    re.captures(subject)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
}

fn checkin_compound_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        // "Check-in Saturday, 4 July 2026 (15:00 - 21:00)"
        Regex::new(
            r"(?i)Check[\-\s]?in\s+((?:Mon|Tue|Wed|Thu|Fri|Sat|Sun)[a-z]*,\s+\d{1,2}\s+\w+\s+\d{4})\s*\((\d{2}:\d{2})",
        )
        .unwrap()
    })
}

fn checkout_compound_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)Check[\-\s]?out\s+((?:Mon|Tue|Wed|Thu|Fri|Sat|Sun)[a-z]*,\s+\d{1,2}\s+\w+\s+\d{4})\s*\((\d{2}:\d{2})",
        )
        .unwrap()
    })
}

fn guests_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?:You booked for|booked for)\s+(\d+\s+adults?(?:[,\s]+\d+\s+\w+)*)")
            .unwrap()
    })
}

fn nights_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(\d+)\s+nights?\b").unwrap())
}

fn rooms_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(\d+)\s+rooms?\b").unwrap())
}

fn total_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"Total\s+price\s+£\s*(\d+(?:[,\d]+)?(?:\.\d{2})?)")
            .unwrap()
    })
}

fn confirmation_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"Confirmation:\s*(\d{8,12})").unwrap())
}

fn pin_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"PIN:\s*(\d{4})").unwrap())
}

fn booking_com_property_url_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // Hotel page URL: https://www.booking.com/hotel/gb/the-mounts-bay-inn.html
    R.get_or_init(|| {
        Regex::new(r"(https?://(?:www\.|secure\.)?booking\.com/hotel/[a-z]+/[a-z0-9\-]+\.[a-z\-]+\.html)").unwrap()
    })
}

fn property_body_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"([\w][\w\s']{3,80}?)\s+is\s+expecting\s+you").unwrap())
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
    fn property_from_subject_confirmed() {
        let raw = "Subject: 🛄 Thanks! Your booking is confirmed at The Mounts Bay Inn\nFrom: x@booking.com\n\n";
        let parsed = parse_test(raw);
        let r = BookingComBookings.extract(&parsed, "").unwrap();
        assert_eq!(
            r.get("property").and_then(|v| v.as_str()),
            Some("The Mounts Bay Inn")
        );
    }

    #[test]
    fn checkin_with_time_window() {
        let html = r#"<p>Reservation details</p><p>Check-in Saturday, 4 July 2026 (15:00 - 21:00)</p>"#;
        let raw = "Subject: x\n\n";
        let parsed = parse_test(raw);
        let r = BookingComBookings.extract(&parsed, html).unwrap();
        assert_eq!(
            r.get("checkin").and_then(|v| v.as_str()),
            Some("Saturday, 4 July 2026")
        );
        assert_eq!(r.get("checkin_time").and_then(|v| v.as_str()), Some("15:00"));
    }

    #[test]
    fn confirmation_number_and_pin() {
        let html = r#"<p>Confirmation: 5022794464 PIN: 2554 (Confidential)</p>"#;
        let raw = "Subject: x\n\n";
        let parsed = parse_test(raw);
        let r = BookingComBookings.extract(&parsed, html).unwrap();
        assert_eq!(
            r.get("booking_ref").and_then(|v| v.as_str()),
            Some("5022794464")
        );
        assert_eq!(r.get("pin").and_then(|v| v.as_str()), Some("2554"));
    }
}
