use regex::Regex;

/// Extract the value of a bold markdown field from content.
///
/// Looks for patterns like `**Field Name**: value` and returns the value.
/// Returns None if the field is not found or has no value.
pub fn extract_field(content: &str, field_name: &str) -> Option<String> {
    let pattern = format!(r"\*\*{}\*\*:[ \t]*(.*)", regex::escape(field_name));
    let re = Regex::new(&pattern).ok()?;

    re.captures(content).and_then(|caps| {
        let value = caps[1].trim().to_string();
        if value.is_empty() {
            None
        } else {
            Some(value)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_field() {
        let content = "# EB88\n\n**Referral**: Dr Smith\n**Started**: 2026-01-15\n";
        assert_eq!(
            extract_field(content, "Referral"),
            Some("Dr Smith".to_string())
        );
        assert_eq!(
            extract_field(content, "Started"),
            Some("2026-01-15".to_string())
        );
        assert_eq!(extract_field(content, "Missing"), None);
    }

    #[test]
    fn test_extract_field_empty_value() {
        let content = "**Referral**: \n**Started**: 2026-01-15\n";
        assert_eq!(extract_field(content, "Referral"), None);
    }
}
