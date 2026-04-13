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

    // 5. Ensure admin/drafts/ and admin/letters/ exist
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
