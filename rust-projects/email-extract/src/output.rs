use crate::extract::{BodyType, EmailData};
use anyhow::Result;
use regex::Regex;
use serde_json::{json, Value};

/// Produce plain text output for an email.
pub fn to_text(email: &EmailData, metadata_only: bool, full_headers: bool) -> String {
    let mut out = String::new();

    out.push_str(&format!("From:    {}\n", email.from));
    out.push_str(&format!("To:      {}\n", email.to));
    if !email.cc.is_empty() {
        out.push_str(&format!("Cc:      {}\n", email.cc));
    }
    out.push_str(&format!("Date:    {}\n", display_date(email)));
    out.push_str(&format!("Subject: {}\n", email.subject));

    if full_headers {
        out.push('\n');
        out.push_str("--- Full Headers ---\n");
        for (key, value) in &email.all_headers {
            out.push_str(&format!("{}: {}\n", key, value));
        }
    }

    if !email.attachments.is_empty() {
        out.push('\n');
        out.push_str(&format!("Attachments ({}):\n", email.attachments.len()));
        for att in &email.attachments {
            out.push_str(&format!(
                "  - {} ({}, {} bytes)\n",
                att.filename, att.content_type, att.size
            ));
        }
    }

    if !metadata_only {
        out.push('\n');
        match email.body_type {
            BodyType::HtmlConverted => {
                out.push_str("[converted from HTML]\n\n");
            }
            BodyType::Empty => {
                out.push_str("[no body content]\n");
            }
            _ => {}
        }
        out.push_str(&email.body);
    }

    out
}

/// Produce markdown output with YAML frontmatter for an email.
pub fn to_markdown(email: &EmailData, metadata_only: bool, full_headers: bool) -> String {
    let mut out = String::new();

    // YAML frontmatter
    out.push_str("---\n");
    out.push_str(&format!("from: \"{}\"\n", yaml_escape(&email.from)));
    out.push_str(&format!("to: \"{}\"\n", yaml_escape(&email.to)));
    if !email.cc.is_empty() {
        out.push_str(&format!("cc: \"{}\"\n", yaml_escape(&email.cc)));
    }
    out.push_str(&format!("date: \"{}\"\n", display_date(email)));
    if let Some(ref parsed) = email.date_parsed {
        out.push_str(&format!("date_iso: \"{}\"\n", parsed));
    }
    out.push_str(&format!("subject: \"{}\"\n", yaml_escape(&email.subject)));
    if !email.message_id.is_empty() {
        out.push_str(&format!(
            "message_id: \"{}\"\n",
            yaml_escape(&email.message_id)
        ));
    }
    if !email.in_reply_to.is_empty() {
        out.push_str(&format!(
            "in_reply_to: \"{}\"\n",
            yaml_escape(&email.in_reply_to)
        ));
    }

    let body_type_str = match email.body_type {
        BodyType::PlainText => "text/plain",
        BodyType::HtmlConverted => "text/html (converted)",
        BodyType::Empty => "empty",
    };
    out.push_str(&format!("body_type: \"{}\"\n", body_type_str));

    if !email.attachments.is_empty() {
        out.push_str("attachments:\n");
        for att in &email.attachments {
            out.push_str(&format!(
                "  - name: \"{}\"\n    type: \"{}\"\n    size: {}\n",
                yaml_escape(&att.filename),
                att.content_type,
                att.size
            ));
        }
    }

    out.push_str(&format!("source: \"{}\"\n", yaml_escape(&email.source_path)));
    out.push_str("---\n\n");

    // Title
    out.push_str(&format!("# {}\n\n", email.subject));

    if full_headers {
        out.push_str("## Headers\n\n");
        out.push_str("| Key | Value |\n");
        out.push_str("|-----|-------|\n");
        for (key, value) in &email.all_headers {
            out.push_str(&format!(
                "| {} | {} |\n",
                md_table_escape(key),
                md_table_escape(value)
            ));
        }
        out.push('\n');
    }

    if !metadata_only {
        match email.body_type {
            BodyType::HtmlConverted => {
                out.push_str("*[Converted from HTML]*\n\n");
            }
            BodyType::Empty => {
                out.push_str("*[No body content]*\n");
            }
            _ => {}
        }
        out.push_str(&email.body);
    }

    out
}

/// Produce JSON output for a single email.
pub fn to_json(email: &EmailData, metadata_only: bool) -> Result<String> {
    let value = email_to_json_value(email, metadata_only);
    Ok(serde_json::to_string_pretty(&value)?)
}

/// Produce JSON output for multiple emails.
pub fn to_json_array(emails: &[EmailData], metadata_only: bool) -> Result<String> {
    let values: Vec<Value> = emails
        .iter()
        .map(|e| email_to_json_value(e, metadata_only))
        .collect();
    Ok(serde_json::to_string_pretty(&values)?)
}

fn email_to_json_value(email: &EmailData, metadata_only: bool) -> Value {
    let mut obj = json!({
        "from": email.from,
        "to": email.to,
        "date": email.date,
        "date_parsed": email.date_parsed,
        "subject": email.subject,
        "message_id": email.message_id,
        "source_path": email.source_path,
    });

    if !email.cc.is_empty() {
        obj["cc"] = json!(email.cc);
    }
    if !email.in_reply_to.is_empty() {
        obj["in_reply_to"] = json!(email.in_reply_to);
    }

    let body_type_str = match email.body_type {
        BodyType::PlainText => "text/plain",
        BodyType::HtmlConverted => "text/html (converted)",
        BodyType::Empty => "empty",
    };
    obj["body_type"] = json!(body_type_str);

    if !email.attachments.is_empty() {
        obj["attachments"] = json!(email.attachments);
    }

    if !metadata_only {
        obj["body"] = json!(email.body);
    }

    obj
}

/// Generate a safe filename from subject and date.
pub fn safe_filename(subject: &str, date: &str) -> String {
    // Try to extract a date prefix
    let date_prefix = extract_date_prefix(date);

    // Sanitize subject for use as filename
    let safe_subject = sanitize_filename(subject);

    if date_prefix.is_empty() {
        if safe_subject.is_empty() {
            "unnamed-email".to_string()
        } else {
            safe_subject
        }
    } else if safe_subject.is_empty() {
        date_prefix
    } else {
        // Truncate subject to reasonable length
        let max_subject_len = 60;
        let truncated = if safe_subject.len() > max_subject_len {
            safe_subject[..max_subject_len].to_string()
        } else {
            safe_subject
        };
        format!("{}-{}", date_prefix, truncated)
    }
}

fn extract_date_prefix(date: &str) -> String {
    // Try to parse RFC 2822 date
    if let Ok(dt) = chrono::DateTime::<chrono::FixedOffset>::parse_from_rfc2822(date) {
        return dt.format("%Y-%m-%d").to_string();
    }
    String::new()
}

fn sanitize_filename(s: &str) -> String {
    let re = Regex::new(r"[^a-zA-Z0-9_-]").unwrap();
    let result = re.replace_all(s, "-").to_string();
    // Collapse multiple hyphens
    let multi_hyphen = Regex::new(r"-{2,}").unwrap();
    let result = multi_hyphen.replace_all(&result, "-").to_string();
    // Trim leading/trailing hyphens
    result.trim_matches('-').to_lowercase()
}

fn yaml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn md_table_escape(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn display_date(email: &EmailData) -> String {
    if let Some(ref parsed) = email.date_parsed {
        parsed.clone()
    } else {
        email.date.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_filename_with_date() {
        let name = safe_filename("Meeting notes re: Q1 budget", "Thu, 13 Feb 2025 10:30:00 +0000");
        assert!(name.starts_with("2025-02-13"));
        assert!(name.contains("meeting-notes"));
    }

    #[test]
    fn test_safe_filename_no_date() {
        let name = safe_filename("Hello World!", "");
        assert_eq!(name, "hello-world");
    }

    #[test]
    fn test_safe_filename_empty() {
        let name = safe_filename("", "");
        assert_eq!(name, "unnamed-email");
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(
            sanitize_filename("Re: Meeting @10am!! (urgent)"),
            "re-meeting-10am-urgent"
        );
    }

    #[test]
    fn test_yaml_escape() {
        assert_eq!(yaml_escape(r#"say "hello""#), r#"say \"hello\""#);
    }
}
