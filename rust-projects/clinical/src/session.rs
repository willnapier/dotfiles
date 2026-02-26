use anyhow::{Context, Result};
use regex::Regex;
use std::path::Path;

use crate::markdown;

/// Parsed auth marker: `#### Auth: N sessions from YYYY-MM-DD`
#[derive(Debug, Clone)]
pub struct AuthMarker {
    pub sessions_authorised: u32,
    pub auth_date: String,
    /// Line index (0-based) where this marker appears.
    pub line_index: usize,
}

/// Computed auth status for a single client.
#[derive(Debug)]
pub struct AuthStatus {
    pub client_id: String,
    pub funder: String,
    pub sessions_used: u32,
    pub sessions_authorised: u32,
    pub remaining: i32,
    pub total_sessions: u32,
    pub letter_status: String,
    pub therapy_commenced: String,
    pub funding_label: String,
}

/// Parse all auth markers from lines of a client .md file.
pub fn parse_auth_markers(lines: &[&str]) -> Vec<AuthMarker> {
    let re = Regex::new(r"^#### Auth: (\d+) sessions from (\d{4}-\d{2}-\d{2})").unwrap();
    let mut markers = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if let Some(caps) = re.captures(line) {
            markers.push(AuthMarker {
                sessions_authorised: caps[1].parse().unwrap_or(0),
                auth_date: caps[2].to_string(),
                line_index: i,
            });
        }
    }

    markers
}

/// Count session headers (`### YYYY-MM-DD`) in a slice of lines.
/// Also counts DNA sessions (`### YYYY-MM-DD DNA`).
pub fn count_sessions(lines: &[&str]) -> u32 {
    let re = Regex::new(r"^### \d{4}-\d{2}-\d{2}").unwrap();
    lines.iter().filter(|l| re.is_match(l)).count() as u32
}

/// Find the index of the `## Session Notes` (or `## Session`) header.
pub fn find_session_section(lines: &[&str]) -> Option<usize> {
    let re = Regex::new(r"^## Session").unwrap();
    lines.iter().position(|l| re.is_match(l))
}

/// Compute the full auth status for a client file.
pub fn compute_auth_status(client_id: &str, content: &str) -> Option<AuthStatus> {
    let lines: Vec<&str> = content.lines().collect();

    let markers = parse_auth_markers(&lines);
    if markers.is_empty() {
        return None;
    }

    let last_auth = markers.last().unwrap();
    let after_auth = &lines[(last_auth.line_index + 1)..];
    let sessions_used = count_sessions(after_auth);
    let remaining = last_auth.sessions_authorised as i32 - sessions_used as i32;

    // Total sessions (from ## Session Notes section onwards)
    let session_section_idx = find_session_section(&lines).unwrap_or(0);
    let all_session_lines = &lines[(session_section_idx + 1)..];
    let total_sessions = count_sessions(all_session_lines);

    // Funder label from **Funding**: line
    let funder = markdown::extract_field(content, "Funding").unwrap_or_else(|| "unknown".to_string());

    // Therapy commenced
    let therapy_commenced =
        markdown::extract_field(content, "Therapy commenced").unwrap_or_default();

    // Last update letter
    let last_letter_raw =
        markdown::extract_field(content, "Last update letter").unwrap_or_default();
    let last_letter = if last_letter_raw == "none yet"
        || last_letter_raw == "null"
        || last_letter_raw.is_empty()
    {
        String::new()
    } else {
        last_letter_raw
    };

    // Update letter status
    let letter_status = compute_letter_status(total_sessions, &last_letter, all_session_lines);

    Some(AuthStatus {
        client_id: client_id.to_string(),
        funder: funder.clone(),
        sessions_used,
        sessions_authorised: last_auth.sessions_authorised,
        remaining,
        total_sessions,
        letter_status,
        therapy_commenced,
        funding_label: funder,
    })
}

/// Determine update letter status.
///
/// Rule: due at session 2, then every 6 sessions after last letter.
fn compute_letter_status(total_sessions: u32, last_letter: &str, session_lines: &[&str]) -> String {
    if total_sessions < 2 {
        return String::new();
    }

    if last_letter.is_empty() {
        return format!(
            "update letter due - session {}, no letter sent",
            total_sessions
        );
    }

    // Count sessions after last letter date
    let re = Regex::new(r"^### (\d{4}-\d{2}-\d{2})").unwrap();
    let sessions_since = session_lines
        .iter()
        .filter(|l| {
            if let Some(caps) = re.captures(l) {
                let date = &caps[1];
                date > last_letter
            } else {
                false
            }
        })
        .count();

    if sessions_since >= 6 {
        format!("{} sessions since last letter - update due", sessions_since)
    } else {
        String::new()
    }
}

/// Build the sessions info string for auth letter drafts.
pub fn sessions_info_string(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let markers = parse_auth_markers(&lines);

    if markers.is_empty() {
        // No auth markers — just count total
        let session_idx = find_session_section(&lines).unwrap_or(0);
        let count = count_sessions(&lines[(session_idx + 1)..]);
        format!("{} sessions to date", count)
    } else {
        let last_auth = markers.last().unwrap();
        let after_auth = &lines[(last_auth.line_index + 1)..];
        let used = count_sessions(after_auth);
        format!(
            "{} of {} authorised sessions used since {}",
            used, last_auth.sessions_authorised, last_auth.auth_date
        )
    }
}

/// Find all client .md files (where filename matches parent directory name).
pub fn find_client_md_files(clients_dir: &Path) -> Result<Vec<(String, std::path::PathBuf)>> {
    let mut results = Vec::new();

    if !clients_dir.exists() {
        return Ok(results);
    }

    let entries: Vec<_> = std::fs::read_dir(clients_dir)
        .with_context(|| format!("Failed to read: {}", clients_dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .collect();

    for entry in entries {
        let dir_name = entry.file_name().to_string_lossy().to_string();
        let md_file = entry.path().join(format!("{}.md", dir_name));
        if md_file.exists() {
            results.push((dir_name, md_file));
        }
    }

    results.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_MD: &str = "\
# EB88

**Referral**: Dr Smith
**Therapy commenced**: July 2023
**Funding**: Employer (insurer-administered)
**Last update letter**: none yet

## Presenting Difficulties

## Formulation

## Session Notes

#### Auth: 10 sessions from 2026-01-15

### 2026-01-27
Session notes here.

### 2026-02-03
More notes.

### 2026-02-17
Even more notes.

### 2026-02-24
Latest session.
";

    #[test]
    fn test_parse_auth_markers() {
        let lines: Vec<&str> = SAMPLE_MD.lines().collect();
        let markers = parse_auth_markers(&lines);
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].sessions_authorised, 10);
        assert_eq!(markers[0].auth_date, "2026-01-15");
    }

    #[test]
    fn test_count_sessions_after_auth() {
        let lines: Vec<&str> = SAMPLE_MD.lines().collect();
        let markers = parse_auth_markers(&lines);
        let after = &lines[(markers[0].line_index + 1)..];
        assert_eq!(count_sessions(after), 4);
    }

    #[test]
    fn test_compute_auth_status() {
        let status = compute_auth_status("EB88", SAMPLE_MD).unwrap();
        assert_eq!(status.client_id, "EB88");
        assert_eq!(status.sessions_used, 4);
        assert_eq!(status.sessions_authorised, 10);
        assert_eq!(status.remaining, 6);
        assert_eq!(status.total_sessions, 4);
        assert_eq!(status.funder, "Employer (insurer-administered)");
    }

    #[test]
    fn test_no_auth_markers_returns_none() {
        let content = "# TEST01\n\n## Session Notes\n\n### 2026-01-15\nNotes.\n";
        assert!(compute_auth_status("TEST01", content).is_none());
    }

    #[test]
    fn test_sessions_info_with_auth() {
        let info = sessions_info_string(SAMPLE_MD);
        assert_eq!(info, "4 of 10 authorised sessions used since 2026-01-15");
    }

    #[test]
    fn test_sessions_info_without_auth() {
        let content = "# TEST\n\n## Session Notes\n\n### 2026-01-15\n\n### 2026-01-22\n";
        let info = sessions_info_string(content);
        assert_eq!(info, "2 sessions to date");
    }

    #[test]
    fn test_update_letter_due_no_letter_sent() {
        let status = compute_auth_status("EB88", SAMPLE_MD).unwrap();
        assert!(status.letter_status.contains("no letter sent"));
    }

    #[test]
    fn test_update_letter_not_due_when_recent() {
        let content = "\
# TEST

**Funding**: AXA
**Last update letter**: 2026-02-20

## Session Notes

#### Auth: 10 sessions from 2026-01-01

### 2026-01-10

### 2026-01-17

### 2026-02-21
";
        let status = compute_auth_status("TEST", content).unwrap();
        assert_eq!(status.letter_status, "");
    }

    #[test]
    fn test_count_sessions_includes_dna() {
        let lines = vec!["### 2026-01-15", "notes", "### 2026-01-22 DNA", "### 2026-01-29"];
        assert_eq!(count_sessions(&lines), 3);
    }

    #[test]
    fn test_multiple_auth_markers_uses_last() {
        let content = "\
# TEST

**Funding**: BUPA

## Session Notes

#### Auth: 6 sessions from 2025-06-01

### 2025-06-10
### 2025-06-17
### 2025-06-24
### 2025-07-01
### 2025-07-08
### 2025-07-15

#### Auth: 10 sessions from 2025-08-01

### 2025-08-10
### 2025-08-17
";
        let status = compute_auth_status("TEST", content).unwrap();
        assert_eq!(status.sessions_authorised, 10);
        assert_eq!(status.sessions_used, 2);
        assert_eq!(status.remaining, 8);
        // Total includes all sessions
        assert_eq!(status.total_sessions, 8);
    }
}
