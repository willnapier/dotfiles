use anyhow::{Context, Result};
use chrono::NaiveDate;
use std::io::Write;
use std::process::{Command, Stdio};

use crate::types::DraftEntry;

const DRAFT_PROMPT: &str = r#"You are drafting DayPage dev:: entries for missed development sessions.

Each session below is a development conversation that wasn't logged. For each, write ONE entry in this exact format:
DATE: YYYY-MM-DD
dev:: Nx Mmin - concise description of what was done

Rules:
- N = approximate exchange count (from message count)
- M = approximate duration in minutes (from start/end times)
- Use specific names: file names, tool names, feature names — NOT vague descriptions
- One entry per session, keep descriptions to 1-2 sentences
- Start description with past-tense verb (Built, Fixed, Discussed, Implemented, etc.)

Sessions to draft entries for:
"#;

/// Build prompt and call `claude -p` to draft entries for unmatched sessions.
pub fn draft_entries(
    sessions: &[(NaiveDate, &str)], // (date, session detail text)
) -> Result<Vec<DraftEntry>> {
    if sessions.is_empty() {
        return Ok(vec![]);
    }

    let mut input = String::new();
    for (date, detail) in sessions {
        input.push_str(&format!("\n--- Date: {} ---\n{}\n", date, detail));
    }

    let mut child = Command::new("claude")
        .args(["-p", DRAFT_PROMPT])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn claude -p")?;

    child
        .stdin
        .take()
        .expect("stdin not captured")
        .write_all(input.as_bytes())
        .context("Failed to write to claude stdin")?;

    let output = child
        .wait_with_output()
        .context("Failed to wait for claude -p")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("claude -p failed: {}", stderr.trim());
    }

    let response =
        String::from_utf8(output.stdout).context("claude -p output is not valid UTF-8")?;

    parse_draft_response(&response)
}

/// Parse the claude -p response into DraftEntry items.
fn parse_draft_response(response: &str) -> Result<Vec<DraftEntry>> {
    let mut entries = vec![];
    let mut current_date: Option<NaiveDate> = None;

    // Strip markdown fences if present
    let text = response
        .trim()
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    for line in text.lines() {
        let line = line.trim();

        if let Some(date_str) = line.strip_prefix("DATE:") {
            let date_str = date_str.trim();
            if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                current_date = Some(date);
            }
        } else if line.contains("dev::") {
            if let Some(date) = current_date {
                // Extract the dev:: entry (may have leading "- " or other prefix)
                let entry = if let Some(pos) = line.find("dev::") {
                    line[pos..].to_string()
                } else {
                    line.to_string()
                };
                entries.push(DraftEntry { date, entry });
            }
        }
    }

    Ok(entries)
}

/// Queue a drafted entry via daypage-append.
pub fn apply_entry(date: NaiveDate, entry: &str) -> Result<()> {
    let date_str = date.format("%Y-%m-%d").to_string();
    let status = Command::new("daypage-append")
        .arg("--date")
        .arg(&date_str)
        .arg(entry)
        .status()
        .context("Failed to run daypage-append")?;

    if !status.success() {
        anyhow::bail!("daypage-append failed for {}: exit {}", date_str, status);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_draft_response() {
        let response = r#"
DATE: 2026-03-07
dev:: 7x 20min - Discussed dev-catchup tool design with plan mode

DATE: 2026-03-06
dev:: 5x 15min - Built state-capture improvements
"#;
        let entries = parse_draft_response(response).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].date,
            NaiveDate::from_ymd_opt(2026, 3, 7).unwrap()
        );
        assert!(entries[0].entry.contains("dev-catchup"));
        assert_eq!(
            entries[1].date,
            NaiveDate::from_ymd_opt(2026, 3, 6).unwrap()
        );
    }

    #[test]
    fn test_parse_draft_with_markdown_fences() {
        let response = r#"```
DATE: 2026-03-07
dev:: 7x 20min - Something here
```"#;
        let entries = parse_draft_response(response).unwrap();
        assert_eq!(entries.len(), 1);
    }
}
