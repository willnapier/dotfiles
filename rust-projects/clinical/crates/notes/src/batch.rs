//! Batch note processing — write multiple observations, cook in background,
//! review all at once.
//!
//! Input: a markdown file with `# CLIENT_ID` headings and observation text.
//! Output: a generated-notes file opened in $EDITOR for review.
//! After review: accepted notes are appended to client files + finalised.

use anyhow::{bail, Context, Result};
use regex::Regex;
use std::io::Write;
use std::process::Command;

use clinical_core::client;

use crate::finalise;
use crate::note;

/// A single observation entry from the batch input file.
#[derive(Debug)]
pub struct BatchEntry {
    pub client_id: String,
    pub observation: String,
}

/// A generated note ready for review.
#[derive(Debug)]
struct GeneratedNote {
    pub client_id: String,
    pub observation: String,
    pub note: String,
    pub generation_time_secs: f64,
    pub faithfulness_annotations: Option<String>,
}

/// Parse a batch input file into entries.
///
/// Format:
/// ```markdown
/// # CT71
/// Catrin reported accepting the new role. She credited the 'old codger'
/// discussion as pivotal. Ambivalence from husband about time commitment.
///
/// # MA93
/// Client presented with renewed anxiety about custody hearing...
///
/// # BB88
/// Ms Brown reported a productive week...
/// ```
pub fn parse_batch_file(content: &str) -> Result<Vec<BatchEntry>> {
    let header_re = Regex::new(r"^#\s+(\S+)\s*$").unwrap();
    let mut entries = Vec::new();
    let mut current_id: Option<String> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    for line in content.lines() {
        if let Some(caps) = header_re.captures(line) {
            // Flush previous entry
            if let Some(id) = current_id.take() {
                let obs = current_lines.join("\n").trim().to_string();
                if !obs.is_empty() {
                    entries.push(BatchEntry {
                        client_id: id,
                        observation: obs,
                    });
                }
            }
            current_id = Some(caps[1].to_string());
            current_lines.clear();
        } else {
            current_lines.push(line);
        }
    }

    // Flush last entry
    if let Some(id) = current_id.take() {
        let obs = current_lines.join("\n").trim().to_string();
        if !obs.is_empty() {
            entries.push(BatchEntry {
                client_id: id,
                observation: obs,
            });
        }
    }

    if entries.is_empty() {
        bail!(
            "No entries found. Expected format:\n\
             # CLIENT_ID\n\
             observation text...\n\n\
             # ANOTHER_ID\n\
             more observation text..."
        );
    }

    Ok(entries)
}

/// Run the full batch workflow:
/// 1. Parse the input file
/// 2. Generate notes for all entries (sequentially via LLM)
/// 3. Write results to a review file
/// 4. Open in $EDITOR
/// 5. After editor closes, parse the review file and save accepted notes
pub fn run(input_path: &str, no_save: bool, compare: bool) -> Result<()> {
    if compare {
        return run_compare(input_path, no_save);
    }
    run_single(input_path, no_save)
}

fn run_single(input_path: &str, no_save: bool) -> Result<()> {
    let content = std::fs::read_to_string(input_path)
        .with_context(|| format!("Failed to read: {}", input_path))?;

    let entries = parse_batch_file(&content)?;
    eprintln!("Parsed {} observations.", entries.len());

    // Verify all client IDs exist before starting generation
    for entry in &entries {
        let path = client::notes_path(&entry.client_id);
        if !path.exists() {
            bail!(
                "Client file not found: {} ({}). Fix the batch file and re-run.",
                entry.client_id,
                path.display()
            );
        }
    }

    // Generate notes
    eprintln!("Generating notes...\n");
    let mut generated: Vec<GeneratedNote> = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        eprint!(
            "[{}/{}] {} ... ",
            i + 1,
            entries.len(),
            entry.client_id
        );
        std::io::stderr().flush()?;

        let start = std::time::Instant::now();

        let prompt = note::build_prompt_public(&entry.client_id, &entry.observation)?;
        let (llm_cmd, llm_args) = note::resolve_llm_command_public(None);

        let args: Vec<&str> = llm_args.split_whitespace().collect();
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
        let elapsed = start.elapsed().as_secs_f64();

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("FAILED ({:.0}s): {}", elapsed, stderr.trim());
            continue;
        }

        let note_text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if note_text.is_empty() {
            eprintln!("EMPTY ({:.0}s)", elapsed);
            continue;
        }

        // Validate (structural)
        let validation = note::validate_note(&note_text);
        let all_issues: Vec<&String> = validation.warnings.iter()
            .chain(validation.failures.iter())
            .collect();
        let warn = if !all_issues.is_empty() {
            format!(" [{}]", all_issues.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("; "))
        } else {
            String::new()
        };

        // Faithfulness check
        let client_ctx = note::load_client_context_public(&entry.client_id).unwrap_or_default();
        let faith = crate::faithfulness::check_faithfulness(
            &note_text,
            &entry.observation,
            &client_ctx,
        );
        let faith_warn = if !faith.hard_failures().is_empty() || !faith.soft_flags().is_empty() {
            let count = faith.hard_failures().len() + faith.soft_flags().len();
            format!(" [{}  faithfulness flag{}]", count, if count == 1 { "" } else { "s" })
        } else {
            String::new()
        };

        eprintln!("{:.0}s{}{}", elapsed, warn, faith_warn);

        generated.push(GeneratedNote {
            client_id: entry.client_id.clone(),
            observation: entry.observation.clone(),
            note: note_text,
            generation_time_secs: elapsed,
            faithfulness_annotations: crate::faithfulness::format_flags_for_review(&faith),
        });
    }

    if generated.is_empty() {
        bail!("No notes were generated successfully.");
    }

    eprintln!(
        "\n{}/{} notes generated.",
        generated.len(),
        entries.len()
    );

    // Write review file
    let review_path = format!(
        "/tmp/clinical-batch-review-{}.md",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    );

    let mut review = String::new();
    review.push_str("# Batch Review\n");
    review.push_str("# Delete any note you don't want to save.\n");
    review.push_str("# Edit notes freely. Save and close to accept.\n\n");

    for gen in &generated {
        review.push_str(&format!(
            "## {} ({:.0}s)\n\n{}\n",
            gen.client_id, gen.generation_time_secs, gen.note
        ));
        if let Some(ref annotations) = gen.faithfulness_annotations {
            review.push_str("\n");
            review.push_str(annotations);
        }
        review.push_str("\n---\n\n");
    }

    std::fs::write(&review_path, &review)
        .with_context(|| format!("Failed to write: {}", review_path))?;

    if no_save {
        eprintln!("Review file: {}", review_path);
        print!("{}", review);
        return Ok(());
    }

    // Open in $EDITOR
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    eprintln!("Opening review file in {}...", editor);

    let status = Command::new(&editor)
        .arg(&review_path)
        .status()
        .with_context(|| format!("Failed to open editor: {}", editor))?;

    if !status.success() {
        bail!("Editor exited with error. No notes saved.");
    }

    // Parse the review file after editing
    let edited = std::fs::read_to_string(&review_path)
        .context("Failed to read review file after editing")?;

    let accepted = parse_review_file(&edited, &generated)?;

    if accepted.is_empty() {
        eprintln!("No notes accepted (all deleted or file empty).");
        return Ok(());
    }

    // Save accepted notes
    eprintln!("Saving {} notes...", accepted.len());
    for (client_id, note_text) in &accepted {
        note::append_note(client_id, note_text)?;
        eprintln!("  {} — appended", client_id);
        finalise::run(client_id)?;
    }

    // Clean up
    std::fs::remove_file(&review_path).ok();
    eprintln!("Done. {} notes saved.", accepted.len());

    Ok(())
}

/// Parse the edited review file to find which notes survived editing.
/// Returns (client_id, note_text) pairs.
fn parse_review_file(
    content: &str,
    originals: &[GeneratedNote],
) -> Result<Vec<(String, String)>> {
    let header_re = Regex::new(r"^## (\S+)").unwrap();
    let mut results = Vec::new();
    let mut current_id: Option<String> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    for line in content.lines() {
        // Skip comment lines
        if line.starts_with("# ") && !line.starts_with("## ") {
            continue;
        }

        if let Some(caps) = header_re.captures(line) {
            // Flush previous
            if let Some(id) = current_id.take() {
                let text = flush_note(&current_lines);
                if !text.is_empty() {
                    results.push((id, text));
                }
            }
            current_id = Some(caps[1].to_string());
            current_lines.clear();
        } else if line.trim() == "---" {
            // Section separator — flush
            if let Some(id) = current_id.take() {
                let text = flush_note(&current_lines);
                if !text.is_empty() {
                    results.push((id, text));
                }
                current_lines.clear();
            }
        } else {
            current_lines.push(line);
        }
    }

    // Flush last
    if let Some(id) = current_id.take() {
        let text = flush_note(&current_lines);
        if !text.is_empty() {
            results.push((id, text));
        }
    }

    Ok(results)
}

fn flush_note(lines: &[&str]) -> String {
    let text = lines.join("\n").trim().to_string();
    // Skip if it's just the timing info or empty
    if text.is_empty() || !text.contains("###") {
        return String::new();
    }
    text
}

// ---------------------------------------------------------------------------
// Compare mode: generate Q4 and Q8 per observation, pick one per client.
// All attempts (accepted and rejected) logged to ~/Clinical/comparisons.jsonl.
// ---------------------------------------------------------------------------

const COMPARE_MODELS: &[(&str, &str)] = &[
    ("q4", "clinical-voice-q4"),
    ("q8", "clinical-voice-q8"),
];

struct CompareVariant {
    client_id: String,
    observation: String,
    variant_label: String,
    gen: note::GeneratedComparison,
}

fn run_compare(input_path: &str, no_save: bool) -> Result<()> {
    let content = std::fs::read_to_string(input_path)
        .with_context(|| format!("Failed to read: {}", input_path))?;

    let entries = parse_batch_file(&content)?;
    eprintln!("Parsed {} observations. Compare mode: generating Q4 + Q8 per observation.", entries.len());

    // Verify all client IDs exist before starting generation
    for entry in &entries {
        let path = client::notes_path(&entry.client_id);
        if !path.exists() {
            bail!(
                "Client file not found: {} ({}). Fix the batch file and re-run.",
                entry.client_id,
                path.display()
            );
        }
    }

    eprintln!("Generating notes...\n");
    let mut variants: Vec<CompareVariant> = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        eprintln!("[{}/{}] {}", i + 1, entries.len(), entry.client_id);

        let prompt = note::build_prompt_public(&entry.client_id, &entry.observation)?;
        let client_ctx = note::load_client_context_public(&entry.client_id).unwrap_or_default();

        for (label, model) in COMPARE_MODELS {
            eprint!("  {} ... ", label);
            std::io::stderr().flush()?;

            match note::generate_one(&entry.client_id, &entry.observation, model, &prompt, &client_ctx) {
                Ok(gen) => {
                    eprintln!(
                        "{:.0}s, {} attempt{}, {} flag{}",
                        gen.generation_secs,
                        gen.attempts,
                        if gen.attempts == 1 { "" } else { "s" },
                        gen.soft_flags.len(),
                        if gen.soft_flags.len() == 1 { "" } else { "s" },
                    );
                    variants.push(CompareVariant {
                        client_id: entry.client_id.clone(),
                        observation: entry.observation.clone(),
                        variant_label: label.to_string(),
                        gen,
                    });
                }
                Err(e) => {
                    eprintln!("FAILED: {}", e);
                }
            }
        }
    }

    if variants.is_empty() {
        bail!("No notes were generated successfully.");
    }

    // Write review file: per observation, both variants side-by-side
    let review_path = format!(
        "/tmp/clinical-batch-compare-{}.md",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    );

    let mut review = String::new();
    review.push_str("# Batch Review — Compare mode\n");
    review.push_str("# For each client: DELETE the variant you don't want, keep the one you'll save.\n");
    review.push_str("# Keep the full `## <ID> — <variant>` header of the one you're keeping.\n");
    review.push_str("# Delete the whole section (header + note + trailing ---) of the other.\n");
    review.push_str("# Leaving both variants is an error; leaving neither = reject for that client.\n\n");

    // Group variants by client_id preserving order
    let mut seen_ids: Vec<String> = Vec::new();
    for v in &variants {
        if !seen_ids.contains(&v.client_id) {
            seen_ids.push(v.client_id.clone());
        }
    }

    for id in &seen_ids {
        let client_variants: Vec<&CompareVariant> = variants.iter().filter(|v| &v.client_id == id).collect();

        // Observation block (informational, not part of chosen note)
        if let Some(first) = client_variants.first() {
            review.push_str(&format!("<!-- Observation for {}: {} -->\n\n", id, first.observation.replace('\n', " ")));
        }

        for v in &client_variants {
            review.push_str(&format!(
                "## {} — {} [{:.0}s, {} attempt{}, {} flag{}]\n\n{}\n",
                v.client_id,
                v.variant_label,
                v.gen.generation_secs,
                v.gen.attempts,
                if v.gen.attempts == 1 { "" } else { "s" },
                v.gen.soft_flags.len(),
                if v.gen.soft_flags.len() == 1 { "" } else { "s" },
                v.gen.note,
            ));
            if !v.gen.soft_flags.is_empty() {
                review.push_str("\n<!-- Faithfulness flags:\n");
                for f in &v.gen.soft_flags {
                    review.push_str(&format!("  - {}\n", f));
                }
                review.push_str("-->\n");
            }
            review.push_str("\n---\n\n");
        }
    }

    std::fs::write(&review_path, &review)
        .with_context(|| format!("Failed to write: {}", review_path))?;

    if no_save {
        eprintln!("Review file: {}", review_path);
        print!("{}", review);
        // Log to comparisons.jsonl even in no-save mode (as unaccepted)
        log_all_variants(&variants, None)?;
        return Ok(());
    }

    // Open in $EDITOR
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    eprintln!("\nOpening review file in {}...", editor);

    let status = Command::new(&editor)
        .arg(&review_path)
        .status()
        .with_context(|| format!("Failed to open editor: {}", editor))?;

    if !status.success() {
        bail!("Editor exited with error. No notes saved.");
    }

    // Parse edited file to see which variants survived per client
    let edited = std::fs::read_to_string(&review_path)
        .context("Failed to read review file after editing")?;

    let chosen = parse_compare_review(&edited)?;

    // Validate: at most one variant per client
    let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (id, variant, _) in &chosen {
        if let Some(existing) = seen.get(id) {
            bail!(
                "Multiple variants left for {}: {} and {}. Delete one and re-run.",
                id, existing, variant
            );
        }
        seen.insert(id.clone(), variant.clone());
    }

    // Build accepted-decisions map (client_id -> variant_label, or None = rejected)
    let mut decisions: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
    for id in &seen_ids {
        decisions.insert(id.clone(), seen.get(id).cloned());
    }

    // Log all variants with acceptance flag
    log_all_variants(&variants, Some(&decisions))?;

    if chosen.is_empty() {
        eprintln!("No notes accepted (all deleted).");
        return Ok(());
    }

    // Save accepted notes
    eprintln!("\nSaving {} notes...", chosen.len());
    for (client_id, variant_label, note_text) in &chosen {
        note::append_note(client_id, note_text)?;
        eprintln!("  {} ({}) — appended", client_id, variant_label);
        finalise::run(client_id)?;
    }

    // Clean up
    std::fs::remove_file(&review_path).ok();
    eprintln!("Done. {} notes saved. All {} variants logged to ~/Clinical/comparisons.jsonl.", chosen.len(), variants.len());

    Ok(())
}

/// Parse a compare-mode review file. Expects `## <ID> — <variant>` headers.
/// Returns (client_id, variant_label, note_text) tuples.
fn parse_compare_review(content: &str) -> Result<Vec<(String, String, String)>> {
    // Header pattern: "## CLIENT_ID — variant" (em-dash, from our writer)
    let header_re = Regex::new(r"^##\s+(\S+)\s+—\s+(\S+)").unwrap();
    let mut results = Vec::new();
    let mut current: Option<(String, String)> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    for line in content.lines() {
        if line.starts_with("# ") && !line.starts_with("## ") {
            continue; // top-level heading / comments
        }

        if let Some(caps) = header_re.captures(line) {
            if let Some((id, variant)) = current.take() {
                let text = flush_note(&current_lines);
                if !text.is_empty() {
                    results.push((id, variant, text));
                }
            }
            current = Some((caps[1].to_string(), caps[2].to_string()));
            current_lines.clear();
        } else if line.trim() == "---" {
            if let Some((id, variant)) = current.take() {
                let text = flush_note(&current_lines);
                if !text.is_empty() {
                    results.push((id, variant, text));
                }
                current_lines.clear();
            }
        } else {
            current_lines.push(line);
        }
    }

    if let Some((id, variant)) = current.take() {
        let text = flush_note(&current_lines);
        if !text.is_empty() {
            results.push((id, variant, text));
        }
    }

    Ok(results)
}

/// Log every generated variant to ~/Clinical/comparisons.jsonl.
/// `decisions` is a map of client_id -> accepted variant label (None = rejected).
fn log_all_variants(
    variants: &[CompareVariant],
    decisions: Option<&std::collections::HashMap<String, Option<String>>>,
) -> Result<()> {
    let now = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
    for v in variants {
        let accepted = decisions.and_then(|d| {
            d.get(&v.client_id).map(|chosen| {
                chosen.as_ref().map(|c| c == &v.variant_label).unwrap_or(false)
            })
        });
        let entry = crate::faithfulness::ComparisonEntry {
            timestamp: now.clone(),
            client_id: v.client_id.clone(),
            model: v.gen.model.clone(),
            observation: v.observation.clone(),
            note: v.gen.note.clone(),
            hard_failures: v.gen.hard_failures,
            soft_flags: v.gen.soft_flags.len(),
            flag_details: v.gen.soft_flags.clone(),
            attempts: v.gen.attempts,
            regen_reasons: v.gen.regen_reasons.clone(),
            generation_secs: v.gen.generation_secs,
            accepted,
        };
        if let Err(e) = crate::faithfulness::log_comparison(&entry) {
            eprintln!("Warning: failed to log {}/{} to comparisons.jsonl: {}", v.client_id, v.variant_label, e);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_batch_file() {
        let input = "\
# CT71
Client discussed anxiety about the new role.

# MA93
Client presented with renewed worry about custody hearing.
Explored the function of avoidance behaviours.
";
        let entries = parse_batch_file(input).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].client_id, "CT71");
        assert!(entries[0].observation.contains("anxiety"));
        assert_eq!(entries[1].client_id, "MA93");
        assert!(entries[1].observation.contains("custody"));
    }

    #[test]
    fn test_parse_batch_file_empty() {
        let result = parse_batch_file("no headers here");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_compare_review_both_variants_kept() {
        let input = "\
# Batch Review
# Delete one variant per client.

## CT71 — q4 [12s, 1 attempt]

### 2026-04-17

Body of q4 note.

---

## CT71 — q8 [11s, 1 attempt]

### 2026-04-17

Body of q8 note.

---
";
        let result = parse_compare_review(input).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].0, "CT71");
        assert_eq!(result[0].1, "q4");
        assert!(result[0].2.contains("Body of q4"));
        assert_eq!(result[1].0, "CT71");
        assert_eq!(result[1].1, "q8");
        assert!(result[1].2.contains("Body of q8"));
    }

    #[test]
    fn test_parse_compare_review_one_variant_kept() {
        let input = "\
# Batch Review

## CT71 — q8 [11s, 1 attempt]

### 2026-04-17

Only q8 survived.

---
";
        let result = parse_compare_review(input).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "CT71");
        assert_eq!(result[0].1, "q8");
        assert!(result[0].2.contains("Only q8"));
    }

    #[test]
    fn test_parse_compare_review_none_kept() {
        let input = "\
# Batch Review

<!-- everything deleted -->
";
        let result = parse_compare_review(input).unwrap();
        assert!(result.is_empty());
    }
}
