use anyhow::{Context, Result};
use regex::Regex;
use std::path::{Path, PathBuf};

use crate::client;
use crate::identity;
use crate::markdown;
use crate::session;

/// A pending field change for a client file.
#[derive(Debug)]
struct FieldChange {
    field: String,
    action: Action,
    value: String,
    old: Option<String>,
}

#[derive(Debug)]
enum Action {
    Add,
    Update,
}

/// Pending changes for one client.
struct ClientChanges {
    id: String,
    file: PathBuf,
    changes: Vec<FieldChange>,
}

/// Run `clinical populate`.
pub fn run(apply: bool) -> Result<()> {
    let clients_dir = client::clients_dir();
    if !clients_dir.exists() {
        println!("No clients directory found.");
        return Ok(());
    }

    let mut all_changes: Vec<ClientChanges> = Vec::new();
    let mut skipped = 0u32;

    let entries = list_client_dirs(&clients_dir)?;

    for (client_id, client_dir) in &entries {
        let client_file = find_client_md(client_dir, client_id);
        let client_file = match client_file {
            Some(f) => f,
            None => {
                skipped += 1;
                continue;
            }
        };

        let content = std::fs::read_to_string(&client_file)
            .with_context(|| format!("Failed to read: {}", client_file.display()))?;

        let mut changes = Vec::new();

        // --- SESSION COUNT ---
        compute_session_count_change(&content, &mut changes);

        // --- LAST UPDATE LETTER ---
        compute_last_update_change(&content, client_dir, client_id, &mut changes);

        // --- REFERRING DOCTOR (from identity.yaml) ---
        compute_referring_doctor_change(&content, client_dir, &mut changes);

        // --- REFERRAL TYPE (inferred) ---
        compute_referral_type_change(&content, client_dir, client_id, &mut changes);

        if changes.is_empty() {
            skipped += 1;
        } else {
            all_changes.push(ClientChanges {
                id: client_id.clone(),
                file: client_file,
                changes,
            });
        }
    }

    if all_changes.is_empty() {
        println!("No changes needed.");
        return Ok(());
    }

    // Print summary
    for client in &all_changes {
        println!("  {}:", client.id);
        for change in &client.changes {
            let prefix = match change.action {
                Action::Add => "  + ",
                Action::Update => "  ~ ",
            };
            let old_label = match &change.old {
                Some(old) => format!(" (was: {})", old),
                None => String::new(),
            };
            println!(
                "{}**{}**: {}{}",
                prefix, change.field, change.value, old_label
            );
        }
    }

    println!();
    let change_count = all_changes.len();

    if apply {
        for client in &all_changes {
            let content = std::fs::read_to_string(&client.file)
                .with_context(|| format!("Failed to read: {}", client.file.display()))?;
            let mut lines: Vec<String> = content.lines().map(String::from).collect();

            for change in &client.changes {
                match change.action {
                    Action::Update => {
                        lines = markdown::update_field(&lines, &change.field, &change.value);
                    }
                    Action::Add => {
                        lines = markdown::insert_field_after_last(
                            &lines,
                            &change.field,
                            &change.value,
                        );
                    }
                }
            }

            let result = lines.join("\n");
            std::fs::write(&client.file, result)
                .with_context(|| format!("Failed to write: {}", client.file.display()))?;
            println!("  Applied: {}", client.id);
        }
        println!();
        println!(
            "Done. {} files modified, {} unchanged.",
            change_count, skipped
        );
    } else {
        println!(
            "Dry run. {} files would be modified, {} unchanged.",
            change_count, skipped
        );
        println!("Run with --apply to modify files.");
    }

    Ok(())
}

/// List client directories under the clients dir.
fn list_client_dirs(clients_dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut results = Vec::new();

    for entry in std::fs::read_dir(clients_dir)
        .with_context(|| format!("Failed to read: {}", clients_dir.display()))?
    {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            results.push((name, entry.path()));
        }
    }

    results.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(results)
}

/// Find the client .md file, handling lowercase filename edge cases.
fn find_client_md(client_dir: &Path, client_id: &str) -> Option<PathBuf> {
    let primary = client_dir.join(format!("{}.md", client_id));
    if primary.exists() {
        return Some(primary);
    }

    let lower = client_dir.join(format!("{}.md", client_id.to_lowercase()));
    if lower.exists() {
        return Some(lower);
    }

    None
}

/// Extract field value treating "null", "none yet", and empty as None.
fn extract_field_or_none(content: &str, field: &str) -> Option<String> {
    match markdown::extract_field(content, field) {
        Some(v) if v == "null" || v == "none yet" || v.is_empty() => None,
        other => other,
    }
}

/// Compute session count changes.
fn compute_session_count_change(content: &str, changes: &mut Vec<FieldChange>) {
    let existing = markdown::extract_field(content, "Session count");
    let lines: Vec<&str> = content.lines().collect();
    let session_section_idx = match session::find_session_section(&lines) {
        Some(idx) => idx,
        None => return,
    };

    let session_lines = &lines[(session_section_idx + 1)..];
    let actual_count = session::count_sessions(session_lines);

    if actual_count == 0 {
        return;
    }

    // Count DNAs
    let dna_re = Regex::new(r"(?i)^### \d{4}-\d{2}-\d{2}.*DNA").unwrap();
    let dna_count = session_lines
        .iter()
        .filter(|l| dna_re.is_match(l))
        .count() as u32;

    let display = if dna_count > 0 {
        format!("{} (incl {} DNA)", actual_count, dna_count)
    } else {
        actual_count.to_string()
    };

    match &existing {
        None => {
            changes.push(FieldChange {
                field: "Session count".to_string(),
                action: Action::Add,
                value: display,
                old: None,
            });
        }
        Some(existing_val) => {
            // Don't overwrite tracking/estimated markers
            if existing_val.contains("tracking") || existing_val.contains("estimated") {
                return;
            }
            // Extract numeric part from existing
            let num_re = Regex::new(r"(\d+)").unwrap();
            let existing_num: u32 = num_re
                .captures(existing_val)
                .and_then(|c| c[1].parse().ok())
                .unwrap_or(0);

            if existing_num != actual_count {
                changes.push(FieldChange {
                    field: "Session count".to_string(),
                    action: Action::Update,
                    value: display,
                    old: Some(existing_val.clone()),
                });
            }
        }
    }
}

/// Compute last update letter changes from update file dates.
fn compute_last_update_change(
    content: &str,
    client_dir: &Path,
    client_id: &str,
    changes: &mut Vec<FieldChange>,
) {
    let existing = extract_field_or_none(content, "Last update letter");

    // Look for *-[ID]-update.md files
    let pattern = format!("*-{}-update.md", client_id);
    let glob_pattern = client_dir.join(&pattern);
    let mut update_files: Vec<PathBuf> = glob::glob(glob_pattern.to_str().unwrap_or(""))
        .into_iter()
        .flatten()
        .filter_map(|r| r.ok())
        .collect();
    update_files.sort();

    if update_files.is_empty() {
        return;
    }

    let latest_file = update_files.last().unwrap();
    let latest_name = latest_file
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    let date_re = Regex::new(r"^(\d{4}-\d{2}-\d{2})").unwrap();
    let latest_date = match date_re.captures(&latest_name) {
        Some(caps) => caps[1].to_string(),
        None => return,
    };

    match &existing {
        None => {
            changes.push(FieldChange {
                field: "Last update letter".to_string(),
                action: Action::Update,
                value: latest_date,
                old: Some("missing".to_string()),
            });
        }
        Some(existing_val) => {
            if latest_date > *existing_val {
                changes.push(FieldChange {
                    field: "Last update letter".to_string(),
                    action: Action::Update,
                    value: latest_date,
                    old: Some(existing_val.clone()),
                });
            }
        }
    }
}

/// Compute referring doctor changes from identity.yaml.
fn compute_referring_doctor_change(
    content: &str,
    client_dir: &Path,
    changes: &mut Vec<FieldChange>,
) {
    let existing = markdown::extract_field(content, "Referring doctor");
    if existing.is_some() {
        return; // Already populated, don't overwrite
    }

    let id_file = client_dir.join("private").join("identity.yaml");
    if !id_file.exists() {
        return;
    }

    let ident = match identity::load_identity(&id_file) {
        Ok(id) => id,
        Err(_) => return,
    };

    let ref_name = match &ident.referrer.name {
        Some(n) if !n.is_empty() => n.clone(),
        _ => return,
    };

    let mut parts = vec![ref_name];
    if let Some(role) = &ident.referrer.role {
        if !role.is_empty() {
            parts.push(role.clone());
        }
    }
    if let Some(practice) = &ident.referrer.practice {
        if !practice.is_empty() {
            parts.push(practice.clone());
        }
    }

    let value = parts.join(", ");
    changes.push(FieldChange {
        field: "Referring doctor".to_string(),
        action: Action::Add,
        value,
        old: None,
    });
}

/// Compute referral type changes (inferred from identity.yaml or referral file existence).
fn compute_referral_type_change(
    content: &str,
    client_dir: &Path,
    client_id: &str,
    changes: &mut Vec<FieldChange>,
) {
    let existing = markdown::extract_field(content, "Referral type");

    // Only populate if missing or placeholder
    match &existing {
        Some(val)
            if val != "[to confirm]"
                && val != "[To confirm]"
                && !val.is_empty()
                && val != "null" =>
        {
            return;
        }
        _ => {}
    }

    let inferred = infer_referral_type(client_dir, client_id);
    let inferred = match inferred {
        Some(t) => t,
        None => return,
    };

    let action = if existing.is_none() {
        Action::Add
    } else {
        Action::Update
    };

    changes.push(FieldChange {
        field: "Referral type".to_string(),
        action,
        value: inferred,
        old: existing.or_else(|| Some("missing".to_string())),
    });
}

/// Infer referral type from identity.yaml referrer role or referral file existence.
fn infer_referral_type(client_dir: &Path, client_id: &str) -> Option<String> {
    let id_file = client_dir.join("private").join("identity.yaml");
    if id_file.exists() {
        if let Ok(ident) = identity::load_identity(&id_file) {
            if let Some(role) = &ident.referrer.role {
                let lower = role.to_lowercase();
                if lower.contains("gp")
                    || lower.contains("general pract")
                    || lower.contains("company gp")
                    || lower.contains("psychiatr")
                    || lower.contains("consultant")
                    || lower.contains("specialist")
                {
                    return Some("doctor".to_string());
                }
            }
        }
    }

    // Check for referral letter existence
    let pattern = format!("*-{}-referral.md", client_id);
    let glob_pattern = client_dir.join(&pattern);
    let referral_files: Vec<_> = glob::glob(glob_pattern.to_str().unwrap_or(""))
        .into_iter()
        .flatten()
        .filter_map(|r| r.ok())
        .collect();

    if !referral_files.is_empty() {
        return Some("doctor".to_string());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_count_new() {
        let content = "\
# TEST

**Funding**: AXA

## Session Notes

### 2026-01-15
Notes.

### 2026-01-22
More notes.
";
        let mut changes = Vec::new();
        compute_session_count_change(content, &mut changes);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].field, "Session count");
        assert_eq!(changes[0].value, "2");
    }

    #[test]
    fn test_session_count_update() {
        let content = "\
# TEST

**Session count**: 3

## Session Notes

### 2026-01-15

### 2026-01-22

### 2026-01-29

### 2026-02-05
";
        let mut changes = Vec::new();
        compute_session_count_change(content, &mut changes);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].value, "4");
        assert!(matches!(changes[0].old.as_deref(), Some("3")));
    }

    #[test]
    fn test_session_count_no_change() {
        let content = "\
# TEST

**Session count**: 2

## Session Notes

### 2026-01-15

### 2026-01-22
";
        let mut changes = Vec::new();
        compute_session_count_change(content, &mut changes);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_session_count_with_dna() {
        let content = "\
# TEST

## Session Notes

### 2026-01-15

### 2026-01-22 DNA

### 2026-01-29
";
        let mut changes = Vec::new();
        compute_session_count_change(content, &mut changes);
        assert_eq!(changes[0].value, "3 (incl 1 DNA)");
    }

    #[test]
    fn test_session_count_tracking_not_overwritten() {
        let content = "\
# TEST

**Session count**: tracking from 2025-06-01

## Session Notes

### 2026-01-15
";
        let mut changes = Vec::new();
        compute_session_count_change(content, &mut changes);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_extract_field_or_none() {
        assert_eq!(
            extract_field_or_none("**Last update letter**: none yet\n", "Last update letter"),
            None
        );
        assert_eq!(
            extract_field_or_none("**Last update letter**: 2026-01-15\n", "Last update letter"),
            Some("2026-01-15".to_string())
        );
    }
}
