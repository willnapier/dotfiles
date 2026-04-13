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
pub fn run(input_path: &str, no_save: bool) -> Result<()> {
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

        // Validate
        let validation = note::validate_note(&note_text);
        let all_issues: Vec<&String> = validation.warnings.iter()
            .chain(validation.failures.iter())
            .collect();
        let warn = if !all_issues.is_empty() {
            format!(" [{}]", all_issues.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("; "))
        } else {
            String::new()
        };

        eprintln!("{:.0}s{}", elapsed, warn);

        generated.push(GeneratedNote {
            client_id: entry.client_id.clone(),
            observation: entry.observation.clone(),
            note: note_text,
            generation_time_secs: elapsed,
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
            "## {} ({:.0}s)\n\n{}\n\n---\n\n",
            gen.client_id, gen.generation_time_secs, gen.note
        ));
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
}
