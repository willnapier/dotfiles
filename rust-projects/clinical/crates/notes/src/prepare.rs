use anyhow::{Context, Result};
use regex::Regex;

use clinical_core::client;

use crate::{markdown, session};

/// Extract the last N session blocks from content.
///
/// Each block runs from `### YYYY-MM-DD` to the next `###` header or EOF.
/// Returns vec of (date_line, block_text) most recent last.
fn extract_last_n_sessions(content: &str, n: usize) -> Vec<(String, String)> {
    let header_re = Regex::new(r"^### \d{4}-\d{2}-\d{2}").unwrap();
    let lines: Vec<&str> = content.lines().collect();

    // Find all session header indices
    let mut header_indices: Vec<usize> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if header_re.is_match(line) {
            header_indices.push(i);
        }
    }

    // Take the last N
    let start = if header_indices.len() > n {
        header_indices.len() - n
    } else {
        0
    };
    let selected = &header_indices[start..];

    let mut results = Vec::new();
    for (pos, &idx) in selected.iter().enumerate() {
        let date_line = lines[idx].to_string();

        // Block runs from header+1 to next header or EOF
        let end = if pos + 1 < selected.len() {
            selected[pos + 1]
        } else if let Some(&next) = header_indices.get(start + pos + 1) {
            next
        } else {
            lines.len()
        };

        let block: Vec<&str> = lines[(idx + 1)..end]
            .iter()
            .copied()
            .collect();

        // Trim trailing blank lines
        let trimmed = block
            .into_iter()
            .collect::<Vec<_>>()
            .join("\n");
        let trimmed = trimmed.trim_end().to_string();

        results.push((date_line, trimmed));
    }

    results
}

/// Count DNA sessions specifically (### YYYY-MM-DD DNA).
fn count_dna_sessions(content: &str) -> u32 {
    let re = Regex::new(r"^### \d{4}-\d{2}-\d{2} DNA").unwrap();
    content.lines().filter(|l| re.is_match(l)).count() as u32
}

/// Generate the universal session note template.
///
/// Every note gets the same structure: Risk + narrative + Formulation.
/// Mode distinctions (self-pay vs insurer) only matter at the letter stage.
fn template_skeleton(date: &str) -> String {
    format!(
        "### {date}\n\n\
         **Risk**: [no immediate concerns noted — or document if present]\n\n\
         [Session narrative in ACT/CBS process language]\n\n\
         **Formulation**: [1-2 sentences on current clinical picture and direction]"
    )
}

pub fn run(id: &str, sessions_context: usize) -> Result<()> {
    let path = client::notes_path(id);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read client file: {}", path.display()))?;

    let lines: Vec<&str> = content.lines().collect();

    // Extract reference fields
    let funding = markdown::extract_field(&content, "Funding").unwrap_or_default();
    let referral_type = markdown::extract_field(&content, "Referral type");
    let referring_doctor = markdown::extract_field(&content, "Referring doctor");
    let therapy_commenced = markdown::extract_field(&content, "Therapy commenced");
    let last_letter_raw = markdown::extract_field(&content, "Last update letter").unwrap_or_default();
    let next_specialist = markdown::extract_field(&content, "Next specialist appointment");
    let current_specialist = markdown::extract_field(&content, "Current specialist");

    // Session count
    let session_idx = session::find_session_section(&lines).unwrap_or(0);
    let session_lines = &lines[(session_idx + 1)..];
    let total_sessions = session::count_sessions(session_lines);
    let new_session_number = total_sessions + 1;
    let dna_count = count_dna_sessions(&content);

    // Today's date
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    // Auth status
    let auth_status = session::compute_auth_status(id, &content);
    let auth_markers = session::parse_auth_markers(&lines);

    // Letter status (for all clients, not just insurer)
    let last_letter = if last_letter_raw == "none yet" || last_letter_raw == "null" || last_letter_raw.is_empty() {
        String::new()
    } else {
        last_letter_raw.clone()
    };
    let letter_status = session::compute_letter_status(new_session_number, &last_letter, session_lines);

    // === Build output ===

    // Header
    println!("=== CLINICAL NOTE CONTEXT: {} ===", id);
    println!("Date: {}", today);
    println!("Session number: {}", new_session_number);
    if dna_count > 0 {
        println!("DNA sessions: {}", dna_count);
    }
    if let Some(ref tc) = therapy_commenced {
        println!("Therapy commenced: {}", tc);
    }
    if !funding.is_empty() {
        println!("Funding: {}", funding);
    }
    if let Some(ref rt) = referral_type {
        println!("Referral type: {}", rt);
    }
    if let Some(ref rd) = referring_doctor {
        println!("Referring doctor: {}", rd);
    }

    // Auth status section
    println!();
    println!("=== AUTH STATUS ===");
    if let Some(ref auth) = auth_status {
        let auth_date = auth_markers
            .last()
            .map(|m| m.auth_date.as_str())
            .unwrap_or("unknown");
        println!(
            "{} of {} authorised sessions used (since {})",
            auth.sessions_used, auth.sessions_authorised, auth_date
        );
        println!("Remaining: {}", auth.remaining);
    } else {
        println!("N/A (not insurer-funded)");
    }

    // Alerts
    println!();
    println!("=== ALERTS ===");
    let mut alerts: Vec<String> = Vec::new();

    // Auth running low
    if let Some(ref auth) = auth_status {
        if auth.remaining <= 2 {
            alerts.push(format!(
                "\u{26a0}\u{fe0f} Auth letter needed — {} sessions remaining",
                auth.remaining
            ));
        }
    }

    // Update letter due (doctor-referred, session count trigger)
    if let Some(ref rt) = referral_type {
        let rt_lower = rt.to_lowercase();
        if rt_lower == "doctor" {
            if !letter_status.is_empty() {
                alerts.push(format!(
                    "\u{1f4cb} Update letter due — session {}",
                    new_session_number
                ));
            }
        }
    }

    // Specialist appointment within 2 weeks
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

    // Missing referral type
    if referral_type.is_none() {
        alerts.push("\u{2753} Referral type not set — ask William".to_string());
    }

    if alerts.is_empty() {
        println!("[none]");
    } else {
        for alert in &alerts {
            println!("{}", alert);
        }
    }

    // Recent sessions
    println!();
    println!("=== RECENT SESSIONS (last {}) ===", sessions_context);
    let recent = extract_last_n_sessions(&content, sessions_context);
    if recent.is_empty() {
        println!("[no previous sessions]");
    } else {
        for (i, (date_line, block)) in recent.iter().enumerate() {
            if i > 0 {
                println!();
            }
            println!("{}", date_line);
            if !block.is_empty() {
                println!("{}", block);
            }
        }
    }

    // Template
    println!();
    println!("=== TEMPLATE ===");
    println!("{}", template_skeleton(&today));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_SELF_PAY: &str = "\
# CT71

**Therapy commenced**: 4 October 2023
**Referral type**: doctor
**Referring doctor**: Dr Loughlin, GP, Example Surgery
**Funding**: Employer direct (Partner)
**Session count**: 14
**Last update letter**: 2026-01-15

## Presenting Difficulties

Workplace stress and attachment patterns.

## Formulation

ACT-informed formulation here.

## Session Notes

### 2026-02-20
Session focused on NVC practice and workplace boundaries.

### 2026-02-27
Explored attachment patterns in the context of new relationship.

### 2026-03-06
Client discussed workplace dynamics and values-based decision making.
";

    const SAMPLE_INSURER: &str = "\
# AB99

**Therapy commenced**: January 2026
**Referral type**: doctor
**Referring doctor**: Dr Smith, Psychiatrist
**Funding**: AXA (via GP)
**Session count**: 8
**Last update letter**: 2026-02-01

## Session Notes

#### Auth: 10 sessions from 2026-01-01

### 2026-01-10
First session notes.

### 2026-01-17
Second session.

### 2026-01-24
Third session.

### 2026-01-31
Fourth session.

### 2026-02-07
Fifth session.

### 2026-02-14
Sixth session.

### 2026-02-21
Seventh session.

### 2026-02-28
Eighth session.
";

    #[test]
    fn test_extract_last_n_sessions_returns_last_3() {
        let sessions = extract_last_n_sessions(SAMPLE_SELF_PAY, 3);
        assert_eq!(sessions.len(), 3);
        assert!(sessions[0].0.contains("2026-02-20"));
        assert!(sessions[1].0.contains("2026-02-27"));
        assert!(sessions[2].0.contains("2026-03-06"));
    }

    #[test]
    fn test_extract_sessions_includes_block_text() {
        let sessions = extract_last_n_sessions(SAMPLE_SELF_PAY, 1);
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].1.contains("workplace dynamics"));
    }

    #[test]
    fn test_extract_more_than_available() {
        let sessions = extract_last_n_sessions(SAMPLE_SELF_PAY, 10);
        assert_eq!(sessions.len(), 3); // only 3 exist
    }

    #[test]
    fn test_count_dna_sessions() {
        let content = "## Session Notes\n\n### 2026-01-15\nNotes.\n\n### 2026-01-22 DNA\n\n### 2026-01-29\nMore notes.\n";
        assert_eq!(count_dna_sessions(content), 1);
    }

    #[test]
    fn test_count_dna_sessions_none() {
        assert_eq!(count_dna_sessions(SAMPLE_SELF_PAY), 0);
    }

    #[test]
    fn test_template_universal() {
        let t = template_skeleton("2026-03-20");
        assert!(t.starts_with("### 2026-03-20"));
        assert!(t.contains("Risk"));
        assert!(t.contains("Formulation"));
        assert!(t.contains("ACT/CBS"));
    }
}
