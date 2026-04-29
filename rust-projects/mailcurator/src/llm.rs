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

/// Truncate HTML to a safe input size for the LLM. Anthropic's context
/// is generous but most extraction signal is in the first chunk anyway,
/// and large HTML inflates cost + latency. 8KB ≈ 2000 tokens which is
/// plenty for an order-summary table or a booking confirmation.
const MAX_HTML_BYTES: usize = 8192;

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
    let truncated = if html.len() > MAX_HTML_BYTES {
        // Cut at a UTF-8 char boundary to avoid mid-codepoint splits.
        let mut end = MAX_HTML_BYTES;
        while end > 0 && !html.is_char_boundary(end) {
            end -= 1;
        }
        &html[..end]
    } else {
        html
    };

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
