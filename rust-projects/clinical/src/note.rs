use anyhow::{bail, Context, Result};
use regex::Regex;
use std::io::{self, Write};
use std::process::Command;

use crate::{client, finalise, markdown, session};

/// Validation errors for LLM-generated notes.
struct ValidationResult {
    errors: Vec<String>,
}

impl ValidationResult {
    fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Validate that a generated note has the required structure.
fn validate_note(note: &str) -> ValidationResult {
    let mut errors = Vec::new();

    let date_re = Regex::new(r"^### \d{4}-\d{2}-\d{2}").unwrap();
    if !note.lines().any(|l| date_re.is_match(l)) {
        errors.push("Missing session header (### YYYY-MM-DD)".to_string());
    }

    if !note.contains("**Risk**:") {
        errors.push("Missing **Risk**: line".to_string());
    }

    if !note.contains("**Formulation**:") {
        errors.push("Missing **Formulation**: line".to_string());
    }

    // Check for refusal patterns
    let refusal_patterns = [
        "I can't",
        "I'm unable",
        "I cannot",
        "I'm not able",
        "I apologize",
        "I must decline",
    ];
    for pattern in &refusal_patterns {
        if note.contains(pattern) {
            errors.push(format!("Possible LLM refusal detected: \"{}\"", pattern));
            break;
        }
    }

    ValidationResult { errors }
}

/// Build the full prompt for the LLM.
fn build_prompt(context: &str, observation: &str) -> String {
    format!(
        "{context}\n\n\
         === INSTRUCTION ===\n\
         You are a clinical documentation assistant for a Chartered Psychologist.\n\
         Write a session note translating the observation below into ACT/CBS process language.\n\
         Use the template from the context above. Include **Risk**: and **Formulation**: lines.\n\
         Output ONLY the session note (starting with ### DATE), no preamble or explanation.\n\n\
         === OBSERVATION ===\n\
         {observation}"
    )
}

/// Capture the output of `clinical note-prepare` by running the logic directly.
fn capture_note_prepare(id: &str) -> Result<String> {
    // Redirect stdout to capture the output
    let path = client::notes_path(id);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read client file: {}", path.display()))?;

    let lines: Vec<&str> = content.lines().collect();

    let funding = markdown::extract_field(&content, "Funding").unwrap_or_default();
    let referral_type = markdown::extract_field(&content, "Referral type");
    let referring_doctor = markdown::extract_field(&content, "Referring doctor");
    let therapy_commenced = markdown::extract_field(&content, "Therapy commenced");
    let last_letter_raw =
        markdown::extract_field(&content, "Last update letter").unwrap_or_default();
    let next_specialist = markdown::extract_field(&content, "Next specialist appointment");
    let current_specialist = markdown::extract_field(&content, "Current specialist");

    let session_idx = session::find_session_section(&lines).unwrap_or(0);
    let session_lines = &lines[(session_idx + 1)..];
    let total_sessions = session::count_sessions(session_lines);
    let new_session_number = total_sessions + 1;

    let dna_re = Regex::new(r"^### \d{4}-\d{2}-\d{2} DNA").unwrap();
    let dna_count = session_lines.iter().filter(|l| dna_re.is_match(l)).count() as u32;

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    let auth_status = session::compute_auth_status(id, &content);
    let auth_markers = session::parse_auth_markers(&lines);

    let last_letter = if last_letter_raw == "none yet"
        || last_letter_raw == "null"
        || last_letter_raw.is_empty()
    {
        String::new()
    } else {
        last_letter_raw.clone()
    };
    let letter_status =
        session::compute_letter_status(new_session_number, &last_letter, session_lines);

    let mut out = String::new();

    out.push_str(&format!("=== CLINICAL NOTE CONTEXT: {} ===\n", id));
    out.push_str(&format!("Date: {}\n", today));
    out.push_str(&format!("Session number: {}\n", new_session_number));
    if dna_count > 0 {
        out.push_str(&format!("DNA sessions: {}\n", dna_count));
    }
    if let Some(ref tc) = therapy_commenced {
        out.push_str(&format!("Therapy commenced: {}\n", tc));
    }
    if !funding.is_empty() {
        out.push_str(&format!("Funding: {}\n", funding));
    }
    if let Some(ref rt) = referral_type {
        out.push_str(&format!("Referral type: {}\n", rt));
    }
    if let Some(ref rd) = referring_doctor {
        out.push_str(&format!("Referring doctor: {}\n", rd));
    }

    out.push_str("\n=== AUTH STATUS ===\n");
    if let Some(ref auth) = auth_status {
        let auth_date = auth_markers
            .last()
            .map(|m| m.auth_date.as_str())
            .unwrap_or("unknown");
        out.push_str(&format!(
            "{} of {} authorised sessions used (since {})\n",
            auth.sessions_used, auth.sessions_authorised, auth_date
        ));
        out.push_str(&format!("Remaining: {}\n", auth.remaining));
    } else {
        out.push_str("N/A (not insurer-funded)\n");
    }

    // Alerts
    out.push_str("\n=== ALERTS ===\n");
    let mut alerts: Vec<String> = Vec::new();

    if let Some(ref auth) = auth_status {
        if auth.remaining <= 2 {
            alerts.push(format!(
                "\u{26a0}\u{fe0f} Auth letter needed — {} sessions remaining",
                auth.remaining
            ));
        }
    }

    if let Some(ref rt) = referral_type {
        if rt.to_lowercase() == "doctor" && !letter_status.is_empty() {
            alerts.push(format!(
                "\u{1f4cb} Update letter due — session {}",
                new_session_number
            ));
        }
    }

    if let Some(ref next_appt) = next_specialist {
        if next_appt != "unknown" && next_appt != "N/A" && !next_appt.is_empty() {
            if let Ok(appt_date) = chrono::NaiveDate::parse_from_str(next_appt, "%Y-%m-%d") {
                let today_date = chrono::Local::now().date_naive();
                let days_until = (appt_date - today_date).num_days();
                if days_until >= 0 && days_until <= 14 {
                    let specialist_name = current_specialist
                        .as_deref()
                        .or(referring_doctor.as_deref())
                        .unwrap_or("specialist");
                    alerts.push(format!(
                        "\u{1f4cb} Update report due — {} appointment {}",
                        specialist_name, next_appt
                    ));
                }
            }
        }
    }

    if referral_type.is_none() {
        alerts.push("\u{2753} Referral type not set".to_string());
    }

    if alerts.is_empty() {
        out.push_str("[none]\n");
    } else {
        for alert in &alerts {
            out.push_str(&format!("{}\n", alert));
        }
    }

    // Recent sessions (last 3)
    out.push_str("\n=== RECENT SESSIONS (last 3) ===\n");
    let header_re = Regex::new(r"^### \d{4}-\d{2}-\d{2}").unwrap();
    let mut header_indices: Vec<usize> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if header_re.is_match(line) {
            header_indices.push(i);
        }
    }
    let n = 3;
    let start = if header_indices.len() > n {
        header_indices.len() - n
    } else {
        0
    };
    let selected = &header_indices[start..];

    if selected.is_empty() {
        out.push_str("[no previous sessions]\n");
    } else {
        for (pos, &idx) in selected.iter().enumerate() {
            if pos > 0 {
                out.push('\n');
            }
            out.push_str(lines[idx]);
            out.push('\n');

            let end = if pos + 1 < selected.len() {
                selected[pos + 1]
            } else {
                lines.len()
            };

            let block: String = lines[(idx + 1)..end]
                .iter()
                .copied()
                .collect::<Vec<_>>()
                .join("\n");
            let block = block.trim_end();
            if !block.is_empty() {
                out.push_str(block);
                out.push('\n');
            }
        }
    }

    // Template
    out.push_str(&format!(
        "\n=== TEMPLATE ===\n\
         ### {}\n\n\
         **Risk**: [no immediate concerns noted — or document if present]\n\n\
         [Session narrative in ACT/CBS process language]\n\n\
         **Formulation**: [1-2 sentences on current clinical picture and direction]\n",
        today
    ));

    Ok(out)
}

/// Append a note to the end of a client file.
fn append_note(id: &str, note: &str) -> Result<()> {
    let path = client::notes_path(id);
    append_note_to_path(&path, note)
}

/// Append a note to a specific file path.
fn append_note_to_path(path: &std::path::Path, note: &str) -> Result<()> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Could not read: {}", path.display()))?;

    // Ensure there's a blank line before the new note
    let separator = if content.ends_with("\n\n") {
        ""
    } else if content.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };

    let new_content = format!("{}{}{}\n", content, separator, note.trim_end());
    std::fs::write(path, new_content)
        .with_context(|| format!("Failed to write: {}", path.display()))?;

    Ok(())
}

/// Run `clinical note <ID> <observation>`.
pub fn run(id: &str, observation: &str, auto_confirm: bool) -> Result<()> {
    // Step 1: Pre-compute context
    eprintln!("Preparing context for {}...", id);
    let context = capture_note_prepare(id)?;

    // Step 2: Build prompt and call LLM
    let prompt = build_prompt(&context, observation);

    let llm_cmd = std::env::var("CLINICAL_LLM_CMD").unwrap_or_else(|_| "claude".to_string());
    let llm_args = std::env::var("CLINICAL_LLM_ARGS")
        .unwrap_or_else(|_| "-p --output-format text".to_string());

    let args: Vec<&str> = llm_args.split_whitespace().collect();

    eprintln!("Generating note via {}...", llm_cmd);
    let output = Command::new(&llm_cmd)
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to start LLM command: {}", llm_cmd))?;

    // Write prompt to stdin
    let mut child = output;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes())?;
    }

    let output = child.wait_with_output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("LLM command failed (exit {}): {}", output.status, stderr);
    }

    let note = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if note.is_empty() {
        bail!("LLM returned empty output");
    }

    // Step 3: Validate
    let validation = validate_note(&note);
    if !validation.is_ok() {
        eprintln!("\n⚠️  Validation warnings:");
        for err in &validation.errors {
            eprintln!("  - {}", err);
        }
        eprintln!();
    }

    // Step 4: Show note for review
    println!("\n{}", note);

    // Step 5: Confirm
    if !auto_confirm {
        eprint!("\nAppend to {}.md? [y/n] ", id);
        io::stderr().flush()?;

        let mut response = String::new();
        io::stdin().read_line(&mut response)?;

        if !response.trim().eq_ignore_ascii_case("y") {
            eprintln!("Aborted.");
            return Ok(());
        }
    }

    // Step 6: Append
    append_note(id, &note)?;
    eprintln!("Note appended to {}.md", id);

    // Step 7: Finalise
    finalise::run(id)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_note_valid() {
        let note = "\
### 2026-03-20

**Risk**: No immediate concerns noted.

Client explored workplace dynamics and values-based decision making.

**Formulation**: Continued work on distinguishing chosen action from reactive patterns.
";
        let result = validate_note(note);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_note_missing_risk() {
        let note = "\
### 2026-03-20

Client explored workplace dynamics.

**Formulation**: Continued work on patterns.
";
        let result = validate_note(note);
        assert!(!result.is_ok());
        assert!(result.errors.iter().any(|e| e.contains("Risk")));
    }

    #[test]
    fn test_validate_note_missing_formulation() {
        let note = "\
### 2026-03-20

**Risk**: No concerns.

Client explored workplace dynamics.
";
        let result = validate_note(note);
        assert!(!result.is_ok());
        assert!(result.errors.iter().any(|e| e.contains("Formulation")));
    }

    #[test]
    fn test_validate_note_missing_header() {
        let note = "\
**Risk**: No concerns.

Client explored workplace dynamics.

**Formulation**: Continued work.
";
        let result = validate_note(note);
        assert!(!result.is_ok());
        assert!(result.errors.iter().any(|e| e.contains("header")));
    }

    #[test]
    fn test_validate_note_refusal() {
        let note = "I can't generate clinical notes about this topic.";
        let result = validate_note(note);
        assert!(!result.is_ok());
        assert!(result.errors.iter().any(|e| e.contains("refusal")));
    }

    #[test]
    fn test_build_prompt_contains_context_and_observation() {
        let prompt = build_prompt("=== CONTEXT ===\nsome context", "She discussed dating");
        assert!(prompt.contains("=== CONTEXT ==="));
        assert!(prompt.contains("She discussed dating"));
        assert!(prompt.contains("ACT/CBS"));
    }

    #[test]
    fn test_append_note_formatting() {
        let tmp = tempfile::TempDir::new().unwrap();
        let md_path = tmp.path().join("TEST01.md");

        let initial = "# TEST01\n\n## Session Notes\n\n### 2026-01-15\nFirst note.\n";
        std::fs::write(&md_path, initial).unwrap();

        append_note_to_path(&md_path, "### 2026-01-22\n\nSecond note.").unwrap();

        let result = std::fs::read_to_string(&md_path).unwrap();
        assert!(result.contains("### 2026-01-15\nFirst note."));
        assert!(result.contains("### 2026-01-22\n\nSecond note."));
        // Should have blank line separator
        assert!(result.contains("First note.\n\n### 2026-01-22"));
    }
}
