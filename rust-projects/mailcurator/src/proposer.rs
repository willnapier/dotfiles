//! `propose-policy` subcommand — sample N messages from a sender,
//! ask Claude to draft a `[[policy]]` + optional `[[policy.extractor]]`
//! TOML block. Always quarantine-first: never proposes `delete_after_days`
//! on the first draft for a sender.
//!
//! Composes with the existing subcommand surface:
//!   - `mailcurator unmatched` finds sender candidates (FN detection)
//!   - `mailcurator propose-policy <sender>` drafts the block (this file)
//!   - `mailcurator validate` confirms the block parses cleanly
//!   - `mailcurator preview <name>` shows what the block would match
//!   - `mailcurator improve <name>` refines the proposal post-paste via
//!     the Karpathy loop
//!
//! Cuts per-policy authoring time from roughly 30 min (read messages,
//! decide lifecycle, write extractor regex by hand) to a 5 min review
//! pass over Claude's draft. The reviewer remains the gate — output is
//! printed to stdout for paste-into-policies.toml, not auto-appended.

use anyhow::{Context, Result};
use mailparse::{MailHeaderMap, parse_mail};
use std::process::Command;

use crate::llm;

/// Maximum body bytes shown to the model per sampled message. Keeps the
/// prompt within sensible context-window bounds even for HTML-heavy
/// senders whose plain-text part is ~10KB.
const MAX_BODY_BYTES: usize = 2048;

const MIN_SAMPLES: usize = 1;
const MAX_SAMPLES: usize = 20;

/// Entry point for the `propose-policy` subcommand.
pub fn propose_policy(sender: &str, samples: usize, body_samples: usize) -> Result<()> {
    llm::probe()?;

    let samples = samples.clamp(MIN_SAMPLES, MAX_SAMPLES);
    let body_samples = body_samples.min(samples);

    let query = format!("from:{sender}");
    let paths = list_files(&query)?;

    if paths.is_empty() {
        anyhow::bail!(
            "no messages found from `{sender}`. \
             Try `mailcurator unmatched` to see candidate senders, \
             or check the from-match pattern."
        );
    }

    // Reservoir-sample: prefer a spread across the sender's history over
    // the most-recent slice, so seasonal variation in body shape is more
    // likely to be in the sample.
    use rand::seq::SliceRandom;
    let mut rng = rand::thread_rng();
    let chosen: Vec<&String> = if paths.len() <= samples {
        paths.iter().collect()
    } else {
        paths.choose_multiple(&mut rng, samples).collect()
    };

    eprintln!(
        "Sampling {} of {} messages from `{sender}`...",
        chosen.len(),
        paths.len()
    );

    let mut envelopes = Vec::new();
    for path in &chosen {
        match read_envelope_and_body(path) {
            Ok(env) => envelopes.push(env),
            Err(e) => eprintln!("  skipped {path}: {e}"),
        }
    }

    if envelopes.is_empty() {
        anyhow::bail!("could not parse any of the sampled messages");
    }

    let prompt = build_prompt(sender, &envelopes, body_samples);

    eprintln!("Asking Claude to draft policy block...");
    let response = llm::ask(&prompt)?;

    let block = strip_code_fences(&response);
    println!("{block}");

    eprintln!();
    eprintln!("# Review the above and paste into ~/dotfiles/mailcurator/policies.toml");
    eprintln!("# Then: `mailcurator validate` to confirm it parses,");
    eprintln!("#       `mailcurator preview <name>` to see what it matches,");
    eprintln!("#       `mailcurator improve <name>` to refine post-paste.");

    Ok(())
}

struct Envelope {
    subject: String,
    from: String,
    date: String,
    body: String,
}

/// Return absolute paths of every maildir file whose envelope matches
/// the notmuch query. Different from `store::list_messages` which
/// returns thread summaries — we need file paths so we can parse the
/// full body, not just the indexed metadata.
fn list_files(query: &str) -> Result<Vec<String>> {
    let output = Command::new("notmuch")
        .args(["search", "--output=files", query])
        .output()
        .with_context(|| format!("spawning `notmuch search --output=files {query}`"))?;
    if !output.status.success() {
        anyhow::bail!(
            "notmuch search failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let s = String::from_utf8_lossy(&output.stdout);
    Ok(s.lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect())
}

fn read_envelope_and_body(path: &str) -> Result<Envelope> {
    let raw = std::fs::read(path).with_context(|| format!("reading {path}"))?;
    let parsed = parse_mail(&raw).with_context(|| format!("parsing {path}"))?;
    let subject = parsed.headers.get_first_value("Subject").unwrap_or_default();
    let from = parsed.headers.get_first_value("From").unwrap_or_default();
    let date = parsed.headers.get_first_value("Date").unwrap_or_default();
    let body = extract_text_body(&parsed);
    Ok(Envelope {
        subject,
        from,
        date,
        body,
    })
}

/// Prefer `text/plain` if any subpart has it; fall back to any `text/*`
/// part otherwise. Returns the empty string if no text part exists
/// (caller decides how to handle — typically by including the envelope
/// only).
fn extract_text_body(part: &mailparse::ParsedMail) -> String {
    if part.ctype.mimetype == "text/plain" {
        return part.get_body().unwrap_or_default();
    }
    for sub in &part.subparts {
        let b = extract_text_body(sub);
        if !b.is_empty() {
            return b;
        }
    }
    if part.ctype.mimetype.starts_with("text/") {
        return part.get_body().unwrap_or_default();
    }
    String::new()
}

fn build_prompt(sender: &str, envelopes: &[Envelope], body_samples: usize) -> String {
    let mut p = String::new();
    p.push_str(
        "You are authoring a mailcurator policy block for a specific sender. \
         mailcurator is a Rust tool that applies per-sender lifecycle policies to \
         email (archive-after-N-days, optional trash-after-M-days) and optionally \
         extracts structured data from message bodies into JSONL stores.\n\n",
    );
    p.push_str(&format!("Sender to handle: {sender}\n\n"));

    p.push_str(&format!(
        "Sample messages from this sender (n={}):\n\n",
        envelopes.len()
    ));
    for (i, e) in envelopes.iter().enumerate() {
        p.push_str(&format!(
            "{}. From: {} | Date: {} | Subject: {}\n",
            i + 1,
            truncate(&e.from, 80),
            truncate(&e.date, 40),
            truncate(&e.subject, 200)
        ));
    }
    p.push('\n');

    if body_samples > 0 {
        p.push_str(&format!(
            "Body extracts (first {body_samples}, truncated to {MAX_BODY_BYTES} bytes each):\n\n"
        ));
        for (i, e) in envelopes.iter().take(body_samples).enumerate() {
            p.push_str(&format!("--- Message {} body ---\n", i + 1));
            p.push_str(&truncate(&e.body, MAX_BODY_BYTES));
            p.push_str("\n\n");
        }
    }

    p.push_str(POLICY_INSTRUCTIONS);
    p
}

const POLICY_INSTRUCTIONS: &str = r#"
Decide:

1. Lifecycle category — what kind of email is this? Pick one or two from:
   - auth-code (verification codes, OTP, password reset) — delete after 1-3 days
   - marketing (promotional, commercial intent) — archive after 1-3 days, optionally delete after 7-30
   - newsletter (regular informational, low retention value) — archive after 3-7 days, optionally delete after 14-30
   - invoice (receipts, financial transactions) — archive after 7 days; only set delete_after_days if extractors are present (extract-and-destroy)
   - notification (service notifications, sign-in alerts) — archive after 1-3 days, optionally delete after 7-14
   - delivery (parcel tracking) — extract tracking + ETA, archive after 3-7 days
   - shipping-receipt (order receipts with retention value) — archive after 7-14 days
   - calendar-invite (meeting invites) — archive after 3-7 days
   - correspondence (human-to-human) — archive after 30+ days, do NOT delete
   - bulk-marketing (mass commercial, opt-out senders) — delete after 1-3 days
   - other (anything else) — archive after 7 days

2. Should this policy extract structured data? Look at the body extracts.
   If they contain repeating structured fields (amounts, dates, identifiers,
   tracking numbers, receipt numbers) worth capturing into a JSONL store,
   propose an extractor block. If the messages are pure communication /
   marketing / notification with no extractable data, OMIT the extractor.

3. QUARANTINE-FIRST — do NOT set `delete_after_days` on this first draft.
   Always omit it or comment it out with a note. The reviewer will enable
   destruction after a week of clean operation, never on draft.

Categories for the extracted JSONL store (`[[policy.extractor]] category`):
   bills | orders | journeys | bookings | deliveries | subscriptions

Field-rule shapes inside `[[policy.extractor.field]]`:
   - `literal = "Value"`       — fixed string (typical for `vendor`)
   - `body_regex = '£(\d+\.\d{2})'` — regex against decoded body text
   - `subject_regex = '...'`   — regex against Subject header
   - `header = "Date"`         — header value passthrough (RFC 2822)
   - `kind = "date"`           — normalise captured string to ISO YYYY-MM-DD
     (supports ISO, dd/mm/yyyy, "23 April 2026", "April 23, 2026" American
     comma-form). Apply this to any date-valued field (`due_date`,
     `payment_date`, `expected_delivery`, etc.).

Output rules:
- Output ONLY the TOML block. No surrounding prose, no markdown code fences.
- Use kebab-case for `name`.
- For `from`, prefer the exact From header if it's a stable address
  (e.g. `invoice+statements@mail.example.com`), or `@domain` if the
  sender varies within a domain.
- Lead with two comment lines: a one-line purpose + a one-line lifecycle
  rationale. These help the reviewer skim policies.toml later.

Format:

# {policy-slug} — {one-line purpose}.
# Lifecycle: {one-line reasoning}.
[[policy]]
name = "{kebab-case-slug}"
from = "{exact-email or @domain}"
intended_categories = ["{category}"]
archive_after_days = {N}
# delete_after_days = {M}  # QUARANTINED — enable after a week of clean operation.

# Optional, only if extractable:
[[policy.extractor]]
category = "{store}"

[[policy.extractor.field]]
name = "vendor"
literal = "{vendor name observed}"

[[policy.extractor.field]]
name = "amount"
body_regex = '{regex tuned to body shape}'

# ... one [[policy.extractor.field]] block per extractable field.

Output the TOML block now:
"#;

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

fn strip_code_fences(s: &str) -> String {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("```toml") {
        return rest.trim_start().trim_end_matches("```").trim().to_string();
    }
    if let Some(rest) = trimmed.strip_prefix("```") {
        return rest.trim_start().trim_end_matches("```").trim().to_string();
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_short_strings() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_appends_ellipsis_when_cut() {
        let s = "abcdefghij";
        let out = truncate(s, 5);
        assert!(out.ends_with('…'));
        assert!(out.len() < s.len() + 4); // ellipsis is 3 bytes in UTF-8
    }

    #[test]
    fn truncate_respects_utf8_boundaries() {
        // "café" is 5 bytes (a + b + c + e + 0x82). Truncating at byte 4
        // would split the é (which is 0xc3 0xa9). Truncate should back off.
        let s = "café";
        let out = truncate(s, 4);
        // Either backs off to before é, or includes é fully.
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn strip_code_fences_removes_toml_fence() {
        let raw = "```toml\n[[policy]]\nname = \"x\"\n```";
        let out = strip_code_fences(raw);
        assert!(out.starts_with("[[policy]]"));
        assert!(!out.contains("```"));
    }

    #[test]
    fn strip_code_fences_passes_through_unfenced() {
        let raw = "[[policy]]\nname = \"x\"";
        let out = strip_code_fences(raw);
        assert_eq!(out, raw);
    }
}
