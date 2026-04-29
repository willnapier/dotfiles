//! Amazon.co.uk order placement extractor.
//!
//! Targets messages matched by the `amazon-orders` policy in policies.toml
//! (subject: "Amazon.co.uk order of", "Ordered:", or "Thank you for your
//! order"). Uses CSS-selector-based parsing of the order summary table for
//! reliability across template tweaks.
//!
//! Extracted fields:
//! - `order_id`   — the canonical 202-XXXXXXX-XXXXXXX (or 203-, 205-) form
//! - `total`      — Grand Total in pounds, anchored on the `<b>Grand Total:</b>`
//!                 cell (NOT subtotal/tax — important: the prior generic regex
//!                 was unreliably capturing subtotals because it fired on the
//!                 first "Total" cell in document order)
//! - `subtotal`   — Item Subtotal (pre-tax/shipping)
//! - `items`      — list of item titles from the order summary
//! - `eta`        — arrival/delivery date if present in subject or body

use anyhow::Result;
use mailparse::{MailHeaderMap, ParsedMail};
use regex::Regex;
use scraper::{Html, Selector};
use serde_json::{Map, Value};
use std::sync::OnceLock;

use super::VendorExtractor;

/// JSON-schema-shaped string the LLM is asked to fill. Field names match
/// the deterministic extractor so cached LLM output merges cleanly.
const AMAZON_LLM_SCHEMA: &str = r#"{
  "order_id": "string|null — canonical Amazon order ID, either 3-digit form like 202-1234567-1234567 (physical) or D-prefix form like D01-1234567-1234567 (digital). Null if not present.",
  "total": "string|null — Grand Total / Order Total in pounds as a decimal, e.g. \"3.49\". Null if not present. Do NOT use subtotal or pre-tax figures.",
  "subtotal": "string|null — Item Subtotal in pounds as decimal. Null if not present.",
  "items": ["string"] ,
  "eta": "string|null — arrival/delivery date if mentioned (e.g. \"Tuesday\", \"25 March\"). Null if not present."
}"#;

pub struct AmazonOrders;

impl VendorExtractor for AmazonOrders {
    fn name(&self) -> &'static str {
        "amazon_orders"
    }

    fn required_fields(&self) -> &'static [&'static str] {
        // Items deliberately omitted — pre-2014 emails don't include an
        // order-summary block of the modern shape, and we'd rather show 95%
        // coverage truthfully than 60% with items dragging the average.
        // Revisit when items extraction is more robust.
        &["order_id", "total"]
    }

    fn llm_schema(&self) -> Option<&'static str> {
        Some(AMAZON_LLM_SCHEMA)
    }

    fn validate_field(&self, field: &str, value: &Value) -> bool {
        match (field, value) {
            // Reject hallucinated order IDs that don't match the
            // canonical 3-digit-or-D-prefix form. This is the most
            // common LLM hallucination — it'll happily produce
            // "ORDER-12345" or similar plausible-looking strings.
            ("order_id", Value::String(s)) => {
                static R: OnceLock<Regex> = OnceLock::new();
                let re = R.get_or_init(|| {
                    Regex::new(r"^(?:20[2-5]|D\d{2})-\d{7}-\d{7}$").unwrap()
                });
                re.is_match(s)
            }
            // Total / subtotal must be parseable as a decimal pound
            // amount — reject plain text or impossible values.
            ("total" | "subtotal", Value::String(s)) => {
                s.parse::<f64>().map(|f| f >= 0.0 && f < 100_000.0).unwrap_or(false)
            }
            // Items: array of non-empty strings.
            ("items", Value::Array(a)) => {
                a.iter().any(|v| matches!(v, Value::String(s) if !s.is_empty()))
            }
            // ETA: any non-empty string. Hallucination risk lower here.
            ("eta", Value::String(s)) => !s.is_empty(),
            // Unknown field: fall through to the trait default.
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

        // Order ID: 202-XXXXXXX-XXXXXXX form. Look in subject first (newer
        // dispatch/delivery subjects include it explicitly), then HTML
        // (where it appears in "Order #" footers and in URL parameters).
        if let Some(id) = order_id_re().captures(&subject).and_then(|c| c.get(1)) {
            out.insert("order_id".into(), Value::String(id.as_str().to_string()));
        } else if let Some(id) = order_id_re().captures(html).and_then(|c| c.get(1)) {
            out.insert("order_id".into(), Value::String(id.as_str().to_string()));
        }

        // Parse HTML once for DOM-based extraction.
        if !html.is_empty() {
            let doc = Html::parse_document(html);

            // Grand Total: anchor on bolded "Grand Total:" header cell.
            // Amazon wraps both the label and the amount in <b> tags for
            // grand-total rows, distinguishing them from subtotal rows.
            if let Some(total) = extract_table_amount(&doc, &["Grand Total"]) {
                out.insert("total".into(), Value::String(total));
            }
            if let Some(sub) = extract_table_amount(&doc, &["Item Subtotal", "Subtotal"]) {
                out.insert("subtotal".into(), Value::String(sub));
            }

            // Items: <h2 / a> within order-summary blocks. Best-effort —
            // many older emails don't have a structured summary.
            let items = extract_items(&doc);
            if !items.is_empty() {
                out.insert("items".into(), Value::Array(items.into_iter().map(Value::String).collect()));
            }
        }

        // ETA: subject-level "arriving <day>" pattern (Amazon's modern
        // dispatch emails put this in the subject).
        if let Some(eta) = eta_re().captures(&subject).and_then(|c| c.get(1)) {
            out.insert("eta".into(), Value::String(eta.as_str().trim().to_string()));
        }

        Ok(out)
    }
}

// ── helpers ─────────────────────────────────────────────────────────────

fn order_id_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // Two Amazon order-ID forms:
    //   - 20{2..5}-XXXXXXX-XXXXXXX  physical orders (most common)
    //   - D{nn}-XXXXXXX-XXXXXXX     digital orders (Kindle, MP3, video)
    // The digital form discovered 2026-04-29: modern "Amazon.co.uk order
    // of <book>" emails for Kindle purchases use D01-style IDs, embedded
    // only in URLs (orderID query parameter), not in the subject line.
    R.get_or_init(|| Regex::new(r"\b((?:20[2-5]|D\d{2})-\d{7}-\d{7})\b").unwrap())
}

fn eta_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // 1–3 words of letters after "arriving". Bounded length stops the lazy
    // match from greedy-eating the whole subject; alphabetic-only avoids
    // running into "Tuesday: 2 items" style separators or numeric dates.
    R.get_or_init(|| {
        Regex::new(r"(?i)arriving\s+([A-Za-z][a-zA-Z]*(?:\s+[A-Za-z][a-zA-Z]*){0,2})").unwrap()
    })
}

/// Find a `<th>` cell whose text matches one of the labels, return the
/// adjacent `<td>` cell's text. Strip surrounding £/$/€ and whitespace
/// from the value so the field is a clean numeric string ("3.49").
fn extract_table_amount(doc: &Html, labels: &[&str]) -> Option<String> {
    let row_sel = Selector::parse("tr").ok()?;
    for row in doc.select(&row_sel) {
        // Within each row, look for th + td pair.
        let th_text = row
            .select(&Selector::parse("th").ok()?)
            .next()
            .map(|n| n.text().collect::<String>())
            .unwrap_or_default();
        let th_clean = th_text.trim().trim_end_matches(':').trim();
        if !labels.iter().any(|l| th_clean.eq_ignore_ascii_case(l)) {
            continue;
        }
        let td_text = row
            .select(&Selector::parse("td").ok()?)
            .next()
            .map(|n| n.text().collect::<String>())
            .unwrap_or_default();
        let amount = td_text.trim().trim_start_matches(['£', '$', '€']).trim();
        if !amount.is_empty() {
            return Some(amount.to_string());
        }
    }
    None
}

/// Pull item titles from order summary block. Amazon wraps each item title
/// in an <a> link inside the summary. This is heuristic — pre-2014 emails
/// have a different structure and won't match.
fn extract_items(doc: &Html) -> Vec<String> {
    let mut items = Vec::new();
    // Heuristic: look for <a> tags that contain "amazon.co.uk/dp/" or
    // "amazon.co.uk/gp/product/" in href, with non-empty text content.
    let Ok(sel) = Selector::parse("a") else {
        return items;
    };
    for a in doc.select(&sel) {
        let href = a.value().attr("href").unwrap_or_default();
        if !(href.contains("/dp/") || href.contains("/gp/product/")) {
            continue;
        }
        let text = a.text().collect::<String>().trim().to_string();
        if text.is_empty() || text.len() > 200 {
            continue;
        }
        // Avoid dupes (Amazon emails sometimes wrap the same item in
        // multiple links — image link + title link).
        if !items.contains(&text) {
            items.push(text);
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_test(raw: &str) -> ParsedMail<'_> {
        mailparse::parse_mail(raw.as_bytes()).unwrap()
    }

    #[test]
    fn order_id_from_subject() {
        let raw = "Subject: Delivered: 1 item | Order # 202-2900583-9701921\n\
                   From: auto-confirm@amazon.co.uk\n\n";
        let parsed = parse_test(raw);
        let result = AmazonOrders.extract(&parsed, "").unwrap();
        assert_eq!(
            result.get("order_id").and_then(|v| v.as_str()),
            Some("202-2900583-9701921")
        );
    }

    #[test]
    fn grand_total_anchored_correctly() {
        // Mimic Amazon's order summary structure: subtotal first, grand total last.
        // Naive "first Total" regex would grab subtotal; this extractor must grab grand total.
        let html = r#"<table>
            <tr><th>Item Subtotal:</th><td>£2.91</td></tr>
            <tr><th>Total Before Tax:</th><td>£2.91</td></tr>
            <tr><th>Tax Collected:</th><td>£0.58</td></tr>
            <tr><th><b>Grand Total:</b></th><td><b>£3.49</b></td></tr>
        </table>"#;
        let parsed = parse_test("Subject: x\n\n");
        let result = AmazonOrders.extract(&parsed, html).unwrap();
        assert_eq!(result.get("total").and_then(|v| v.as_str()), Some("3.49"));
        assert_eq!(result.get("subtotal").and_then(|v| v.as_str()), Some("2.91"));
    }

    #[test]
    fn missing_html_returns_partial_record() {
        // No HTML body — should still extract order_id from subject.
        let raw = "Subject: Delivered: Order # 202-1111111-2222222\nFrom: a@amazon.co.uk\n\n";
        let parsed = parse_test(raw);
        let result = AmazonOrders.extract(&parsed, "").unwrap();
        assert!(result.contains_key("order_id"));
        assert!(!result.contains_key("total"));
    }

    #[test]
    fn eta_from_arriving_subject() {
        let raw = "Subject: Arriving Tuesday: 2 items\nFrom: a@amazon.co.uk\n\n";
        let parsed = parse_test(raw);
        let result = AmazonOrders.extract(&parsed, "").unwrap();
        assert_eq!(result.get("eta").and_then(|v| v.as_str()), Some("Tuesday"));
    }
}
