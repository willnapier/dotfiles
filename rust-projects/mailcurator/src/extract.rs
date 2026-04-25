// Content extraction: per-policy extractors that pull structured fields
// from messages and append them to JSONL files in ~/.local/share/mailcurator/.
//
// Design principle (from the project memory): extract and destroy. Once a
// message's information is captured to plain-text JSONL, the email envelope
// is disposable. Lifecycle's `delete_after_days` is gated on the
// `curator-<policy>-extracted` tag, so messages we haven't yet captured
// from won't be destroyed.
//
// Field rule kinds:
//   - literal     hard-coded value (e.g. carrier name)
//   - header      pull from a named RFC822 header (case-insensitive)
//   - body_regex  regex on decoded, HTML-stripped body; first capture group
//                 is the value (or whole match if no groups)
//   - subject_regex  same idea against the Subject header
//
// Bodies are decoded via mailparse (handles MIME multipart + transfer
// encodings). HTML parts are crudely stripped (regex `<[^>]+>` → space)
// before regexes apply — sufficient for the structured-data patterns in
// transactional emails (tracking numbers, dates, amounts).

use anyhow::{Context, Result};
use mailparse::{parse_mail, MailHeaderMap};
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs;
use std::process::Command;

use crate::policy::{FieldRule, Policy};
use crate::store;

/// Run a policy's extractors over every matching message not yet tagged
/// `curator-<name>-extracted`. For each:
///   1. read the message file from disk
///   2. parse + decode body (mailparse handles MIME + transfer encoding)
///   3. apply each FieldRule
///   4. append the resulting record(s) to <category>.jsonl
///   5. tag the message with the extracted-tag (idempotency)
///
/// Returns the number of messages actually extracted.
pub fn run_extractors(pol: &Policy, dry_run: bool) -> Result<u64> {
    if pol.extractors.is_empty() {
        return Ok(0);
    }

    let base = pol.base_query();
    let extracted_tag = pol.extracted_tag();
    let query = format!("({base}) and not tag:{extracted_tag}");

    // Get message file paths
    let files = list_files(&query)?;
    if files.is_empty() {
        return Ok(0);
    }

    if dry_run {
        return Ok(files.len() as u64);
    }

    let mut count = 0u64;
    for path in &files {
        match extract_one(pol, path) {
            Ok(()) => count += 1,
            Err(e) => {
                eprintln!(
                    "  [{}] extract failed for {}: {}",
                    pol.name,
                    path.split('/').next_back().unwrap_or(path),
                    e
                );
                // Continue: one bad message shouldn't stop the batch.
            }
        }
    }

    // Tag all successfully-extracted messages with the extracted-tag in one
    // notmuch call. We re-query for the same set; any that had errors won't
    // appear because we don't apply this tag to them. Simpler approach:
    // only tag the IDs we actually extracted (use msg-id from the files).
    // For now, rely on the per-message tag application inside extract_one.
    Ok(count)
}

fn list_files(query: &str) -> Result<Vec<String>> {
    let output = Command::new("notmuch")
        .args(["search", "--output=files", query])
        .output()
        .with_context(|| format!("spawning notmuch search for files matching: {query}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "notmuch search --output=files failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

fn extract_one(pol: &Policy, path: &str) -> Result<()> {
    let raw = fs::read(path).with_context(|| format!("reading {path}"))?;
    let parsed = parse_mail(&raw).with_context(|| format!("parsing RFC822 from {path}"))?;

    // Cache derived data so multi-rule extractors don't re-do work.
    let subject = parsed.headers.get_first_value("Subject").unwrap_or_default();
    let body_text = decode_body_to_text(&parsed)?;
    let message_id = parsed.headers.get_first_value("Message-ID").unwrap_or_else(|| path.to_string());

    for ex in &pol.extractors {
        let mut record = Map::new();
        record.insert("message_id".into(), Value::String(message_id.clone()));
        record.insert("policy".into(), Value::String(pol.name.clone()));
        record.insert(
            "extracted_at".into(),
            Value::String(chrono::Utc::now().to_rfc3339()),
        );

        for f in &ex.fields {
            if let Some(v) = apply_rule(f, &parsed, &subject, &body_text)? {
                record.insert(f.name.clone(), Value::String(v));
            }
        }

        store::append_record(&ex.category, &Value::Object(record))?;
    }

    // Tag this single message as extracted (idempotent).
    crate::notmuch::apply_tag_changes(
        &format!("id:\"{}\"", message_id.trim_start_matches('<').trim_end_matches('>')),
        &[&pol.extracted_tag()],
        &[],
    )?;

    Ok(())
}

fn apply_rule(
    f: &FieldRule,
    parsed: &mailparse::ParsedMail,
    subject: &str,
    body_text: &str,
) -> Result<Option<String>> {
    if let Some(lit) = &f.literal {
        return Ok(Some(lit.clone()));
    }
    if let Some(h) = &f.header {
        return Ok(parsed.headers.get_first_value(h));
    }
    if let Some(re) = &f.subject_regex {
        return Ok(apply_regex(re, subject)?);
    }
    if let Some(re) = &f.body_regex {
        return Ok(apply_regex(re, body_text)?);
    }
    Ok(None)
}

fn apply_regex(pattern: &str, text: &str) -> Result<Option<String>> {
    let re = Regex::new(pattern).with_context(|| format!("compiling regex {pattern}"))?;
    Ok(re.captures(text).map(|caps| {
        // Use first capture group if present, else whole match.
        caps.get(1).or_else(|| caps.get(0)).map(|m| m.as_str().to_string())
    }).flatten())
}

/// Decode message body to plain text. For multipart messages, prefer
/// text/plain; fall back to HTML-stripped text/html. Walks all parts.
fn decode_body_to_text(parsed: &mailparse::ParsedMail) -> Result<String> {
    // Build a map of content-type → decoded body. Pick text/plain first,
    // then text/html (with HTML stripped).
    let mut bodies: HashMap<String, String> = HashMap::new();
    collect_bodies(parsed, &mut bodies);

    if let Some(plain) = bodies.get("text/plain") {
        return Ok(plain.clone());
    }
    if let Some(html) = bodies.get("text/html") {
        return Ok(strip_html(html));
    }
    // Last resort: any text we found.
    Ok(bodies.values().next().cloned().unwrap_or_default())
}

fn collect_bodies(part: &mailparse::ParsedMail, out: &mut HashMap<String, String>) {
    let ctype = part.ctype.mimetype.to_lowercase();
    if part.subparts.is_empty() {
        if ctype.starts_with("text/") {
            if let Ok(body) = part.get_body() {
                out.entry(ctype).or_insert(body);
            }
        }
        return;
    }
    for sub in &part.subparts {
        collect_bodies(sub, out);
    }
}

/// Crude HTML-to-text: replace tags with spaces, decode common entities,
/// collapse whitespace. Sufficient for regex extraction of structured tokens
/// like tracking numbers and dates from transactional email bodies.
fn strip_html(html: &str) -> String {
    let tagless = Regex::new(r"<[^>]+>").unwrap().replace_all(html, " ");
    let entities: &[(&str, &str)] = &[
        ("&nbsp;", " "),
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
        ("&apos;", "'"),
    ];
    let mut s = tagless.into_owned();
    for (e, r) in entities {
        s = s.replace(e, r);
    }
    // Collapse whitespace runs to single spaces.
    let collapsed = Regex::new(r"\s+").unwrap().replace_all(&s, " ");
    collapsed.trim().to_string()
}
