use anyhow::{Context, Result};
use chrono::{DateTime, FixedOffset};
use mailparse::{parse_mail, MailHeaderMap, ParsedMail};
use regex::Regex;
use serde::Serialize;
use std::path::Path;

/// Structured email data extracted from a MIME message.
#[derive(Debug, Clone, Serialize)]
pub struct EmailData {
    pub from: String,
    pub to: String,
    pub cc: String,
    pub date: String,
    pub date_parsed: Option<String>,
    pub subject: String,
    pub message_id: String,
    pub in_reply_to: String,
    pub body: String,
    pub body_type: BodyType,
    pub attachments: Vec<AttachmentInfo>,
    #[serde(skip)]
    pub all_headers: Vec<(String, String)>,
    pub source_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub enum BodyType {
    PlainText,
    HtmlConverted,
    Empty,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttachmentInfo {
    pub filename: String,
    pub content_type: String,
    pub size: usize,
}

/// Parse an email file from disk into structured EmailData.
pub fn parse_email(path: &Path, prefer_html: bool, strip_html: bool) -> Result<EmailData> {
    let raw = std::fs::read(path)
        .with_context(|| format!("Failed to read email file: {}", path.display()))?;

    let parsed = parse_mail(&raw)
        .with_context(|| format!("Failed to parse MIME message: {}", path.display()))?;

    let headers = &parsed.headers;

    let from = headers
        .get_first_value("From")
        .unwrap_or_default();
    let to = headers
        .get_first_value("To")
        .unwrap_or_default();
    let cc = headers
        .get_first_value("Cc")
        .unwrap_or_default();
    let date_raw = headers
        .get_first_value("Date")
        .unwrap_or_default();
    let subject = headers
        .get_first_value("Subject")
        .unwrap_or_else(|| "(no subject)".to_string());
    let message_id = headers
        .get_first_value("Message-ID")
        .or_else(|| headers.get_first_value("Message-Id"))
        .unwrap_or_default();
    let in_reply_to = headers
        .get_first_value("In-Reply-To")
        .unwrap_or_default();

    // Parse date into ISO format
    let date_parsed = parse_email_date(&date_raw);

    // Collect all headers
    let all_headers: Vec<(String, String)> = headers
        .iter()
        .map(|h| {
            (
                h.get_key().to_string(),
                h.get_value().to_string(),
            )
        })
        .collect();

    // Extract body (text/plain preferred, HTML fallback)
    let (body, body_type) = extract_body(&parsed, prefer_html, strip_html);

    // Collect attachment info
    let attachments = extract_attachment_info(&parsed);

    Ok(EmailData {
        from,
        to,
        cc,
        date: date_raw,
        date_parsed,
        subject,
        message_id,
        in_reply_to,
        body,
        body_type,
        attachments,
        all_headers,
        source_path: path.display().to_string(),
    })
}

/// Extract the body from a parsed email message.
/// Prefers text/plain unless prefer_html is set.
/// Falls back to HTML with tag stripping if no text/plain is available.
fn extract_body(parsed: &ParsedMail, prefer_html: bool, strip_html: bool) -> (String, BodyType) {
    let mut text_body: Option<String> = None;
    let mut html_body: Option<String> = None;

    collect_body_parts(parsed, &mut text_body, &mut html_body);

    if prefer_html {
        if let Some(html) = html_body {
            let converted = html_to_text(&html, strip_html);
            return (converted, BodyType::HtmlConverted);
        }
        if let Some(text) = text_body {
            return (clean_text(&text), BodyType::PlainText);
        }
    } else {
        if let Some(text) = text_body {
            return (clean_text(&text), BodyType::PlainText);
        }
        if let Some(html) = html_body {
            let converted = html_to_text(&html, strip_html);
            return (converted, BodyType::HtmlConverted);
        }
    }

    (String::new(), BodyType::Empty)
}

/// Recursively collect text/plain and text/html parts from a MIME message.
fn collect_body_parts(
    parsed: &ParsedMail,
    text_body: &mut Option<String>,
    html_body: &mut Option<String>,
) {
    let content_type = parsed.ctype.mimetype.to_lowercase();

    if parsed.subparts.is_empty() {
        // Leaf node
        if let Ok(body) = parsed.get_body() {
            match content_type.as_str() {
                "text/plain" => {
                    if text_body.is_none() {
                        *text_body = Some(body);
                    }
                }
                "text/html" => {
                    if html_body.is_none() {
                        *html_body = Some(body);
                    }
                }
                _ => {}
            }
        }
    } else {
        // Recurse into multipart subparts
        for subpart in &parsed.subparts {
            collect_body_parts(subpart, text_body, html_body);
        }
    }
}

/// Extract attachment metadata (filename, content-type, size) from a MIME message.
fn extract_attachment_info(parsed: &ParsedMail) -> Vec<AttachmentInfo> {
    let mut attachments = Vec::new();
    collect_attachments(parsed, &mut attachments);
    attachments
}

fn collect_attachments(parsed: &ParsedMail, attachments: &mut Vec<AttachmentInfo>) {
    let content_type = parsed.ctype.mimetype.to_lowercase();
    let disposition = parsed
        .headers
        .get_first_value("Content-Disposition")
        .unwrap_or_default()
        .to_lowercase();

    if parsed.subparts.is_empty() {
        // Check if this is an attachment (not a body part)
        let is_attachment = disposition.starts_with("attachment")
            || (content_type != "text/plain"
                && content_type != "text/html"
                && !content_type.starts_with("multipart/"));

        if is_attachment {
            let filename = parsed
                .ctype
                .params
                .get("name")
                .cloned()
                .or_else(|| extract_filename_from_disposition(&disposition))
                .unwrap_or_else(|| "unnamed".to_string());

            let size = parsed
                .get_body_raw()
                .map(|b| b.len())
                .unwrap_or(0);

            attachments.push(AttachmentInfo {
                filename,
                content_type: content_type.clone(),
                size,
            });
        }
    } else {
        for subpart in &parsed.subparts {
            collect_attachments(subpart, attachments);
        }
    }
}

/// Extract filename from Content-Disposition header value.
fn extract_filename_from_disposition(disposition: &str) -> Option<String> {
    let re = Regex::new(r#"filename="?([^";\s]+)"?"#).ok()?;
    re.captures(disposition)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Convert HTML to plain text by stripping tags and decoding entities.
fn html_to_text(html: &str, aggressive_strip: bool) -> String {
    let mut text = html.to_string();

    // Replace common block elements with newlines
    let block_tags = Regex::new(r"(?i)<(?:br|p|div|tr|li|h[1-6])[^>]*>").unwrap();
    text = block_tags.replace_all(&text, "\n").to_string();

    // Replace hr with separator
    let hr_tag = Regex::new(r"(?i)<hr[^>]*>").unwrap();
    text = hr_tag.replace_all(&text, "\n---\n").to_string();

    // Remove style and script blocks entirely
    let style_script = Regex::new(r"(?is)<(?:style|script)[^>]*>.*?</(?:style|script)>").unwrap();
    text = style_script.replace_all(&text, "").to_string();

    // Remove all remaining HTML tags
    let all_tags = Regex::new(r"<[^>]+>").unwrap();
    text = all_tags.replace_all(&text, "").to_string();

    // Decode common HTML entities
    text = text
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&#160;", " ");

    // Decode numeric entities
    let numeric_entity = Regex::new(r"&#(\d+);").unwrap();
    text = numeric_entity
        .replace_all(&text, |caps: &regex::Captures| {
            let code: u32 = caps[1].parse().unwrap_or(0);
            char::from_u32(code)
                .map(|c| c.to_string())
                .unwrap_or_default()
        })
        .to_string();

    if aggressive_strip {
        // Collapse multiple blank lines
        let multi_blank = Regex::new(r"\n{3,}").unwrap();
        text = multi_blank.replace_all(&text, "\n\n").to_string();

        // Trim leading/trailing whitespace on each line
        text = text
            .lines()
            .map(|line| line.trim())
            .collect::<Vec<_>>()
            .join("\n");
    }

    clean_text(&text)
}

/// Clean up text: normalize line endings, trim trailing whitespace.
fn clean_text(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    // Trim trailing whitespace on each line, preserve structure
    let lines: Vec<&str> = text.lines().map(|l| l.trim_end()).collect();
    let mut result = lines.join("\n");
    // Ensure single trailing newline
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Parse email Date header into ISO 8601 format.
fn parse_email_date(date_str: &str) -> Option<String> {
    if date_str.is_empty() {
        return None;
    }

    let cleaned = date_str.trim();

    // Try parsing with chrono's RFC 2822 parser directly
    if let Ok(dt) = DateTime::<FixedOffset>::parse_from_rfc2822(cleaned) {
        return Some(dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string());
    }

    // Many real emails have wrong day-of-week or extra whitespace.
    // Strip the day-of-week prefix and try again.
    let stripped = strip_day_prefix(cleaned);
    if let Ok(dt) = DateTime::<FixedOffset>::parse_from_rfc2822(&stripped) {
        return Some(dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string());
    }

    // Try adding a dummy day-of-week if there isn't one
    // (some mailers omit it, but chrono may need it)
    let with_day = format!("Mon, {}", stripped);
    if let Ok(dt) = DateTime::<FixedOffset>::parse_from_rfc2822(&with_day) {
        return Some(dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string());
    }

    // Strip timezone name suffixes like "(GMT)" or "(PST)"
    let re_tz_name = Regex::new(r"\s*\([A-Z]{2,5}\)\s*$").ok()?;
    let no_tz_name = re_tz_name.replace(cleaned, "").to_string();
    if let Ok(dt) = DateTime::<FixedOffset>::parse_from_rfc2822(&no_tz_name) {
        return Some(dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string());
    }

    // Return None if we can't parse it -- the raw date is still available
    None
}

/// Strip "Mon, " style day-of-week prefix from a date string.
fn strip_day_prefix(s: &str) -> String {
    let re = Regex::new(r"^(?i)[A-Za-z]{3},\s*").unwrap();
    re.replace(s, "").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_to_text_basic() {
        let html = "<p>Hello <b>World</b></p><br>Line two";
        let result = html_to_text(html, false);
        assert!(result.contains("Hello"));
        assert!(result.contains("World"));
        assert!(result.contains("Line two"));
    }

    #[test]
    fn test_html_to_text_entities() {
        let html = "Tom &amp; Jerry &lt;friends&gt;";
        let result = html_to_text(html, false);
        assert!(result.contains("Tom & Jerry <friends>"));
    }

    #[test]
    fn test_html_to_text_style_removal() {
        let html = "<style>.foo { color: red; }</style><p>Content</p>";
        let result = html_to_text(html, false);
        assert!(!result.contains("color"));
        assert!(result.contains("Content"));
    }

    #[test]
    fn test_clean_text_normalizes_endings() {
        let text = "Line 1\r\nLine 2\rLine 3";
        let result = clean_text(text);
        assert_eq!(result, "Line 1\nLine 2\nLine 3\n");
    }

    #[test]
    fn test_parse_email_date_rfc2822() {
        let date = "Thu, 13 Feb 2025 10:30:00 +0000";
        let result = parse_email_date(date);
        assert!(result.is_some());
        assert!(result.unwrap().starts_with("2025-02-13"));
    }

    #[test]
    fn test_parse_email_date_empty() {
        assert!(parse_email_date("").is_none());
    }

    #[test]
    fn test_filename_from_disposition() {
        let d = r#"attachment; filename="report.pdf""#;
        let result = extract_filename_from_disposition(d);
        assert_eq!(result, Some("report.pdf".to_string()));
    }

    #[test]
    fn test_filename_from_disposition_no_quotes() {
        let d = "attachment; filename=report.pdf";
        let result = extract_filename_from_disposition(d);
        assert_eq!(result, Some("report.pdf".to_string()));
    }
}
