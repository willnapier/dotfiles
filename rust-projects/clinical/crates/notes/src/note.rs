use anyhow::{bail, Context, Result};
use regex::Regex;
use std::io::{self, Read, Write};
use std::process::Command;

use clinical_core::client;

use crate::{finalise, session};

/// Validation errors for LLM-generated notes.
pub struct ValidationResult {
    pub errors: Vec<String>,
}

impl ValidationResult {
    fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Validate that a generated note has the required structure.
pub fn validate_note(note: &str) -> ValidationResult {
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

/// Resolve the clinical reference directory.
///
/// Checks CLINICAL_NOTES_SKILL_DIR env var, then falls back to
/// ~/.claude/skills/clinical-notes/
fn skill_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("CLINICAL_NOTES_SKILL_DIR") {
        std::path::PathBuf::from(dir)
    } else {
        dirs::home_dir()
            .expect("Could not find home directory")
            .join(".claude/skills/clinical-notes")
    }
}

/// Load a reference file if it exists, returning empty string if not.
fn load_reference(filename: &str) -> String {
    let path = skill_dir().join(filename);
    std::fs::read_to_string(&path).unwrap_or_default()
}

/// Find correspondence files in the client directory (letters, reports).
fn find_correspondence(id: &str) -> Vec<(String, String)> {
    let dir = client::client_dir(id);
    let mut files = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&dir) {
        let corr_re = Regex::new(r"^\d{4}-\d{2}-\d{2}-.+\.(md|txt)$").unwrap();
        let mut paths: Vec<std::path::PathBuf> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                corr_re.is_match(&name) && name != format!("{}.md", id)
            })
            .map(|e| e.path())
            .collect();
        paths.sort();

        for path in paths {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if let Ok(content) = std::fs::read_to_string(&path) {
                files.push((name, content));
            }
        }
    }

    files
}

/// Build the full prompt for the LLM, including all available context.
fn build_prompt(id: &str, observation: &str) -> Result<String> {
    let path = client::notes_path(id);
    let client_file = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read client file: {}", path.display()))?;

    let lines: Vec<&str> = client_file.lines().collect();
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    // Compute deterministic metadata
    let session_idx = session::find_session_section(&lines).unwrap_or(0);
    let session_lines = &lines[(session_idx + 1)..];
    let total_sessions = session::count_sessions(session_lines);
    let new_session_number = total_sessions + 1;

    // Auth status
    let auth_status = session::compute_auth_status(id, &client_file);
    let auth_markers = session::parse_auth_markers(&lines);

    let mut out = String::new();

    // Clinical reference material
    let philosophy = load_reference("CLINICAL-PHILOSOPHY.md");
    let reference = load_reference("CLINICAL-REFERENCE.md");

    if !philosophy.is_empty() {
        out.push_str("=== CLINICAL PHILOSOPHY ===\n");
        out.push_str(&philosophy);
        out.push_str("\n\n");
    }

    if !reference.is_empty() {
        out.push_str("=== CLINICAL REFERENCE ===\n");
        out.push_str(&reference);
        out.push_str("\n\n");
    }

    // Full client file
    out.push_str(&format!("=== CLIENT FILE: {} ===\n", id));
    out.push_str(&client_file);
    out.push_str("\n\n");

    // Correspondence
    let correspondence = find_correspondence(id);
    if !correspondence.is_empty() {
        out.push_str("=== CORRESPONDENCE ===\n");
        for (name, content) in &correspondence {
            out.push_str(&format!("--- {} ---\n", name));
            out.push_str(content);
            out.push_str("\n\n");
        }
    }

    // Deterministic metadata
    out.push_str(&format!("=== SESSION METADATA ===\n"));
    out.push_str(&format!("Date: {}\n", today));
    out.push_str(&format!("Session number: {}\n", new_session_number));

    if let Some(ref auth) = auth_status {
        let auth_date = auth_markers
            .last()
            .map(|m| m.auth_date.as_str())
            .unwrap_or("unknown");
        out.push_str(&format!(
            "Auth: {} of {} used (since {}), {} remaining\n",
            auth.sessions_used, auth.sessions_authorised, auth_date, auth.remaining
        ));
    }

    // Instruction
    out.push_str(&format!(
        "\n=== INSTRUCTION ===\n\
         You are a clinical documentation assistant for a Chartered Psychologist (BPS).\n\
         You have the clinician's full therapeutic framework and the complete client file above.\n\
         Write a session note for session {} on {} translating the observation below \
         into ACT/CBS process language.\n\
         Draw on the full therapeutic arc — reference previous sessions, ongoing themes, \
         and the client's formulation where relevant.\n\
         Use the clinician's voice and framework from the reference material.\n\
         Include **Risk**: and **Formulation**: lines.\n\
         Output ONLY the session note (starting with ### {}), no preamble or explanation.\n\n\
         === OBSERVATION ===\n\
         {}\n",
        new_session_number, today, today, observation
    ));

    Ok(out)
}

/// Append a note to the end of a client file.
pub fn append_note(id: &str, note: &str) -> Result<()> {
    let path = client::notes_path(id);
    append_note_to_path(&path, note)
}

/// Append a note to a specific file path.
pub fn append_note_to_path(path: &std::path::Path, note: &str) -> Result<()> {
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

/// Run `clinical note-save <ID>`.
///
/// Reads a pre-drafted note from stdin, validates it, appends to the
/// client file, and runs finalise. This is the deterministic save path
/// called after the LLM has drafted a note and the clinician has approved it.
pub fn save(id: &str) -> Result<()> {
    let mut note = String::new();
    io::stdin()
        .read_to_string(&mut note)
        .context("Failed to read note from stdin")?;

    let note = note.trim().to_string();
    if note.is_empty() {
        bail!("Empty note on stdin");
    }

    // Validate
    let validation = validate_note(&note);
    if !validation.is_ok() {
        eprintln!("Validation errors:");
        for err in &validation.errors {
            eprintln!("  - {}", err);
        }
        bail!("Note failed validation — not saved");
    }

    // Append
    let path = client::notes_path(id);
    if !path.exists() {
        bail!("Client file not found: {}", path.display());
    }

    append_note(id, &note)?;

    let line_count = note.lines().count();
    eprintln!("Saved to {}.md ({} lines appended)", id, line_count);

    // Finalise (session count + alerts)
    finalise::run(id)?;

    Ok(())
}

/// Run `clinical note <ID> <observation>`.
pub fn run(id: &str, observation: &str, auto_confirm: bool) -> Result<()> {
    // Step 1: Build full context prompt
    eprintln!("Preparing context for {}...", id);
    let prompt = build_prompt(id, observation)?;

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
    fn test_validate_note_valid_all_fields() {
        let note = "\
### 2026-03-26

**Risk**: No immediate concerns noted.

Client engaged in values clarification work around career transition.

**Formulation**: Increasing flexibility in responding to uncertainty; moving from avoidance to approach.
";
        let result = validate_note(note);
        assert!(result.is_ok());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_validate_note_refusal() {
        let note = "I can't generate clinical notes about this topic.";
        let result = validate_note(note);
        assert!(!result.is_ok());
        assert!(result.errors.iter().any(|e| e.contains("refusal")));
    }

    #[test]
    fn test_build_prompt_contains_observation_and_instruction() {
        let tmp = tempfile::TempDir::new().unwrap();
        let clients_dir = tmp.path().join("clients").join("TEST01");
        std::fs::create_dir_all(&clients_dir).unwrap();
        let md_path = clients_dir.join("TEST01.md");
        std::fs::write(
            &md_path,
            "# TEST01\n\n## Session Notes\n\n### 2026-01-15\nFirst note.\n",
        )
        .unwrap();

        std::env::set_var("CLINICAL_ROOT", tmp.path());
        std::env::set_var("CLINICAL_NOTES_SKILL_DIR", "/nonexistent");

        let prompt = build_prompt("TEST01", "She discussed dating").unwrap();
        assert!(prompt.contains("She discussed dating"));
        assert!(prompt.contains("ACT/CBS"));
        assert!(prompt.contains("# TEST01"));
        assert!(prompt.contains("First note."));

        std::env::remove_var("CLINICAL_ROOT");
        std::env::remove_var("CLINICAL_NOTES_SKILL_DIR");
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
