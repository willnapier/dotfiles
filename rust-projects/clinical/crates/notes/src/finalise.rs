use anyhow::{Context, Result};
use regex::Regex;

use clinical_core::client;

use crate::{markdown, session};

/// Run `clinical note-finalise <ID>`.
///
/// Called after a session note has been appended to the client file.
/// Deterministically updates the session count field and prints
/// a summary with any alerts that need action.
pub fn run(id: &str) -> Result<()> {
    let path = client::notes_path(id);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read client file: {}", path.display()))?;

    let lines: Vec<&str> = content.lines().collect();

    let session_idx = match session::find_session_section(&lines) {
        Some(idx) => idx,
        None => {
            println!("No ## Session Notes section found in {}", id);
            return Ok(());
        }
    };

    let session_lines = &lines[(session_idx + 1)..];
    let total = session::count_sessions(session_lines);

    if total == 0 {
        println!("No session headers found in {}", id);
        return Ok(());
    }

    // Count DNAs
    let dna_re = Regex::new(r"(?i)^### \d{4}-\d{2}-\d{2}.*DNA").unwrap();
    let dna_count = session_lines.iter().filter(|l| dna_re.is_match(l)).count() as u32;

    let display = if dna_count > 0 {
        format!("{} (incl {} DNA)", total, dna_count)
    } else {
        total.to_string()
    };

    // Read current field value
    let existing = markdown::extract_field(&content, "Session count");

    // Check if update is needed
    let needs_update = match &existing {
        None => true,
        Some(val) => {
            if val.contains("tracking") || val.contains("estimated") {
                false
            } else {
                let num_re = Regex::new(r"(\d+)").unwrap();
                let existing_num: u32 = num_re
                    .captures(val)
                    .and_then(|c| c[1].parse().ok())
                    .unwrap_or(0);
                existing_num != total
            }
        }
    };

    if needs_update {
        let mut file_lines: Vec<String> = content.lines().map(String::from).collect();

        if existing.is_some() {
            file_lines = markdown::update_field(&file_lines, "Session count", &display);
        } else {
            file_lines = markdown::insert_field_after_last(&file_lines, "Session count", &display);
        }

        let result = file_lines.join("\n");
        std::fs::write(&path, &result)
            .with_context(|| format!("Failed to write: {}", path.display()))?;

        let old_label = match &existing {
            Some(old) => format!(" (was: {})", old),
            None => " (added)".to_string(),
        };
        println!("Session count: {}{}", display, old_label);
    } else {
        println!("Session count: {} (unchanged)", display);
    }

    // Update sessions_used in identity.yaml to stay in sync
    let identity_path = client::identity_path(id);
    if identity_path.exists() {
        if let Ok(identity) = std::fs::read_to_string(&identity_path) {
            let re = Regex::new(r"(?m)^(\s*sessions_used:\s*)(\d+|null)").unwrap();
            if re.is_match(&identity) {
                let updated = re.replace(&identity, |caps: &regex::Captures| {
                    format!("{}{}", &caps[1], total)
                }).to_string();
                if updated != identity {
                    let _ = std::fs::write(&identity_path, &updated);
                    eprintln!("Updated sessions_used → {} in identity.yaml", total);
                }
            }
        }
    }

    // Re-check alerts now that the note is written
    let auth_status = session::compute_auth_status(id, &content);

    let last_letter_raw =
        markdown::extract_field(&content, "Last update letter").unwrap_or_default();
    let last_letter = if last_letter_raw == "none yet"
        || last_letter_raw == "null"
        || last_letter_raw.is_empty()
    {
        String::new()
    } else {
        last_letter_raw
    };

    let letter_status = session::compute_letter_status(total, &last_letter, session_lines);

    let referral_type = markdown::extract_field(&content, "Referral type");
    let referring_doctor = markdown::extract_field(&content, "Referring doctor");
    let next_specialist = markdown::extract_field(&content, "Next specialist appointment");
    let current_specialist = markdown::extract_field(&content, "Current specialist");

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
                total
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
        alerts.push("\u{2753} Referral type not set — ask William".to_string());
    }

    if !alerts.is_empty() {
        println!();
        for alert in &alerts {
            println!("{}", alert);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn setup_client(id: &str, content: &str) -> TempDir {
        let tmp = TempDir::new().unwrap();
        let clients_dir = tmp.path().join("clients").join(id);
        std::fs::create_dir_all(&clients_dir).unwrap();
        // Route C layout: notes.md (no private/ dir)
        let md_path = clients_dir.join("notes.md");
        let mut f = std::fs::File::create(&md_path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        tmp
    }

    #[test]
    fn test_finalise_increments_count() {
        let content = "\
# TEST01

**Session count**: 2

## Session Notes

### 2026-01-15
Notes.

### 2026-01-22
More notes.

### 2026-01-29
New session just appended.
";
        let tmp = setup_client("TEST01", content);
        std::env::set_var("CLINICAL_ROOT", tmp.path());

        run("TEST01").unwrap();

        let result = std::fs::read_to_string(
            tmp.path().join("clients/TEST01/notes.md"),
        )
        .unwrap();
        assert!(result.contains("**Session count**: 3"));

        std::env::remove_var("CLINICAL_ROOT");
    }

    #[test]
    fn test_finalise_adds_missing_count() {
        let content = "\
# TEST02

**Funding**: Self-pay

## Session Notes

### 2026-01-15
Notes.

### 2026-01-22
More notes.
";
        let tmp = setup_client("TEST02", content);
        std::env::set_var("CLINICAL_ROOT", tmp.path());

        run("TEST02").unwrap();

        let result = std::fs::read_to_string(
            tmp.path().join("clients/TEST02/notes.md"),
        )
        .unwrap();
        assert!(result.contains("**Session count**: 2"));

        std::env::remove_var("CLINICAL_ROOT");
    }

    #[test]
    fn test_finalise_no_change_when_correct() {
        let content = "\
# TEST03

**Session count**: 2

## Session Notes

### 2026-01-15
Notes.

### 2026-01-22
More notes.
";
        let tmp = setup_client("TEST03", content);
        std::env::set_var("CLINICAL_ROOT", tmp.path());

        run("TEST03").unwrap();

        let result = std::fs::read_to_string(
            tmp.path().join("clients/TEST03/notes.md"),
        )
        .unwrap();
        assert!(result.contains("**Session count**: 2"));

        std::env::remove_var("CLINICAL_ROOT");
    }

    #[test]
    fn test_finalise_preserves_tracking() {
        let content = "\
# TEST04

**Session count**: tracking from 2025-06-01

## Session Notes

### 2026-01-15
Notes.
";
        let tmp = setup_client("TEST04", content);
        std::env::set_var("CLINICAL_ROOT", tmp.path());

        run("TEST04").unwrap();

        let result = std::fs::read_to_string(
            tmp.path().join("clients/TEST04/notes.md"),
        )
        .unwrap();
        assert!(result.contains("**Session count**: tracking from 2025-06-01"));

        std::env::remove_var("CLINICAL_ROOT");
    }

    #[test]
    fn test_finalise_includes_dna_label() {
        let content = "\
# TEST05

**Session count**: 2

## Session Notes

### 2026-01-15
Notes.

### 2026-01-22 DNA

### 2026-01-29
New session.
";
        let tmp = setup_client("TEST05", content);
        std::env::set_var("CLINICAL_ROOT", tmp.path());

        run("TEST05").unwrap();

        let result = std::fs::read_to_string(
            tmp.path().join("clients/TEST05/notes.md"),
        )
        .unwrap();
        assert!(result.contains("**Session count**: 3 (incl 1 DNA)"));

        std::env::remove_var("CLINICAL_ROOT");
    }
}
