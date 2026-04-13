use anyhow::{bail, Context, Result};
use regex::Regex;
use clinical_core::client;

/// Migrate a client from Route A (private/ layout) to Route C (flat layout).
///
/// Steps:
/// 1. Move identity.yaml from private/ to root (strip redaction fields)
/// 2. Rename <id>.md to notes.md
/// 3. Move date-prefixed correspondence files to correspondence/
/// 4. Rename private/ to admin/
/// 5. Ensure admin/drafts/ and admin/letters/ exist
pub fn run(id: &str, dry_run: bool) -> Result<()> {
    let dir = client::client_dir(id);
    let private = dir.join("private");

    if !dir.exists() {
        bail!("Client directory not found: {}", dir.display());
    }

    if !private.exists() {
        eprintln!("{}: already Route C (no private/ directory), skipping.", id);
        return Ok(());
    }

    eprintln!("Migrating {} from Route A → Route C{}...", id, if dry_run { " (dry run)" } else { "" });

    // 1. Move identity.yaml to root, strip redaction fields
    let old_identity = private.join("identity.yaml");
    let new_identity = dir.join("identity.yaml");
    if old_identity.exists() {
        if dry_run {
            eprintln!("  Would move: private/identity.yaml → identity.yaml (strip redactions/aliases)");
        } else {
            let content = std::fs::read_to_string(&old_identity)
                .context("Failed to read identity.yaml")?;
            let cleaned = strip_redaction_fields(&content);
            std::fs::write(&new_identity, cleaned)
                .context("Failed to write identity.yaml to root")?;
            std::fs::remove_file(&old_identity)
                .context("Failed to remove old identity.yaml")?;
            eprintln!("  Moved: private/identity.yaml → identity.yaml");
        }
    }

    // 2. Rename <id>.md to notes.md
    let old_notes = dir.join(format!("{}.md", id));
    let new_notes = dir.join("notes.md");
    if old_notes.exists() {
        if dry_run {
            eprintln!("  Would rename: {}.md → notes.md", id);
        } else {
            std::fs::rename(&old_notes, &new_notes)
                .with_context(|| format!("Failed to rename {}.md to notes.md", id))?;
            eprintln!("  Renamed: {}.md → notes.md", id);
        }
    }

    // 3. Move date-prefixed correspondence to correspondence/
    let correspondence = dir.join("correspondence");
    let corr_re = Regex::new(r"^\d{4}-\d{2}-\d{2}-.+\.(md|txt)$").unwrap();

    let corr_files: Vec<_> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            corr_re.is_match(&name)
        })
        .collect();

    if !corr_files.is_empty() {
        if dry_run {
            eprintln!("  Would create: correspondence/");
            for f in &corr_files {
                let name = f.file_name().to_string_lossy().to_string();
                eprintln!("  Would move: {} → correspondence/{}", name, name);
            }
        } else {
            std::fs::create_dir_all(&correspondence)
                .context("Failed to create correspondence/")?;
            for f in &corr_files {
                let name = f.file_name().to_string_lossy().to_string();
                let dest = correspondence.join(&name);
                std::fs::rename(f.path(), &dest)
                    .with_context(|| format!("Failed to move {}", name))?;
                eprintln!("  Moved: {} → correspondence/{}", name, name);
            }
        }
    }

    // 4. Rename private/ to admin/
    let admin = dir.join("admin");
    if dry_run {
        eprintln!("  Would rename: private/ → admin/");
    } else {
        std::fs::rename(&private, &admin)
            .context("Failed to rename private/ to admin/")?;
        eprintln!("  Renamed: private/ → admin/");
    }

    // 5. Clean correspondence: remove de-identified copies, strip -private suffix
    clean_correspondence(id, dry_run)?;

    // 6. Ensure admin/drafts/ and admin/letters/ exist
    let admin_path = if dry_run { &private } else { &admin };
    for subdir in ["drafts", "letters"] {
        let d = admin_path.join(subdir);
        if !d.exists() {
            if dry_run {
                eprintln!("  Would create: admin/{}/", subdir);
            } else {
                let real_d = admin.join(subdir);
                std::fs::create_dir_all(&real_d)
                    .with_context(|| format!("Failed to create admin/{}", subdir))?;
                eprintln!("  Created: admin/{}/", subdir);
            }
        }
    }

    eprintln!("  Done.");
    Ok(())
}

/// Migrate all Route A clients to Route C.
pub fn run_all(dry_run: bool) -> Result<()> {
    let ids = client::list_client_ids()?;
    let route_a_count = ids.iter()
        .filter(|id| client::client_dir(id).join("private").exists())
        .count();

    eprintln!("Found {} Route A clients to migrate{}.",
        route_a_count,
        if dry_run { " (dry run)" } else { "" }
    );

    for id in &ids {
        if client::client_dir(id).join("private").exists() {
            run(id, dry_run)?;
        }
    }

    eprintln!("\n{} clients processed.", route_a_count);
    Ok(())
}

/// Remove de-identified correspondence copies where a real-name (-private) version exists.
///
/// Pattern: `2025-11-25-AB79-referral.md` is the de-identified copy,
/// `2025-11-25-AB79-mcguigan-referral-private.md` is the real-name original.
/// After migration, the de-identified copy is redundant. This also strips the
/// `-private` suffix from the real-name files since the distinction no longer applies.
pub fn clean_correspondence(id: &str, dry_run: bool) -> Result<()> {
    let corr_dir = client::correspondence_dir(id);
    if !corr_dir.exists() {
        return Ok(());
    }

    let files: Vec<String> = std::fs::read_dir(&corr_dir)?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();

    let private_files: Vec<&String> = files.iter()
        .filter(|f| f.ends_with("-private.md") || f.ends_with("-private.txt"))
        .collect();

    if private_files.is_empty() {
        return Ok(());
    }

    // For each -private file, find its de-identified counterpart
    let date_id_re = Regex::new(r"^(\d{4}-\d{2}-\d{2}-[A-Z+]+\d*-)").unwrap();

    for priv_file in &private_files {
        // Extract the date-ID prefix and type suffix from the private file
        if let Some(caps) = date_id_re.captures(priv_file) {
            let prefix = &caps[1];
            // The type is the last segment before -private.ext
            // e.g. "2025-11-25-AB79-mcguigan-referral-private.md" → type is "referral"
            let without_ext = priv_file.trim_end_matches(".md").trim_end_matches(".txt");
            let without_private = without_ext.trim_end_matches("-private");
            let type_suffix = without_private.rsplit('-').next().unwrap_or("");

            // Look for de-identified counterpart: prefix + type + .ext
            let ext = if priv_file.ends_with(".txt") { ".txt" } else { ".md" };
            let deident_name = format!("{}{}{}", prefix, type_suffix, ext);

            if files.contains(&deident_name) {
                if dry_run {
                    eprintln!("  Would remove de-identified copy: {}", deident_name);
                } else {
                    std::fs::remove_file(corr_dir.join(&deident_name))
                        .with_context(|| format!("Failed to remove {}", deident_name))?;
                    eprintln!("  Removed de-identified copy: {}", deident_name);
                }
            }

            // Rename -private file to drop the suffix
            let clean_name = priv_file
                .replace("-private.md", ".md")
                .replace("-private.txt", ".txt");
            if dry_run {
                eprintln!("  Would rename: {} → {}", priv_file, clean_name);
            } else {
                std::fs::rename(
                    corr_dir.join(priv_file.as_str()),
                    corr_dir.join(&clean_name),
                ).with_context(|| format!("Failed to rename {}", priv_file))?;
                eprintln!("  Renamed: {} → {}", priv_file, clean_name);
            }
        }
    }

    Ok(())
}

/// Clean correspondence for all clients.
pub fn clean_correspondence_all(dry_run: bool) -> Result<()> {
    let ids = client::list_client_ids()?;
    for id in &ids {
        let corr_dir = client::correspondence_dir(id);
        if corr_dir.exists() {
            clean_correspondence(id, dry_run)?;
        }
    }
    Ok(())
}

/// Replace "Client" with the client's first name in notes.md.
///
/// Reverses the de-identification that replaced real names with "Client".
/// Reads the first name from identity.yaml.
pub fn personalize(id: &str, dry_run: bool) -> Result<()> {
    let identity_path = client::identity_path(id);
    if !identity_path.exists() {
        eprintln!("{}: no identity.yaml, skipping.", id);
        return Ok(());
    }

    let identity = match clinical_core::identity::load_identity(&identity_path) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("{}: identity.yaml parse error ({}), skipping.", id, e);
            return Ok(());
        }
    };
    let name = identity.name.as_deref().unwrap_or("").trim().to_string();
    if name.is_empty() {
        eprintln!("{}: no name in identity.yaml, skipping.", id);
        return Ok(());
    }

    let first_name = name.split_whitespace().next().unwrap_or(&name);

    let notes_path = client::notes_path(id);
    if !notes_path.exists() {
        eprintln!("{}: no notes file, skipping.", id);
        return Ok(());
    }

    let content = std::fs::read_to_string(&notes_path)
        .with_context(|| format!("Failed to read {}", notes_path.display()))?;

    // Build replacements — order matters (longest first to avoid partial matches)
    let possessive = format!("{}'s", first_name);
    let first = first_name.to_string();
    let replacements: Vec<(&str, &str)> = vec![
        ("the Client's", &possessive),
        ("The Client's", &possessive),
        ("the client's", &possessive),
        ("The client's", &possessive),
        ("Client's", &possessive),
        ("the Client", &first),
        ("The Client", &first),
        ("the client", &first),
        ("The client", &first),
        ("Client", &first),
    ];

    let mut result = content.clone();
    let mut total_count = 0usize;

    for (find, replace) in &replacements {
        let count = result.matches(*find).count();
        if count > 0 {
            result = result.replace(find, replace);
            total_count += count;
        }
    }

    if total_count == 0 {
        eprintln!("{}: no 'Client' references found.", id);
        return Ok(());
    }

    if dry_run {
        eprintln!("{}: would replace {} occurrences of 'Client' → '{}'", id, total_count, first_name);
    } else {
        std::fs::write(&notes_path, &result)
            .with_context(|| format!("Failed to write {}", notes_path.display()))?;
        eprintln!("{}: replaced {} occurrences → '{}'", id, total_count, first_name);
    }

    Ok(())
}

/// Personalize all clients.
pub fn personalize_all(dry_run: bool) -> Result<()> {
    let ids = client::list_client_ids()?;
    for id in &ids {
        personalize(id, dry_run)?;
    }
    Ok(())
}

/// Strip redaction-specific fields from identity.yaml content.
///
/// Removes `redactions:` block and `aliases:` list since these are
/// Route A de-identification artifacts. Preserves all other metadata.
fn strip_redaction_fields(content: &str) -> String {
    let mut out = Vec::new();
    let mut skip_block = false;

    for line in content.lines() {
        // Start skipping on redactions: or aliases: top-level keys
        if line.starts_with("redactions:") || line.starts_with("aliases:") {
            skip_block = true;
            continue;
        }

        // Stop skipping when we hit the next top-level key or blank line after a block
        if skip_block {
            if !line.starts_with(' ') && !line.starts_with('-') && !line.is_empty() {
                // New top-level key — stop skipping
                skip_block = false;
            } else {
                continue;
            }
        }

        out.push(line);
    }

    let mut result = out.join("\n");
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_redaction_fields() {
        let input = "\
name: Jane Smith
title: Ms
aliases:
  - Jenny Smith
  - J. Smith
dob: 1985-03-14
redactions:
  - find: SomeOrg
    replace: their organisation
funding:
  funding_type: self-pay
";
        let result = strip_redaction_fields(input);
        assert!(result.contains("name: Jane Smith"));
        assert!(result.contains("title: Ms"));
        assert!(result.contains("dob: 1985-03-14"));
        assert!(result.contains("funding:"));
        assert!(!result.contains("aliases"));
        assert!(!result.contains("Jenny Smith"));
        assert!(!result.contains("redactions"));
        assert!(!result.contains("SomeOrg"));
    }

    #[test]
    fn test_strip_preserves_content_without_redactions() {
        let input = "\
name: John
dob: 1990-01-01
funding:
  funding_type: insurer
";
        let result = strip_redaction_fields(input);
        assert_eq!(result, input);
    }
}
