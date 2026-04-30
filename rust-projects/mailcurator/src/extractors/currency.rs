//! Shared helpers for currency-aware monetary extraction.
//!
//! Used by airbnb_bookings, amazon_orders, booking_com_bookings,
//! generic_hotel, trainline_journeys, and tesla. Each extractor
//! emits two paired fields when a price is found:
//!   - `total` (or `fare`, `amount`) — the numeric value, decimal string
//!   - `currency` — ISO 4217 code: GBP, EUR, USD, JPY, CHF, CAD, AUD, etc.
//!
//! Defaulting to GBP when no currency marker is present matches the
//! historical behaviour of the prior single-currency regex; explicit
//! detection of £, €, $, ¥, and three-letter codes lets foreign
//! bookings extract correctly from the moment they arrive.

use regex::Regex;
use std::sync::OnceLock;

/// Map a currency symbol or code to its ISO 4217 code. Returns the
/// uppercase code if we recognise it, or None.
pub fn symbol_to_code(s: &str) -> Option<&'static str> {
    match s.trim() {
        "£" => Some("GBP"),
        "€" => Some("EUR"),
        // Ambiguous — could be CAD/AUD/SGD; assume USD as the default.
        // Refine via vendor-specific context (e.g. Airbnb sets identity
        // by host's locale) when it matters.
        "$" => Some("USD"),
        "¥" => Some("JPY"),
        "GBP" => Some("GBP"),
        "EUR" => Some("EUR"),
        "USD" => Some("USD"),
        "JPY" => Some("JPY"),
        "CHF" => Some("CHF"),
        "CAD" => Some("CAD"),
        "AUD" => Some("AUD"),
        "NZD" => Some("NZD"),
        "SEK" => Some("SEK"),
        "NOK" => Some("NOK"),
        "DKK" => Some("DKK"),
        "SGD" => Some("SGD"),
        "HKD" => Some("HKD"),
        "ZAR" => Some("ZAR"),
        "AED" => Some("AED"),
        "INR" => Some("INR"),
        // Ambiguous Nordic — assume SEK; refined by context if needed.
        "kr" | "Kr" => Some("SEK"),
        _ => None,
    }
}

/// Find the first money-shaped token in text and return (currency_code,
/// amount_string). Captures common forms:
///   £319.80, € 1,234.50, USD 99.00, 99.00 EUR, $50, ¥1500
/// Decimal separator: . is canonical; , is normalised to . for amount
/// when followed by exactly 2 digits; , is treated as thousands-sep
/// otherwise.
pub fn find_money(text: &str) -> Option<(String, String)> {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| {
        // Permissive value pattern: [\d.,]+ accepts both UK/US thousand-
        // separators ("1,234.50") and EU thousand-separators
        // ("1.234,50"). normalise_amount() disambiguates after capture.
        // Anchored on a leading word-boundary on the value side so we
        // don't pick up partial digit substrings.
        Regex::new(
            r"(?ix)
            (?:
                # Symbol-prefix: £319.80, € 1,234.50, $50, ¥1500
                (?P<sym>[£€$¥])\s*(?P<v1>\d[\d.,]*)
              |
                # Code-prefix: USD 99.00, EUR 1,234.50
                \b(?P<code1>GBP|EUR|USD|JPY|CHF|CAD|AUD|NZD|SEK|NOK|DKK|SGD|HKD|ZAR|AED|INR)\s+(?P<v2>\d[\d.,]*)
              |
                # Code-suffix: 99.00 EUR, 1234,50 EUR
                \b(?P<v3>\d[\d.,]*)\s+(?P<code2>GBP|EUR|USD|JPY|CHF|CAD|AUD|NZD|SEK|NOK|DKK|SGD|HKD|ZAR|AED|INR)\b
            )",
        )
        .unwrap()
    });
    let caps = re.captures(text)?;
    let (currency, raw_value) = if let Some(s) = caps.name("sym") {
        (symbol_to_code(s.as_str())?, caps.name("v1")?.as_str())
    } else if let Some(c) = caps.name("code1") {
        (symbol_to_code(c.as_str())?, caps.name("v2")?.as_str())
    } else if let Some(c) = caps.name("code2") {
        (symbol_to_code(c.as_str())?, caps.name("v3")?.as_str())
    } else {
        return None;
    };
    Some((currency.to_string(), normalise_amount(raw_value)))
}

/// Normalise an amount string. UK/US use `,` for thousands and `.` for
/// decimal; many EU locales use `.` for thousands and `,` for decimal.
/// Heuristic: if the string ends in `,XX` (exactly 2 digits) treat the
/// comma as decimal separator; otherwise treat all commas as thousands
/// separators (drop them).
pub fn normalise_amount(s: &str) -> String {
    let s = s.trim();
    static EU_DECIMAL_RE: OnceLock<Regex> = OnceLock::new();
    let eu = EU_DECIMAL_RE.get_or_init(|| Regex::new(r"^[\d.]+,\d{1,2}$").unwrap());
    if eu.is_match(s) {
        // EU form: "1.234,50" → strip dots, swap comma→dot.
        return s.replace('.', "").replace(',', ".");
    }
    // US/UK form (or no thousands sep): just drop commas.
    s.replace(',', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pound_sterling() {
        let (c, v) = find_money("Total price £319.80 paid").unwrap();
        assert_eq!(c, "GBP");
        assert_eq!(v, "319.80");
    }

    #[test]
    fn euro_with_thousands() {
        let (c, v) = find_money("Total: € 1,234.50").unwrap();
        assert_eq!(c, "EUR");
        assert_eq!(v, "1234.50");
    }

    #[test]
    fn euro_eu_locale() {
        let (c, v) = find_money("Totale: € 1.234,50").unwrap();
        assert_eq!(c, "EUR");
        assert_eq!(v, "1234.50");
    }

    #[test]
    fn usd_code_prefix() {
        let (c, v) = find_money("Charged USD 99.00 today").unwrap();
        assert_eq!(c, "USD");
        assert_eq!(v, "99.00");
    }

    #[test]
    fn eur_code_suffix() {
        let (c, v) = find_money("Total 49.50 EUR").unwrap();
        assert_eq!(c, "EUR");
        assert_eq!(v, "49.50");
    }

    #[test]
    fn no_match_returns_none() {
        assert!(find_money("no money here").is_none());
        assert!(find_money("just 5 nights").is_none());
    }

    #[test]
    fn yen_no_decimal() {
        let (c, v) = find_money("Cost ¥1500").unwrap();
        assert_eq!(c, "JPY");
        assert_eq!(v, "1500");
    }
}
