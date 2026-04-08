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

/// Update an existing field's value in lines of a markdown file.
///
/// Replaces the first line matching `**field_name**:` with the new value.
/// Returns the modified lines.
pub fn update_field(lines: &[String], field_name: &str, new_value: &str) -> Vec<String> {
    let marker = format!("**{}**:", field_name);
    lines
        .iter()
        .map(|l| {
            if l.contains(&marker) {
                format!("**{}**: {}", field_name, new_value)
            } else {
                l.clone()
            }
        })
        .collect()
}

/// Known reference fields in order of appearance in client .md files.
const REFERENCE_FIELDS: &[&str] = &[
    "Therapy commenced",
    "Formal notes",
    "Referral source",
    "Referral type",
    "Referring doctor",
    "Funding",
    "Session count",
    "Last update letter",
];

/// Insert a new field after the last known reference field line.
///
/// If no reference fields are found, inserts after line 1 (after the `# ID` heading).
/// Returns the modified lines.
pub fn insert_field_after_last(lines: &[String], field_name: &str, value: &str) -> Vec<String> {
    let re = Regex::new(
        r"^\*\*(Therapy commenced|Formal notes|Referral source|Referral type|Referring doctor|Funding|Session count|Last update letter)\*\*:",
    )
    .unwrap();

    // Find the last reference field line
    let mut last_ref_idx = None;
    for (i, line) in lines.iter().enumerate() {
        if re.is_match(line) {
            last_ref_idx = Some(i);
        }
    }

    let insert_after = last_ref_idx.unwrap_or(0);
    let new_line = format!("**{}**: {}", field_name, value);

    let mut result = Vec::with_capacity(lines.len() + 1);
    result.extend_from_slice(&lines[..=insert_after]);
    result.push(new_line);
    result.extend_from_slice(&lines[(insert_after + 1)..]);
    result
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

    #[test]
    fn test_update_field() {
        let lines: Vec<String> = vec![
            "# EB88".into(),
            "**Session count**: 5".into(),
            "**Funding**: AXA".into(),
        ];
        let result = update_field(&lines, "Session count", "8");
        assert_eq!(result[1], "**Session count**: 8");
        assert_eq!(result[2], "**Funding**: AXA"); // unchanged
    }

    #[test]
    fn test_insert_field_after_last() {
        let lines: Vec<String> = vec![
            "# EB88".into(),
            "".into(),
            "**Therapy commenced**: July 2023".into(),
            "**Funding**: AXA".into(),
            "".into(),
            "## Presenting Difficulties".into(),
        ];
        let result = insert_field_after_last(&lines, "Session count", "10");
        assert_eq!(result.len(), 7);
        assert_eq!(result[3], "**Funding**: AXA");
        assert_eq!(result[4], "**Session count**: 10");
        assert_eq!(result[5], "");
    }

    #[test]
    fn test_insert_field_no_reference_fields() {
        let lines: Vec<String> = vec!["# TEST01".into(), "".into(), "Some content".into()];
        let result = insert_field_after_last(&lines, "Session count", "5");
        assert_eq!(result.len(), 4);
        assert_eq!(result[1], "**Session count**: 5");
    }
}
