//! Training corpus accounting — counts and lists session notes eligible
//! for voice fine-tuning.
//!
//! A note is eligible unless it carries the exclusion marker
//! (`<!-- training: exclude -->`) immediately after the session header.

use anyhow::{Context, Result};
use chrono::NaiveDate;
use regex::Regex;

use clinical_core::client;

use crate::note::TRAINING_EXCLUDE_MARKER;
use crate::session;

const VOICE_STATE_FILE: &str = "voice-state.toml";

#[derive(Debug)]
pub struct NoteRecord {
    pub client_id: String,
    pub date: NaiveDate,
    pub excluded: bool,
}

/// Walk all client files and extract every session note with its
/// inclusion status.
pub fn collect_all_notes() -> Result<Vec<NoteRecord>> {
    let clients_dir = client::clients_dir();
    let client_files = session::find_client_md_files(&clients_dir)?;

    let date_re = Regex::new(r"^### (\d{4}-\d{2}-\d{2})").unwrap();

    let mut records = Vec::new();

    for (id, path) in &client_files {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read: {}", path.display()))?;

        let lines: Vec<&str> = content.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if let Some(caps) = date_re.captures(line) {
                let date_str = caps.get(1).unwrap().as_str();
                let date = match NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                // Check if the next non-empty line is the exclusion marker
                let excluded = lines
                    .iter()
                    .skip(i + 1)
                    .take(3) // look at up to 3 lines after the header
                    .any(|l| l.trim() == TRAINING_EXCLUDE_MARKER);

                records.push(NoteRecord {
                    client_id: id.clone(),
                    date,
                    excluded,
                });
            }
        }
    }

    records.sort_by(|a, b| a.date.cmp(&b.date));
    Ok(records)
}

/// Load the last fine-tune date from voice-state.toml if present.
pub fn last_finetune_date() -> Option<NaiveDate> {
    let path = voice_state_path();
    let content = std::fs::read_to_string(&path).ok()?;
    let value: toml::Value = toml::from_str(&content).ok()?;
    value
        .get("training")
        .and_then(|t| t.get("last_finetune_date"))
        .and_then(|v| v.as_str())
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
}

fn voice_state_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("VOICE_STATE_PATH") {
        return std::path::PathBuf::from(p);
    }
    let base = if let Some(home) = dirs::home_dir() {
        home.join(".config").join("clinical-product")
    } else {
        std::path::PathBuf::from(".")
    };
    base.join(VOICE_STATE_FILE)
}

/// Run `clinical training count`.
pub fn count(all: bool) -> Result<()> {
    let records = collect_all_notes()?;

    let last_ft = if all { None } else { last_finetune_date() };

    let (since_label, eligible): (String, Vec<&NoteRecord>) = if let Some(d) = last_ft {
        (
            format!("since last fine-tune ({})", d),
            records
                .iter()
                .filter(|r| r.date > d && !r.excluded)
                .collect(),
        )
    } else {
        (
            "all time".to_string(),
            records.iter().filter(|r| !r.excluded).collect(),
        )
    };

    let excluded_count = records.iter().filter(|r| r.excluded).count();
    let total = records.len();

    println!("Training corpus status:");
    println!("  Window:              {}", since_label);
    println!("  Eligible notes:      {}", eligible.len());
    println!("  Total notes:         {}", total);
    println!("  Excluded from train: {}", excluded_count);

    if let Some(d) = last_ft {
        let future: Vec<_> = records.iter().filter(|r| r.date > d).collect();
        println!("  Total since last ft: {}", future.len());
    }

    Ok(())
}

/// Run `clinical training list`.
pub fn list(excluded_only: bool) -> Result<()> {
    let records = collect_all_notes()?;

    let filtered: Vec<&NoteRecord> = if excluded_only {
        records.iter().filter(|r| r.excluded).collect()
    } else {
        records.iter().collect()
    };

    if filtered.is_empty() {
        if excluded_only {
            println!("No notes marked as excluded from training.");
        } else {
            println!("No session notes found.");
        }
        return Ok(());
    }

    for rec in &filtered {
        let marker = if rec.excluded { "  [excluded]" } else { "" };
        println!("{}  {}{}", rec.date, rec.client_id, marker);
    }

    println!();
    println!("{} notes shown.", filtered.len());

    Ok(())
}

/// Extract the full text of a single session note from a client file.
/// Returns the text from `### YYYY-MM-DD` to the line before the next
/// `### ` header or end of file, trimmed.
fn extract_note_text(content: &str, date: &NaiveDate) -> Option<String> {
    let date_str = date.format("%Y-%m-%d").to_string();
    let header = format!("### {}", date_str);
    let lines: Vec<&str> = content.lines().collect();
    let date_re = Regex::new(r"^### \d{4}-\d{2}-\d{2}").unwrap();

    // Find the start line
    let start = lines.iter().position(|l| l.starts_with(&header))?;

    // Find the end — next session header or end of file
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| date_re.is_match(l))
        .map(|(i, _)| i)
        .unwrap_or(lines.len());

    let note_lines: Vec<&str> = lines[start..end].to_vec();
    let text = note_lines.join("\n").trim().to_string();

    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

const TRAINING_SYSTEM_PROMPT: &str =
    "You are a clinical psychologist's session note writer. \
     Produce a session note in the practitioner's established style.";

const TRAINING_USER_PROMPT: &str = "Write a session note for today's session.";

/// Replace the client's first name with "Client" (and possessive forms) in a
/// note's text. Used for anonymised training export — on-disk notes are
/// unchanged; only the JSONL corpus sees the substitution.
///
/// Word-boundary matching prevents partial overlaps (e.g. "Emma" inside
/// "Emmanuel"). Possessive forms are replaced first so "Emma's" becomes
/// "Client's", not "Client" + stranded "'s".
pub fn anonymise_first_name(note_text: &str, first_name: &str) -> String {
    if first_name.trim().is_empty() {
        return note_text.to_string();
    }
    let escaped = regex::escape(first_name);
    // Possessive first — \b before the name, then literal 's (apostrophe breaks word boundary)
    let possessive = Regex::new(&format!(r"\b{}'s", escaped)).unwrap();
    // Bare name with word boundaries on both sides
    let bare = Regex::new(&format!(r"\b{}\b", escaped)).unwrap();
    let step1 = possessive.replace_all(note_text, "Client's").to_string();
    bare.replace_all(&step1, "Client").to_string()
}

/// Look up a client's first name from identity.yaml. Returns empty string
/// if the file is missing, malformed, or has no name field.
fn client_first_name(client_id: &str) -> String {
    let path = client::identity_path(client_id);
    if !path.exists() {
        return String::new();
    }
    let identity = match clinical_core::identity::load_identity(&path) {
        Ok(i) => i,
        Err(_) => return String::new(),
    };
    let name = identity.name.unwrap_or_default();
    name.split_whitespace().next().unwrap_or("").to_string()
}

/// Run `clinical training export`.
/// Outputs one JSONL line per eligible note, in the format used for
/// voice model fine-tuning (messages array with system/user/assistant roles).
///
/// When `anonymise` is true, each client's first name (and possessive forms)
/// is replaced with "Client" in the exported corpus. On-disk notes are not
/// modified.
pub fn export(output: Option<&str>, all: bool, anonymise: bool) -> Result<()> {
    let records = collect_all_notes()?;
    let last_ft = if all { None } else { last_finetune_date() };

    let eligible: Vec<&NoteRecord> = if let Some(d) = last_ft {
        records
            .iter()
            .filter(|r| r.date > d && !r.excluded)
            .collect()
    } else {
        records.iter().filter(|r| !r.excluded).collect()
    };

    if eligible.is_empty() {
        eprintln!("No eligible notes to export.");
        return Ok(());
    }

    // Load client files into a cache (avoid re-reading for each note)
    let clients_dir = client::clients_dir();
    let client_files = session::find_client_md_files(&clients_dir)?;
    let file_contents: std::collections::HashMap<String, String> = client_files
        .iter()
        .filter_map(|(id, path)| {
            std::fs::read_to_string(path)
                .ok()
                .map(|c| (id.clone(), c))
        })
        .collect();

    // First-name cache so we don't re-parse identity.yaml for every note
    let mut first_name_cache: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    let mut lines = Vec::new();
    let mut skipped = 0u32;
    let mut anonymised_count = 0u32;
    let mut missing_name = 0u32;

    for rec in &eligible {
        let content = match file_contents.get(&rec.client_id) {
            Some(c) => c,
            None => {
                skipped += 1;
                continue;
            }
        };

        let note_text = match extract_note_text(content, &rec.date) {
            Some(t) => t,
            None => {
                skipped += 1;
                continue;
            }
        };

        let final_note = if anonymise {
            let first_name = first_name_cache
                .entry(rec.client_id.clone())
                .or_insert_with(|| client_first_name(&rec.client_id))
                .clone();
            if first_name.is_empty() {
                missing_name += 1;
                note_text
            } else {
                anonymised_count += 1;
                anonymise_first_name(&note_text, &first_name)
            }
        } else {
            note_text
        };

        let entry = serde_json::json!({
            "messages": [
                {"role": "system", "content": TRAINING_SYSTEM_PROMPT},
                {"role": "user", "content": TRAINING_USER_PROMPT},
                {"role": "assistant", "content": final_note}
            ]
        });

        lines.push(serde_json::to_string(&entry)?);
    }

    let jsonl = lines.join("\n") + "\n";

    if let Some(path) = output {
        std::fs::write(path, &jsonl)
            .with_context(|| format!("Failed to write: {}", path))?;
        eprintln!("Exported {} notes to {}", lines.len(), path);
    } else {
        print!("{}", jsonl);
    }

    if skipped > 0 {
        eprintln!("{} notes skipped (missing file or empty note).", skipped);
    }

    if anonymise {
        eprintln!(
            "Anonymised {} note(s); {} note(s) had no client name in identity.yaml.",
            anonymised_count, missing_name
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_excluded_note() {
        let content = "# TEST01\n\n## Session Notes\n\n\
### 2026-01-15\n<!-- training: exclude -->\n\n**Risk**: None.\nBody.\n**Formulation**: ok.\n\n\
### 2026-01-22\n\n**Risk**: None.\nBody.\n**Formulation**: ok.\n";

        // We can't easily test collect_all_notes without a real client dir, but
        // we can test the exclusion detection logic inline.
        let lines: Vec<&str> = content.lines().collect();
        let date_re = Regex::new(r"^### (\d{4}-\d{2}-\d{2})").unwrap();

        let mut results = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if let Some(caps) = date_re.captures(line) {
                let date = caps.get(1).unwrap().as_str().to_string();
                let excluded = lines
                    .iter()
                    .skip(i + 1)
                    .take(3)
                    .any(|l| l.trim() == TRAINING_EXCLUDE_MARKER);
                results.push((date, excluded));
            }
        }

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "2026-01-15");
        assert!(results[0].1, "First note should be excluded");
        assert_eq!(results[1].0, "2026-01-22");
        assert!(!results[1].1, "Second note should be included");
    }

    #[test]
    fn test_anonymise_bare_name() {
        let note = "Emma attended today. Emma reported feeling stuck.";
        assert_eq!(
            anonymise_first_name(note, "Emma"),
            "Client attended today. Client reported feeling stuck."
        );
    }

    #[test]
    fn test_anonymise_possessive() {
        let note = "Emma's mood was low. Emma described Emma's brother.";
        assert_eq!(
            anonymise_first_name(note, "Emma"),
            "Client's mood was low. Client described Client's brother."
        );
    }

    #[test]
    fn test_anonymise_word_boundary_protects_longer_names() {
        // "Emmanuel" should NOT be anonymised when the client's first name is "Emma"
        let note = "Emma referred to her friend Emmanuel.";
        assert_eq!(
            anonymise_first_name(note, "Emma"),
            "Client referred to her friend Emmanuel."
        );
    }

    #[test]
    fn test_anonymise_leaves_other_names_intact() {
        // Partner/family/referrer names stay — only the client's own first name changes
        let note = "Emma discussed her husband David and Dr Sarah Smith (referrer).";
        assert_eq!(
            anonymise_first_name(note, "Emma"),
            "Client discussed her husband David and Dr Sarah Smith (referrer)."
        );
    }

    #[test]
    fn test_anonymise_empty_first_name_is_noop() {
        let note = "Some note body.";
        assert_eq!(anonymise_first_name(note, ""), "Some note body.");
        assert_eq!(anonymise_first_name(note, "   "), "Some note body.");
    }

    #[test]
    fn test_anonymise_handles_apostrophe_edge_case() {
        // "Emma's Emma" — both forms present
        let note = "Emma's tone shifted when Emma paused.";
        assert_eq!(
            anonymise_first_name(note, "Emma"),
            "Client's tone shifted when Client paused."
        );
    }
}
