// Claude CLI wrapper — used by the eval/label/improve subcommands.
//
// Shells out to `claude -p` (Claude Code's non-interactive mode). Matches
// the pattern practiceforge uses: the user has already authenticated via
// `claude auth login`, so no API key is needed in config. Outputs are
// returned as plain text for the caller to parse.
//
// The prompts are constructed by eval.rs; this module only handles the
// subprocess invocation.

use anyhow::{Context, Result};
use serde_json::{Map, Value};
use std::io::Write;
use std::process::{Command, Stdio};

/// Send a prompt to Claude via `claude -p` and return the response text.
/// The prompt is passed on stdin to avoid argv-length limits.
pub fn ask(prompt: &str) -> Result<String> {
    let mut child = Command::new("claude")
        .arg("-p")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning `claude -p` — is Claude Code installed and authenticated?")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes())
            .context("writing prompt to claude stdin")?;
        // Drop stdin so claude sees EOF and starts processing.
    }

    let output = child.wait_with_output().context("waiting for claude to finish")?;
    if !output.status.success() {
        anyhow::bail!(
            "claude exited with status {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Truncate HTML to a safe input size for the LLM. Email HTML is
/// frequently 50KB+ of styles + tracking pixels + dark-mode CSS variants
/// before any actual content. We use a target window of 16KB centered
/// on signal-bearing tokens (£, Total, Order, Reservation, Check-in)
/// rather than just the first N bytes — that early-truncation strategy
/// missed Amazon's order-summary tables which sit at byte 90KB+ in some
/// templates.
const MAX_HTML_BYTES: usize = 16_384;

/// Ask Claude to extract structured data per `schema`, returning a JSON
/// object. The prompt constrains the model to JSON-only output; we still
/// parse defensively (model may emit a leading code-fence or trailing
/// prose, which we strip before parsing).
///
/// `vendor_label` is a human-readable name for the email type
/// ("Amazon order", "Trainline journey") — included in the prompt so
/// Claude has correct context.
///
/// Errors are returned for invocation failure and malformed JSON. A
/// successful call may still return an empty `Map` if the email has no
/// extractable data.
pub fn extract_structured(
    vendor_label: &str,
    schema: &str,
    html: &str,
) -> Result<Map<String, Value>> {
    let truncated = signal_window(html, MAX_HTML_BYTES);
    let truncated = truncated.as_str();

    let prompt = format!(
        "You are extracting structured data from a {vendor_label} email. \
Return ONLY a JSON object matching this schema (no surrounding prose, no \
markdown fences). Omit fields you cannot find — do NOT invent values. If \
the email contains nothing matching the schema, return an empty object {{}}.

Schema:
{schema}

Email HTML (truncated to {} bytes):
{truncated}",
        truncated.len()
    );

    let raw = ask(&prompt)?;
    parse_json_object(&raw)
}

/// Find the byte offset of the first signal token (£, Total, Order,
/// Reservation, HH:MM time, etc.) and return a window of `target_bytes`
/// centered on it. Falls back to a leading-window slice if no signal is
/// found. Always cuts at UTF-8 char boundaries.
fn signal_window(html: &str, target_bytes: usize) -> String {
    if html.len() <= target_bytes {
        return html.to_string();
    }

    // Phase 1: literal signal tokens. Cheap, pick earliest. These cover
    // Amazon (£/Grand Total), Airbnb (Reservation/Check-in), and
    // generic invoice-shaped emails.
    let literal_signals = [
        "Grand Total", "Order Total", "Total Before Tax", "£", "Total:",
        "Reservation", "Check-in", "Confirmation",
        "Depart", "Arrive", "Journey", "Trainline",
    ];
    let earliest_literal = literal_signals
        .iter()
        .filter_map(|s| html.find(s))
        .min();

    // Phase 2: regex-based signals for vendors whose templates don't
    // use the literal tokens above. HH:MM is a strong signal for
    // Trainline journey blocks.
    static TIME_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let time_re = TIME_RE.get_or_init(|| regex::Regex::new(r"\b\d{2}:\d{2}\b").unwrap());
    let earliest_time = time_re.find(html).map(|m| m.start());

    // Pick the earliest of all candidates.
    let anchor = match (earliest_literal, earliest_time) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => 0,
    };

    // Center the window: half before anchor, half after.
    let half = target_bytes / 2;
    let start_raw = anchor.saturating_sub(half);
    let end_raw = (anchor + half).min(html.len());

    // Snap to UTF-8 boundaries.
    let mut start = start_raw;
    while start > 0 && !html.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = end_raw;
    while end < html.len() && !html.is_char_boundary(end) {
        end += 1;
    }
    html[start..end].to_string()
}

/// Strip common LLM output cruft (code fences, leading/trailing prose)
/// and parse what's left as a JSON object. Returns `Ok(empty Map)` if
/// the response is unparseable rather than erroring — extraction is
/// best-effort.
fn parse_json_object(raw: &str) -> Result<Map<String, Value>> {
    let trimmed = raw.trim();

    // Strip code fences if present.
    let stripped = if let Some(rest) = trimmed.strip_prefix("```json") {
        rest.trim_start().trim_end_matches("```").trim()
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        rest.trim_start().trim_end_matches("```").trim()
    } else {
        trimmed
    };

    // Find first { and last } to extract the JSON envelope. Tolerant of
    // a leading "Here's the data:" or trailing prose.
    let start = stripped.find('{');
    let end = stripped.rfind('}');
    let body = match (start, end) {
        (Some(s), Some(e)) if e > s => &stripped[s..=e],
        _ => return Ok(Map::new()),
    };

    let parsed: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return Ok(Map::new()),
    };
    match parsed {
        Value::Object(m) => Ok(m),
        _ => Ok(Map::new()),
    }
}

/// Check that the claude CLI is available and authenticated. Cheap smoke test.
pub fn probe() -> Result<()> {
    let output = Command::new("claude")
        .arg("--version")
        .output()
        .context("spawning `claude --version`")?;
    if !output.status.success() {
        anyhow::bail!(
            "claude --version failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}
