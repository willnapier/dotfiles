use rust_decimal::Decimal;
use std::str::FromStr;

/// Parse amount from First Direct format: "-£9.00" or "+£8478.33"
pub fn parse_amount(s: &str) -> Result<Decimal, String> {
    let cleaned = s
        .trim()
        .replace('£', "")
        .replace(',', "");

    Decimal::from_str(&cleaned)
        .map_err(|e| format!("failed to parse amount '{}': {}", s, e))
}

/// Clean up merchant description
/// Removes excess whitespace, normalizes location info
pub fn clean_description(raw: &str) -> String {
    // Collapse multiple spaces
    let parts: Vec<&str> = raw.split_whitespace().collect();
    let cleaned = parts.join(" ");

    // Remove common noise patterns
    let cleaned = cleaned
        .replace("INT'L **********", "")
        .trim()
        .to_string();

    // Remove trailing location codes that aren't useful
    // e.g., "TESCO STORES 1234 LONDON" -> "TESCO STORES LONDON"
    // For now, keep it simple - just clean whitespace

    cleaned
}

/// Extract the core merchant name for matching
/// More aggressive cleaning for rule matching
pub fn extract_merchant(raw: &str) -> String {
    let cleaned = clean_description(raw);

    // Take first meaningful part before location info
    // This is heuristic and may need tuning
    cleaned
        .to_uppercase()
        .replace("*", "")
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_amount() {
        assert_eq!(parse_amount("-£9.00").unwrap(), Decimal::from_str("-9.00").unwrap());
        assert_eq!(parse_amount("+£8478.33").unwrap(), Decimal::from_str("8478.33").unwrap());
        assert_eq!(parse_amount("-£1,234.56").unwrap(), Decimal::from_str("-1234.56").unwrap());
    }

    #[test]
    fn test_clean_description() {
        assert_eq!(
            clean_description("PRET A MANGER     London"),
            "PRET A MANGER London"
        );
        assert_eq!(
            clean_description("INT'L **********  CLAUDE.AI SUBSCRIPSAN FRANCISCO"),
            "CLAUDE.AI SUBSCRIPSAN FRANCISCO"
        );
    }
}
