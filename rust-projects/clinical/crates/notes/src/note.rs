use anyhow::{bail, Context, Result};
use regex::Regex;
use std::io::{self, Read, Write};
use std::process::Command;

use clinical_core::client;

use crate::{finalise, session};

/// Validation errors for LLM-generated notes.
pub struct ValidationResult {
    /// Soft warnings — displayed but don't block.
    pub warnings: Vec<String>,
    /// Hard failures — block acceptance and trigger regeneration.
    pub failures: Vec<String>,
}

impl ValidationResult {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Validate that a generated note has the required structure.
pub fn validate_note(note: &str) -> ValidationResult {
    let mut warnings = Vec::new();
    let mut failures = Vec::new();

    // --- Structural warnings (soft) ---

    let date_re = Regex::new(r"^### \d{4}-\d{2}-\d{2}").unwrap();
    if !note.lines().any(|l| date_re.is_match(l)) {
        warnings.push("Missing session header (### YYYY-MM-DD)".to_string());
    }

    if !note.contains("**Risk**:") {
        warnings.push("Missing **Risk**: line".to_string());
    }

    if !note.contains("**Formulation**:") {
        warnings.push("Missing **Formulation**: line".to_string());
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
            warnings.push(format!("Possible LLM refusal detected: \"{}\"", pattern));
            break;
        }
    }

    // --- Lint gates (hard block — triggers regeneration) ---

    let collab_re = Regex::new(r"(?i)collaborat\w*").unwrap();
    let agreed_re = Regex::new(r"(?i)\bagree[ds]?\b").unwrap();
    for sentence in note.split(|c| c == '.' || c == '\n') {
        if collab_re.is_match(sentence) && agreed_re.is_match(sentence) {
            failures.push(format!(
                "Redundancy: 'collaborative' + 'agreed' in same sentence: \"{}\"",
                sentence.trim()
            ));
        }
    }

    ValidationResult { warnings, failures }
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

/// Extract the last N session notes from the client file.
/// Sessions are delimited by `### YYYY-MM-DD` headers.
fn extract_last_n_sessions(client_file: &str, n: usize) -> String {
    let date_re = Regex::new(r"^### \d{4}-\d{2}-\d{2}").unwrap();
    let mut session_starts: Vec<usize> = Vec::new();
    let lines: Vec<&str> = client_file.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        if date_re.is_match(line) {
            session_starts.push(i);
        }
    }

    if session_starts.is_empty() {
        return String::new();
    }

    let start_from = if session_starts.len() > n {
        session_starts[session_starts.len() - n]
    } else {
        session_starts[0]
    };

    lines[start_from..].join("\n")
}

/// Find correspondence files in the client directory (letters, reports).
fn find_correspondence(id: &str) -> Vec<(String, String)> {
    let mut files = Vec::new();
    let corr_re = Regex::new(r"^\d{4}-\d{2}-\d{2}-.+\.(md|txt)$").unwrap();

    // Determine search directories based on layout.
    let layout = client::detect_layout(id);
    let search_dirs: Vec<std::path::PathBuf> = match layout {
        client::Layout::RouteC => {
            // Route C: correspondence lives in correspondence/ subdir
            vec![client::correspondence_dir(id)]
        }
        client::Layout::RouteA => {
            // Route A: date-prefixed files in client root
            vec![client::client_dir(id)]
        }
    };

    for dir in &search_dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
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
    }

    files
}

/// Load the client context string (summary + recent sessions, or full file).
/// Extracted from build_prompt for reuse by the faithfulness checker.
fn load_client_context(id: &str) -> Result<String> {
    let path = client::notes_path(id);
    let client_file = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read client file: {}", path.display()))?;

    let mut out = String::new();
    let summary_path = client::client_dir(id).join("summary.md");
    if summary_path.exists() {
        let summary = std::fs::read_to_string(&summary_path).unwrap_or_default();
        out.push_str(&summary);
        out.push('\n');
        let last_sessions = extract_last_n_sessions(&client_file, 3);
        if !last_sessions.is_empty() {
            out.push_str(&last_sessions);
            out.push('\n');
        }
    } else {
        out.push_str(&client_file);
        out.push('\n');
        let correspondence = find_correspondence(id);
        for (_, content) in &correspondence {
            out.push_str(content);
            out.push('\n');
        }
    }
    Ok(out)
}

/// Public wrapper for load_client_context — used by batch processing.
pub fn load_client_context_public(id: &str) -> Result<String> {
    load_client_context(id)
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

    // Modality prompt (practitioner-owned, therapeutic model specific)
    let modality = load_reference("modality-act.md");
    if !modality.is_empty() {
        out.push_str("=== THERAPEUTIC FRAMEWORK ===\n");
        out.push_str(&modality);
        out.push_str("\n\n");
    }

    // Faithfulness prompt (universal, Karpathy-loop optimised)
    let faithfulness = load_reference("faithfulness-prompt.md");
    if !faithfulness.is_empty() {
        out.push_str("=== FAITHFULNESS RULES ===\n");
        out.push_str(&faithfulness);
        out.push_str("\n\n");
    }

    // Client context: prefer summary.md + last 3 sessions over full file
    let summary_path = client::client_dir(id).join("summary.md");
    if summary_path.exists() {
        let summary = std::fs::read_to_string(&summary_path).unwrap_or_default();
        out.push_str(&format!("=== CLIENT SUMMARY: {} ===\n", id));
        out.push_str(&summary);
        out.push_str("\n\n");

        // Last 3 sessions verbatim for recent continuity
        let last_sessions = extract_last_n_sessions(&client_file, 3);
        if !last_sessions.is_empty() {
            out.push_str("=== RECENT SESSIONS ===\n");
            out.push_str(&last_sessions);
            out.push_str("\n\n");
        }
    } else {
        // No summary yet — fall back to full client file
        out.push_str(&format!("=== CLIENT FILE: {} ===\n", id));
        out.push_str(&client_file);
        out.push_str("\n\n");

        // Full correspondence (only when no summary — summary captures the gist)
        let correspondence = find_correspondence(id);
        if !correspondence.is_empty() {
            out.push_str("=== CORRESPONDENCE ===\n");
            for (name, content) in &correspondence {
                out.push_str(&format!("--- {} ---\n", name));
                out.push_str(content);
                out.push_str("\n\n");
            }
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

    // Prompt-Rail: extract grounding constraints from the observation
    // to prevent confabulation upstream (cheaper than catching it downstream).
    let client_context = load_client_context(id).unwrap_or_default();
    let rail = crate::faithfulness::prompt_rail(observation, &client_context);
    if !rail.is_empty() {
        out.push_str(&rail);
        out.push('\n');
    }

    // Instruction
    out.push_str(&format!(
        "\n=== INSTRUCTION ===\n\
         You are a clinical documentation assistant for a Chartered Psychologist (BPS).\n\
         You have the clinician's therapeutic framework and faithfulness rules above.\n\
         Write a session note for session {} on {} using the therapeutic framework provided.\n\
         Follow all faithfulness rules and grounding constraints exactly — they are non-negotiable.\n\
         Use the clinician's voice and framework from the reference material.\n\
         Output ONLY the session note (starting with ### {}), no preamble or explanation.\n\n\
         === OBSERVATION ===\n\
         {}\n",
        new_session_number, today, today, observation
    ));

    Ok(out)
}

/// Marker inserted to exclude a note from future voice fine-tuning.
pub const TRAINING_EXCLUDE_MARKER: &str = "<!-- training: exclude -->";

/// Inject the training-exclude marker immediately after the session header.
fn inject_exclude_marker(note: &str) -> String {
    let lines: Vec<&str> = note.lines().collect();
    let date_re = Regex::new(r"^### \d{4}-\d{2}-\d{2}").unwrap();

    let mut out = String::new();
    let mut injected = false;
    for line in &lines {
        out.push_str(line);
        out.push('\n');
        if !injected && date_re.is_match(line) {
            out.push_str(TRAINING_EXCLUDE_MARKER);
            out.push('\n');
            injected = true;
        }
    }
    out.trim_end().to_string()
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
pub fn save(id: &str, no_train: bool) -> Result<()> {
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
    let all_issues: Vec<&String> = validation.warnings.iter()
        .chain(validation.failures.iter())
        .collect();
    if !all_issues.is_empty() {
        eprintln!("Validation errors:");
        for err in &all_issues {
            eprintln!("  - {}", err);
        }
        bail!("Note failed validation — not saved");
    }

    // Append
    let path = client::notes_path(id);
    if !path.exists() {
        bail!("Client file not found: {}", path.display());
    }

    let note_to_save = if no_train {
        inject_exclude_marker(&note)
    } else {
        note.clone()
    };

    append_note(id, &note_to_save)?;

    let line_count = note_to_save.lines().count();
    let train_status = if no_train { " [excluded from training]" } else { "" };
    eprintln!("Saved to {}.md ({} lines appended){}", id, line_count, train_status);

    // Finalise (session count + alerts)
    finalise::run(id)?;

    // Regenerate client summary in background (async, non-blocking)
    let summary_id = id.to_string();
    std::thread::spawn(move || {
        let _ = Command::new("clinical")
            .arg("summarise")
            .arg(&summary_id)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn();
    });

    Ok(())
}

/// Retroactively mark or unmark a session note for training exclusion.
pub fn mark(id: &str, date: &str, exclude: bool, include: bool) -> Result<()> {
    if !exclude && !include {
        bail!("Must specify either --exclude or --include");
    }

    // Validate date format
    let date_re = Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap();
    if !date_re.is_match(date) {
        bail!("Date must be in YYYY-MM-DD format: {}", date);
    }

    let path = client::notes_path(id);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read: {}", path.display()))?;

    let lines: Vec<&str> = content.lines().collect();
    let header = format!("### {}", date);

    // Find the target session header
    let header_idx = lines
        .iter()
        .position(|l| l.starts_with(&header))
        .ok_or_else(|| anyhow::anyhow!("No session found for {} on {}", id, date))?;

    // Check if marker already present on the line immediately following
    let has_marker = header_idx + 1 < lines.len()
        && lines[header_idx + 1].trim() == TRAINING_EXCLUDE_MARKER;

    let mut new_lines: Vec<String> = lines.iter().map(|s| s.to_string()).collect();

    if exclude {
        if has_marker {
            eprintln!("{} {}: already excluded from training", id, date);
            return Ok(());
        }
        new_lines.insert(header_idx + 1, TRAINING_EXCLUDE_MARKER.to_string());
        eprintln!("{} {}: marked as excluded from training", id, date);
    } else {
        // include
        if !has_marker {
            eprintln!("{} {}: already included in training", id, date);
            return Ok(());
        }
        new_lines.remove(header_idx + 1);
        eprintln!("{} {}: marked as included in training", id, date);
    }

    let new_content = new_lines.join("\n") + "\n";
    std::fs::write(&path, new_content)
        .with_context(|| format!("Failed to write: {}", path.display()))?;

    Ok(())
}

/// Load voice config from ~/.config/clinical-product/voice-config.toml.
/// Returns (cmd, args) to use as the LLM subprocess.
///
/// Resolution order:
/// 1. `model_override` argument — forces the voice route with that model name
///    (still uses endpoint from config or env)
/// 2. CLINICAL_LLM_CMD / CLINICAL_LLM_ARGS env vars (if both set, and no override)
/// 3. voice-config.toml with [voice] endpoint + model (uses clinical-product raw)
/// 4. Default to `claude -p --output-format text`
fn resolve_llm_command(model_override: Option<&str>) -> (String, String) {
    // If model_override is set, we force the voice route. Still need an endpoint.
    if let Some(override_model) = model_override {
        let endpoint = load_voice_endpoint()
            .or_else(|| std::env::var("CLINICAL_VOICE_ENDPOINT").ok());
        if let Some(ep) = endpoint {
            let args = format!(
                "raw --model {} --endpoint {} --no-stream",
                override_model, ep
            );
            return ("clinical-product".to_string(), args);
        }
        // Fall through if no endpoint — this will use env/config/claude below
        eprintln!(
            "Warning: --model-override {} given but no voice endpoint configured",
            override_model
        );
    }

    // Env vars win (when no override)
    if let (Ok(cmd), Ok(args)) = (
        std::env::var("CLINICAL_LLM_CMD"),
        std::env::var("CLINICAL_LLM_ARGS"),
    ) {
        return (cmd, args);
    }

    // Try voice-config.toml
    if let Some((ep, model)) = load_voice_config() {
        let args = format!(
            "raw --model {} --endpoint {} --no-stream",
            model, ep
        );
        return ("clinical-product".to_string(), args);
    }

    // Fall back to claude
    (
        "claude".to_string(),
        "-p --output-format text".to_string(),
    )
}

/// Load voice-config.toml and return (endpoint, model) if [voice] is present.
fn load_voice_config() -> Option<(String, String)> {
    let home = dirs::home_dir()?;
    let config_path = home
        .join(".config")
        .join("clinical-product")
        .join("voice-config.toml");
    let content = std::fs::read_to_string(&config_path).ok()?;
    let value: toml::Value = toml::from_str(&content).ok()?;
    let voice = value.get("voice")?;
    let endpoint = voice.get("endpoint")?.as_str()?.to_string();
    let model = voice
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("clinical-voice-q8")
        .to_string();
    Some((endpoint, model))
}

/// Load just the endpoint from voice-config.toml.
fn load_voice_endpoint() -> Option<String> {
    load_voice_config().map(|(ep, _)| ep)
}

/// Public wrapper for `build_prompt` — used by batch processing.
pub fn build_prompt_public(id: &str, observation: &str) -> Result<String> {
    build_prompt(id, observation)
}

/// Public wrapper for `resolve_llm_command` — used by batch processing.
pub fn resolve_llm_command_public(model_override: Option<&str>) -> (String, String) {
    resolve_llm_command(model_override)
}

/// Run `clinical note <ID> <observation>`.
pub fn run(
    id: &str,
    observation: &str,
    no_train: bool,
    model_override: Option<&str>,
    no_save: bool,
    auto_confirm: bool,
) -> Result<()> {
    // Step 1: Build full context prompt + load client context for faithfulness
    eprintln!("Preparing context for {}...", id);
    let prompt = build_prompt(id, observation)?;
    let client_context = load_client_context(id).unwrap_or_default();

    let (llm_cmd, llm_args) = resolve_llm_command(model_override);
    if let Some(m) = model_override {
        eprintln!("Model override: {}", m);
    }
    if no_save {
        eprintln!("No-save mode: note will NOT be appended or finalised.");
    }

    let args: Vec<&str> = llm_args.split_whitespace().collect();

    const MAX_LINT_RETRIES: usize = 3;
    let mut note = String::new();
    let mut regen_reasons: Vec<String> = Vec::new();

    for attempt in 0..MAX_LINT_RETRIES {
        if attempt > 0 {
            eprintln!("Regenerating (attempt {}/{})...", attempt + 1, MAX_LINT_RETRIES);
        } else {
            eprintln!("Generating note via {}...", llm_cmd);
        }

        let child = Command::new(&llm_cmd)
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to start LLM command: {}", llm_cmd))?;

        let mut child = child;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes())?;
        }

        let output = child.wait_with_output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("LLM command failed (exit {}): {}", output.status, stderr);
        }

        note = String::from_utf8_lossy(&output.stdout).trim().to_string();

        if note.is_empty() {
            bail!("LLM returned empty output");
        }

        let validation = validate_note(&note);

        // Show structural warnings (soft)
        if !validation.warnings.is_empty() {
            eprintln!("\n⚠️  Validation warnings:");
            for w in &validation.warnings {
                eprintln!("  - {}", w);
            }
            eprintln!();
        }

        // Faithfulness check
        let faithfulness =
            crate::faithfulness::check_faithfulness(&note, observation, &client_context);

        // Show faithfulness soft flags (for human review)
        let soft_flags = faithfulness.soft_flags();
        if !soft_flags.is_empty() {
            eprintln!("⚠️  Faithfulness flags (review these):");
            for flag in &soft_flags {
                let display = if flag.sentence.len() > 80 {
                    format!("{}...", &flag.sentence[..80])
                } else {
                    flag.sentence.clone()
                };
                eprintln!("  - \"{}\"", display);
                eprintln!("    {}", flag.reason);
            }
            eprintln!();
        }

        // Check both structural lint gates and faithfulness hard failures
        let faith_hard = faithfulness.hard_failures();
        if validation.passed() && faith_hard.is_empty() {
            break;
        }

        // Show structural failures
        if !validation.passed() {
            eprintln!("\n🚫 Lint failure (auto-regenerating):");
            for f in &validation.failures {
                eprintln!("  - {}", f);
                regen_reasons.push(format!("lint: {}", f));
            }
        }

        // Show faithfulness hard failures
        if !faith_hard.is_empty() {
            eprintln!("\n🚫 Faithfulness failure (auto-regenerating):");
            for f in &faith_hard {
                let display = if f.sentence.len() > 80 {
                    format!("{}...", &f.sentence[..80])
                } else {
                    f.sentence.clone()
                };
                eprintln!("  - \"{}\"", display);
                eprintln!("    {}", f.reason);
                regen_reasons.push(format!("faithfulness: {}", f.reason));
            }
        }

        if attempt == MAX_LINT_RETRIES - 1 {
            eprintln!("\n⚠️  Max retries reached — showing note with remaining issues.");
        }
    }

    // Generation summary
    if regen_reasons.is_empty() {
        eprintln!("\n✓ Note generated on first attempt.");
    } else {
        let attempts = regen_reasons.len() + 1; // reasons = failed attempts, +1 for final
        eprintln!(
            "\n✓ Note generated after {} attempt{} ({} regeneration{}).",
            attempts,
            if attempts == 1 { "" } else { "s" },
            regen_reasons.len(),
            if regen_reasons.len() == 1 { "" } else { "s" },
        );
        eprintln!("  Regeneration triggers:");
        for reason in &regen_reasons {
            eprintln!("    - {}", reason);
        }
    }

    // Show note for review
    println!("\n{}", note);

    // Step 5: If no_save, stop here — caller only wanted to see the output.
    if no_save {
        eprintln!("\n(no-save mode — note not appended)");
        return Ok(());
    }

    // Step 6: Confirm
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

    // Step 7: Append (with optional training exclusion marker)
    let note_to_save = if no_train {
        inject_exclude_marker(&note)
    } else {
        note.clone()
    };
    append_note(id, &note_to_save)?;
    let train_status = if no_train { " [excluded from training]" } else { "" };
    eprintln!("Note appended to {}.md{}", id, train_status);

    // Step 8: Finalise
    finalise::run(id)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Compare mode: generate with Q4 and Q8, pick one, log both
// ---------------------------------------------------------------------------

/// Internal: generate a note with a specific model, run faithfulness, return result.
struct GeneratedComparison {
    model: String,
    note: String,
    attempts: usize,
    hard_failures: usize,
    soft_flags: Vec<String>,
    regen_reasons: Vec<String>,
    generation_secs: f64,
}

fn generate_one(
    _id: &str,
    observation: &str,
    model: &str,
    prompt: &str,
    client_context: &str,
) -> Result<GeneratedComparison> {
    let (llm_cmd, llm_args) = resolve_llm_command(Some(model));
    let args: Vec<&str> = llm_args.split_whitespace().collect();

    const MAX_RETRIES: usize = 3;
    let mut note = String::new();
    let mut attempts = 0;
    let mut regen_reasons: Vec<String> = Vec::new();
    let start = std::time::Instant::now();

    for attempt in 0..MAX_RETRIES {
        attempts = attempt + 1;

        let child = Command::new(&llm_cmd)
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to start: {}", llm_cmd))?;

        let mut child = child;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes())?;
        }
        let output = child.wait_with_output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("LLM failed (exit {}): {}", output.status, stderr);
        }

        note = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if note.is_empty() {
            bail!("LLM returned empty output");
        }

        let validation = validate_note(&note);
        let faithfulness =
            crate::faithfulness::check_faithfulness(&note, observation, client_context);

        if validation.passed() && faithfulness.passed_hard() {
            let flags: Vec<String> = faithfulness
                .soft_flags()
                .iter()
                .map(|f| f.reason.clone())
                .collect();
            return Ok(GeneratedComparison {
                model: model.to_string(),
                note,
                attempts,
                hard_failures: 0,
                soft_flags: flags,
                regen_reasons,
                generation_secs: start.elapsed().as_secs_f64(),
            });
        }

        // Collect reasons for this regeneration
        for f in &validation.failures {
            regen_reasons.push(format!("lint: {}", f));
        }
        for f in faithfulness.hard_failures() {
            regen_reasons.push(format!("faithfulness: {}", f.reason));
        }
    }

    // After max retries, return what we have
    let faithfulness =
        crate::faithfulness::check_faithfulness(&note, observation, client_context);
    let flags: Vec<String> = faithfulness
        .soft_flags()
        .iter()
        .map(|f| f.reason.clone())
        .collect();
    Ok(GeneratedComparison {
        model: model.to_string(),
        note,
        attempts,
        hard_failures: faithfulness.hard_failures().len(),
        soft_flags: flags,
        regen_reasons,
        generation_secs: start.elapsed().as_secs_f64(),
    })
}

/// Compare mode: generate with Q4 and Q8, show both, pick one, log both.
pub fn compare_run(id: &str, observation: &str, no_train: bool) -> Result<()> {
    eprintln!("Preparing context for {} (compare mode)...", id);
    let prompt = build_prompt(id, observation)?;
    let client_context = load_client_context(id).unwrap_or_default();

    // Generate Q4
    eprintln!("\n--- Generating Q4 ---");
    let q4 = generate_one(id, observation, "clinical-voice-q4", &prompt, &client_context)?;
    eprintln!("  {} attempt{} ({:.0}s)", q4.attempts, if q4.attempts == 1 { "" } else { "s" }, q4.generation_secs);
    if !q4.regen_reasons.is_empty() {
        for r in &q4.regen_reasons {
            eprintln!("    regen: {}", r);
        }
    }

    // Generate Q8
    eprintln!("\n--- Generating Q8 ---");
    let q8 = generate_one(id, observation, "clinical-voice-q8", &prompt, &client_context)?;
    eprintln!("  {} attempt{} ({:.0}s)", q8.attempts, if q8.attempts == 1 { "" } else { "s" }, q8.generation_secs);
    if !q8.regen_reasons.is_empty() {
        for r in &q8.regen_reasons {
            eprintln!("    regen: {}", r);
        }
    }

    // Display both
    eprintln!("\n========================================");
    println!("#0 — Q4\n");
    println!("{}", q4.note);

    if !q4.soft_flags.is_empty() {
        eprintln!("\n  Faithfulness flags (Q4):");
        for f in &q4.soft_flags {
            eprintln!("    - {}", f);
        }
    }

    println!("\n----------------------------------------");
    println!("#1 — Q8\n");
    println!("{}", q8.note);

    if !q8.soft_flags.is_empty() {
        eprintln!("\n  Faithfulness flags (Q8):");
        for f in &q8.soft_flags {
            eprintln!("    - {}", f);
        }
    }

    eprintln!("\n========================================");

    // Prompt for choice
    eprint!("\n[1] Accept Q4  [2] Accept Q8  [r] Reject both  > ");
    io::stderr().flush()?;

    let mut response = String::new();
    io::stdin().read_line(&mut response)?;
    let choice = response.trim().to_lowercase();

    let now = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();

    let (accepted_note, q4_accepted, q8_accepted) = match choice.as_str() {
        "1" => (Some(&q4.note), true, false),
        "2" => (Some(&q8.note), false, true),
        "r" => (None, false, false),
        _ => {
            eprintln!("Invalid choice. Aborting (both logged as rejected).");
            (None, false, false)
        }
    };

    // Log both to comparisons.jsonl
    let q4_entry = crate::faithfulness::ComparisonEntry {
        timestamp: now.clone(),
        client_id: id.to_string(),
        model: q4.model.clone(),
        observation: observation.to_string(),
        note: q4.note.clone(),
        hard_failures: q4.hard_failures,
        soft_flags: q4.soft_flags.len(),
        flag_details: q4.soft_flags.clone(),
        attempts: q4.attempts,
        regen_reasons: q4.regen_reasons.clone(),
        generation_secs: q4.generation_secs,
        accepted: Some(q4_accepted),
    };
    let q8_entry = crate::faithfulness::ComparisonEntry {
        timestamp: now,
        client_id: id.to_string(),
        model: q8.model.clone(),
        observation: observation.to_string(),
        note: q8.note.clone(),
        hard_failures: q8.hard_failures,
        soft_flags: q8.soft_flags.len(),
        flag_details: q8.soft_flags.clone(),
        attempts: q8.attempts,
        regen_reasons: q8.regen_reasons.clone(),
        generation_secs: q8.generation_secs,
        accepted: Some(q8_accepted),
    };

    if let Err(e) = crate::faithfulness::log_comparison(&q4_entry) {
        eprintln!("Warning: failed to log Q4 comparison: {}", e);
    }
    if let Err(e) = crate::faithfulness::log_comparison(&q8_entry) {
        eprintln!("Warning: failed to log Q8 comparison: {}", e);
    }

    // Save the accepted note
    if let Some(accepted) = accepted_note {
        let note_to_save = if no_train {
            inject_exclude_marker(accepted)
        } else {
            accepted.to_string()
        };
        append_note(id, &note_to_save)?;
        let model_name = if q4_accepted { "Q4" } else { "Q8" };
        eprintln!("Note ({}) appended to {}.md", model_name, id);
        finalise::run(id)?;
    } else {
        eprintln!("Both rejected. Nothing saved. Both logged to comparisons.jsonl.");
    }

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
        assert!(result.passed());
    }

    #[test]
    fn test_validate_note_missing_risk() {
        let note = "\
### 2026-03-20

Client explored workplace dynamics.

**Formulation**: Continued work on patterns.
";
        let result = validate_note(note);
        assert!(!result.warnings.is_empty());
        assert!(result.warnings.iter().any(|e| e.contains("Risk")));
    }

    #[test]
    fn test_validate_note_missing_formulation() {
        let note = "\
### 2026-03-20

**Risk**: No concerns.

Client explored workplace dynamics.
";
        let result = validate_note(note);
        assert!(!result.warnings.is_empty());
        assert!(result.warnings.iter().any(|e| e.contains("Formulation")));
    }

    #[test]
    fn test_validate_note_missing_header() {
        let note = "\
**Risk**: No concerns.

Client explored workplace dynamics.

**Formulation**: Continued work.
";
        let result = validate_note(note);
        assert!(!result.warnings.is_empty());
        assert!(result.warnings.iter().any(|e| e.contains("header")));
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
        assert!(result.passed());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_validate_note_refusal() {
        let note = "I can't generate clinical notes about this topic.";
        let result = validate_note(note);
        assert!(!result.warnings.is_empty());
        assert!(result.warnings.iter().any(|e| e.contains("refusal")));
    }

    #[test]
    fn test_build_prompt_contains_observation_and_instruction() {
        let tmp = tempfile::TempDir::new().unwrap();
        let clients_dir = tmp.path().join("clients").join("TEST01");
        std::fs::create_dir_all(&clients_dir).unwrap();
        // Route C layout: notes.md (no private/ dir in temp)
        let md_path = clients_dir.join("notes.md");
        std::fs::write(
            &md_path,
            "# TEST01\n\n## Session Notes\n\n### 2026-01-15\nFirst note.\n",
        )
        .unwrap();

        std::env::set_var("CLINICAL_ROOT", tmp.path());
        std::env::set_var("CLINICAL_NOTES_SKILL_DIR", "/nonexistent");

        let prompt = build_prompt("TEST01", "She discussed dating").unwrap();
        assert!(prompt.contains("She discussed dating"));
        // ACT/CBS appears via modality-act.md, which isn't available in test env.
        // The prompt contains the framework if the file exists, otherwise skips it.
        assert!(prompt.contains("INSTRUCTION"));
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

// ---------------------------------------------------------------------------
// Summarise
// ---------------------------------------------------------------------------

/// Generate a compressed clinical summary for a client, saved as summary.md.
pub fn summarise(
    id: Option<&str>,
    all: bool,
    dry_run: bool,
    model_override: Option<&str>,
) -> Result<()> {
    if all {
        let ids = client::list_client_ids()?;
        eprintln!("Summarising {} clients...", ids.len());
        for client_id in &ids {
            if let Err(e) = summarise_one(client_id, dry_run, model_override) {
                eprintln!("  {} — error: {}", client_id, e);
            }
        }
        Ok(())
    } else if let Some(id) = id {
        summarise_one(id, dry_run, model_override)
    } else {
        bail!("Provide a client ID or use --all")
    }
}

fn summarise_one(id: &str, dry_run: bool, model_override: Option<&str>) -> Result<()> {
    let notes_path = client::notes_path(id);
    let notes = std::fs::read_to_string(&notes_path)
        .with_context(|| format!("Could not read: {}", notes_path.display()))?;

    if notes.trim().is_empty() {
        eprintln!("  {} — no notes, skipping", id);
        return Ok(());
    }

    // Count sessions
    let session_count = notes.lines().filter(|l| l.starts_with("### ")).count();
    if session_count == 0 {
        eprintln!("  {} — no session headers, skipping", id);
        return Ok(());
    }

    // Build correspondence context
    let correspondence = find_correspondence(id);
    let corr_summary: String = correspondence
        .iter()
        .map(|(name, _)| format!("  - {}", name))
        .collect::<Vec<_>>()
        .join("\n");

    // Build the summarisation prompt
    let prompt = format!(
        "You are a clinical documentation assistant. Summarise the following client file \
         into a compressed clinical summary (~400-600 words). Preserve:\n\
         - Key history and referral context\n\
         - Ongoing therapeutic themes and patterns\n\
         - Current formulation and treatment trajectory\n\
         - Significant events or turning points\n\
         - Auth/funding status if present\n\n\
         Use clinical language (ACT/CBS) consistent with the notes.\n\
         Refer to the client by first name.\n\
         Output ONLY the summary, no preamble.\n\n\
         === CLIENT FILE ({}, {} sessions) ===\n{}\n\n\
         === CORRESPONDENCE FILES ===\n{}\n",
        id,
        session_count,
        notes,
        if corr_summary.is_empty() { "  (none)".to_string() } else { corr_summary },
    );

    // Include correspondence content for context
    let mut full_prompt = prompt;
    for (name, content) in &correspondence {
        full_prompt.push_str(&format!("\n--- {} ---\n{}\n", name, content));
    }

    eprintln!("  {} — generating summary ({} sessions)...", id, session_count);

    // Resolve LLM command (same as note generation)
    let (cmd_name, cmd_args) = resolve_llm_command(model_override);
    let args: Vec<&str> = cmd_args.split_whitespace().collect();

    let mut child = Command::new(&cmd_name)
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to spawn {}", cmd_name))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(full_prompt.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    let summary = String::from_utf8_lossy(&output.stdout).to_string();

    if summary.trim().is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{} — LLM returned empty summary. stderr: {}", id, stderr);
    }

    if dry_run {
        println!("=== SUMMARY: {} ===\n{}", id, summary);
    } else {
        let summary_path = client::client_dir(id).join("summary.md");
        std::fs::write(&summary_path, &summary)
            .with_context(|| format!("write: {}", summary_path.display()))?;
        eprintln!("  {} — saved to {}", id, summary_path.display());
    }

    Ok(())
}
