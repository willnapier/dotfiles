//! Trainline journey extractor.
//!
//! Trainline's email templates vary substantially across notification types
//! ("Get ready for X" pre-journey, "Thanks for booking" confirmation,
//! "Alert: disruption" warning, "Delay Repay" refund). Rather than a deep
//! per-template HTML parse, this extractor goes for the most reliable
//! cross-template signals:
//!
//! - `destination` — from subject: "Get ready for X", "trip to X", "to X is".
//! - `journey_time` — first HH:MM in the HTML body (typically the
//!                    departure time of the next leg).
//! - `journey_date` — first "DD Mmm YYYY" pattern in the body.
//! - `fare` — first £-amount in the body, when present.
//!
//! Coverage will be partial — that's OK. The Karpathy `improve-extractor`
//! loop (Session 3) will refine these patterns against real failures.

use anyhow::Result;
use mailparse::{MailHeaderMap, ParsedMail};
use regex::Regex;
use scraper::Html;
use serde_json::{Map, Value};
use std::sync::OnceLock;

use super::VendorExtractor;

/// JSON schema for the LLM fallback. Field names match the deterministic
/// extractor so cached output merges cleanly.
const TRAINLINE_LLM_SCHEMA: &str = r#"{
  "destination": "string|null — destination station name (e.g. \"London Paddington\", \"Taunton\")",
  "origin": "string|null — origin station name",
  "journey_date": "string|null — date of travel as written in the email, e.g. \"25 March 2026\"",
  "journey_time": "string|null — departure time HH:MM, e.g. \"19:04\"",
  "fare": "string|null — total fare in pounds as decimal, e.g. \"42.50\". No currency symbol.",
  "booking_ref": "string|null — Trainline booking reference / order number"
}"#;

pub struct TrainlineJourneys;

impl VendorExtractor for TrainlineJourneys {
    fn name(&self) -> &'static str {
        "trainline_journeys"
    }

    fn required_fields(&self) -> &'static [&'static str] {
        // Destination + time captures "where + when" — the minimum
        // useful journey record. Fare and date are nice-to-have but
        // missing them doesn't render a row useless. Honest health
        // metric: a journey with destination but no time isn't yet
        // "complete", so LLM should fire to fill the gap.
        &["destination", "journey_time"]
    }

    fn llm_schema(&self) -> Option<&'static str> {
        Some(TRAINLINE_LLM_SCHEMA)
    }

    fn validate_field(&self, field: &str, value: &Value) -> bool {
        match (field, value) {
            // Times must be HH:MM.
            ("journey_time", Value::String(s)) => {
                static R: OnceLock<Regex> = OnceLock::new();
                let re = R.get_or_init(|| Regex::new(r"^\d{2}:\d{2}$").unwrap());
                re.is_match(s)
            }
            // Fares must parse as decimal pounds.
            ("fare", Value::String(s)) => {
                s.parse::<f64>().map(|f| (0.0..=10_000.0).contains(&f)).unwrap_or(false)
            }
            // Station names must be non-empty alphabetic-ish strings (no
            // pure numbers or HTML cruft). Reject "0", "<br>", etc.
            ("destination" | "origin", Value::String(s)) => {
                !s.is_empty() && s.chars().any(|c| c.is_alphabetic()) && s.len() < 80
            }
            // Booking ref: short alphanumeric.
            ("booking_ref", Value::String(s)) => {
                static R: OnceLock<Regex> = OnceLock::new();
                let re = R.get_or_init(|| Regex::new(r"^[A-Z0-9-]{4,15}$").unwrap());
                re.is_match(s)
            }
            // Date: any non-empty string with at least one digit (year).
            ("journey_date", Value::String(s)) => {
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

        if let Some(dest) = destination_from_subject(&subject) {
            out.insert("destination".into(), Value::String(dest));
        }

        if !html.is_empty() {
            // Strip HTML to get clean text body for token regex.
            let text = strip_to_text(html);

            if let Some(t) = first_match(&time_re().captures(&text)) {
                out.insert("journey_time".into(), Value::String(t));
            }
            if let Some(d) = first_match(&date_re().captures(&text)) {
                out.insert("journey_date".into(), Value::String(d));
            }
            if let Some(f) = first_match(&fare_re().captures(&text)) {
                out.insert("fare".into(), Value::String(f));
            }
            if let Some(r) = first_match(&booking_ref_re().captures(&text)) {
                out.insert("booking_ref".into(), Value::String(r));
            }
        }

        Ok(out)
    }
}

fn destination_from_subject(subject: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| {
        // "Get ready for Taunton, Will" → "Taunton"
        // "Your trip to London Paddington is due Delay Repay…" → "London Paddington"
        // "Alert: a disruption may affect your journey to London Cannon Street" → "London Cannon Street"
        Regex::new(
            r"(?i)(?:Get ready for|trip to|journey to|booking for|travel to)\s+([A-Z][\w\s]+?)(?:[,\.]|\s+(?:is|may|on|with|will|station|tomorrow|today)|$)",
        )
        .unwrap()
    });
    re.captures(subject)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
}

fn time_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\b(\d{2}:\d{2})\b").unwrap())
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

fn fare_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"£\s*(\d+\.\d{2})").unwrap())
}

fn booking_ref_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // Trainline booking refs are alphanumeric, often 6-10 chars.
    // "booking reference: ABC123", "Order: XY-12345", etc.
    R.get_or_init(|| {
        Regex::new(
            r"(?i)(?:booking\s+(?:ref(?:erence)?|number)|order\s+(?:ref|number)|confirmation)[:\s#]+([A-Z0-9-]{5,12})",
        )
        .unwrap()
    })
}

fn first_match(caps: &Option<regex::Captures<'_>>) -> Option<String> {
    caps.as_ref()
        .and_then(|c| c.get(1).or_else(|| c.get(0)))
        .map(|m| m.as_str().to_string())
}

fn strip_to_text(html: &str) -> String {
    let doc = Html::parse_document(html);
    // scraper's Html doesn't expose plaintext directly; walk root and join
    // text nodes with single spaces.
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
    fn destination_from_get_ready_for() {
        let raw = "Subject: Get ready for Taunton, Will\nFrom: a@comms.trainline.com\n\n";
        let parsed = parse_test(raw);
        let r = TrainlineJourneys.extract(&parsed, "").unwrap();
        assert_eq!(r.get("destination").and_then(|v| v.as_str()), Some("Taunton"));
    }

    #[test]
    fn destination_from_alert() {
        let raw = "Subject: Alert: a disruption may affect your journey to London Cannon Street\nFrom: a@comms.trainline.com\n\n";
        let parsed = parse_test(raw);
        let r = TrainlineJourneys.extract(&parsed, "").unwrap();
        assert_eq!(
            r.get("destination").and_then(|v| v.as_str()),
            Some("London Cannon Street")
        );
    }

    #[test]
    fn time_and_fare_from_html() {
        let html = r#"<html><body><p>Departure: 19:04</p><p>Total: £42.50</p></body></html>"#;
        let raw = "Subject: Get ready for Taunton, Will\nFrom: a\n\n";
        let parsed = parse_test(raw);
        let r = TrainlineJourneys.extract(&parsed, html).unwrap();
        assert_eq!(r.get("journey_time").and_then(|v| v.as_str()), Some("19:04"));
        assert_eq!(r.get("fare").and_then(|v| v.as_str()), Some("42.50"));
    }
}
