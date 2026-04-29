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

use super::VendorExtractor;

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
            let text = strip_to_text(html);
            if let Some(host) = first_capture(&host_re(), &text) {
                out.insert("host".into(), Value::String(host));
            }
            if let Some(refn) = first_capture(&booking_ref_re(), &text) {
                out.insert("booking_ref".into(), Value::String(refn));
            }
            if let Some(total) = first_capture(&total_re(), &text) {
                out.insert("total".into(), Value::String(total));
            }
            // Long-form check-in / check-out lines (if subject didn't yield).
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
