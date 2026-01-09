use anyhow::{Context, Result};
use regex::Regex;
use std::fs;

use crate::changelog;
use crate::scrolls::{read_scroll, scrolls_dir, write_scroll};

/// Run the import command
pub fn run(file: &str, dry_run: bool) -> Result<()> {
    let content = fs::read_to_string(file)
        .with_context(|| format!("Failed to read file: {}", file))?;

    // Try to parse as JSON first (continuum log format)
    let text = if content.trim().starts_with('{') || content.trim().starts_with('[') {
        extract_text_from_json(&content)?
    } else {
        content
    };

    // Extract module updates
    let updates = extract_module_updates(&text)?;

    if updates.is_empty() {
        println!("No module updates found in file.");
        println!();
        println!("Expected format:");
        println!("  # BEGIN MODULE UPDATE: WILLIAM-LIFESTYLE.md");
        println!("  [module content]");
        println!("  # END MODULE UPDATE");
        return Ok(());
    }

    println!("Found {} module update(s):", updates.len());
    for (name, _) in &updates {
        println!("  • {}", name);
    }
    println!();

    // Extract changelog entry
    let changelog_entry = extract_changelog_entry(&text)?;

    if dry_run {
        println!("DRY RUN - would apply:");
        for (name, content) in &updates {
            let current = read_scroll(name).unwrap_or_default();
            let diff_lines = simple_diff(&current, content);
            println!();
            println!("{}:", name);
            println!("  {} lines changed", diff_lines);
        }
        if let Some(entry) = &changelog_entry {
            println!();
            println!("Changelog entry:");
            for line in entry.lines().take(10) {
                println!("  {}", line);
            }
        }
    } else {
        // Apply updates
        for (name, content) in &updates {
            write_scroll(name, content)?;
            println!("✓ Updated {}", name);
        }

        // Apply changelog
        if let Some(entry) = changelog_entry {
            changelog::append_entry(&entry)?;
            println!("✓ Appended to WILLIAM-CHANGELOG.md");
        } else {
            // Auto-generate changelog entry
            let auto_entry = changelog::generate_entry(&updates)?;
            changelog::append_entry(&auto_entry)?;
            println!("✓ Auto-generated changelog entry");
        }

        println!();
        println!("Import complete. Scrolls updated at: {}", scrolls_dir().display());
    }

    Ok(())
}

/// Extract text content from JSON (continuum log format)
fn extract_text_from_json(json: &str) -> Result<String> {
    // Try to parse as a single object with "content" field
    if let Ok(obj) = serde_json::from_str::<serde_json::Value>(json) {
        if let Some(content) = obj.get("content").and_then(|v| v.as_str()) {
            return Ok(content.to_string());
        }
        // Try messages array
        if let Some(messages) = obj.get("messages").and_then(|v| v.as_array()) {
            let text: Vec<String> = messages
                .iter()
                .filter_map(|m| m.get("content").and_then(|c| c.as_str()))
                .map(|s| s.to_string())
                .collect();
            return Ok(text.join("\n\n"));
        }
    }

    // Try as array of messages
    if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(json) {
        let text: Vec<String> = arr
            .iter()
            .filter_map(|m| m.get("content").and_then(|c| c.as_str()))
            .map(|s| s.to_string())
            .collect();
        return Ok(text.join("\n\n"));
    }

    // Fall back to treating as plain text
    Ok(json.to_string())
}

/// Extract module updates from conversation text
fn extract_module_updates(text: &str) -> Result<Vec<(String, String)>> {
    let re = Regex::new(
        r"(?s)#\s*BEGIN\s+MODULE\s+UPDATE:\s*(\S+\.md)\s*\n(.*?)#\s*END\s+MODULE\s+UPDATE"
    )?;

    let mut updates = Vec::new();
    for cap in re.captures_iter(text) {
        let name = cap[1].to_string();
        let content = cap[2].trim().to_string();
        updates.push((name, content));
    }

    Ok(updates)
}

/// Extract changelog entry from conversation text
fn extract_changelog_entry(text: &str) -> Result<Option<String>> {
    let re = Regex::new(
        r"(?s)#\s*BEGIN\s+CHANGELOG\s+ENTRY\s*\n(.*?)#\s*END\s+CHANGELOG\s+ENTRY"
    )?;

    if let Some(cap) = re.captures(text) {
        Ok(Some(cap[1].trim().to_string()))
    } else {
        Ok(None)
    }
}

/// Simple line-count diff for dry run display
fn simple_diff(old: &str, new: &str) -> usize {
    let old_lines: std::collections::HashSet<_> = old.lines().collect();
    let new_lines: std::collections::HashSet<_> = new.lines().collect();

    let added = new_lines.difference(&old_lines).count();
    let removed = old_lines.difference(&new_lines).count();

    added + removed
}
