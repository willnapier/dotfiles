use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use crate::types::{MemoryFile, MemoryFrontmatter, MemoryState};

/// Default memory directory
pub fn memory_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("No home directory")?;
    Ok(home.join(".claude/projects/-Users-williamnapier/memory"))
}

/// Scan the memory directory and return the full state
pub fn scan_memory() -> Result<MemoryState> {
    let dir = memory_dir()?;
    let index_path = dir.join("MEMORY.md");

    // Read MEMORY.md
    let index_content = if index_path.exists() {
        fs::read_to_string(&index_path).context("Failed to read MEMORY.md")?
    } else {
        String::new()
    };
    let index_line_count = index_content.lines().count();

    // Extract file references from MEMORY.md
    let index_refs = extract_index_refs(&index_content);

    // Read all memory files (excluding MEMORY.md)
    let mut files = Vec::new();
    let mut file_names: HashSet<String> = HashSet::new();

    if dir.exists() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let filename = entry.file_name().to_string_lossy().to_string();
            if filename == "MEMORY.md" || !filename.ends_with(".md") {
                continue;
            }

            match parse_memory_file(&path) {
                Ok(mem_file) => {
                    file_names.insert(filename.clone());
                    files.push(mem_file);
                }
                Err(e) => {
                    eprintln!("Warning: failed to parse {}: {}", filename, e);
                }
            }
        }
    }

    // Find orphaned index refs (referenced in MEMORY.md but file doesn't exist)
    let orphaned_index_refs: Vec<String> = index_refs
        .iter()
        .filter(|r| !file_names.contains(*r))
        .cloned()
        .collect();

    // Find unindexed files (file exists but not referenced in MEMORY.md)
    let ref_set: HashSet<&String> = index_refs.iter().collect();
    let unindexed_files: Vec<String> = file_names
        .iter()
        .filter(|f| !ref_set.contains(f))
        .cloned()
        .collect();

    Ok(MemoryState {
        memory_dir: dir,
        index_path,
        index_content,
        index_line_count,
        files,
        orphaned_index_refs,
        unindexed_files,
    })
}

/// Extract filenames referenced in MEMORY.md as markdown links: [Title](filename.md)
fn extract_index_refs(content: &str) -> Vec<String> {
    let re = Regex::new(r"\[[^\]]+\]\(([^)]+\.md)\)").unwrap();
    re.captures_iter(content)
        .map(|c| c[1].to_string())
        .collect()
}

/// Parse a memory file into a MemoryFile struct
fn parse_memory_file(path: &PathBuf) -> Result<MemoryFile> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let filename = path
        .file_name()
        .context("No filename")?
        .to_string_lossy()
        .to_string();

    let (frontmatter, body) = parse_frontmatter(&content)
        .with_context(|| format!("Failed to parse frontmatter in {}", filename))?;

    let line_count = content.lines().count();

    Ok(MemoryFile {
        path: path.clone(),
        filename,
        frontmatter,
        body,
        line_count,
    })
}

/// Split content into YAML frontmatter and markdown body
fn parse_frontmatter(content: &str) -> Result<(MemoryFrontmatter, String)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        anyhow::bail!("No frontmatter delimiter found");
    }

    // Find the closing ---
    let after_first = &trimmed[3..];
    let close_pos = after_first
        .find("\n---")
        .context("No closing frontmatter delimiter")?;

    let yaml_str = after_first[..close_pos].trim();
    let body_start = close_pos + 4; // skip \n---
    let body = if body_start < after_first.len() {
        after_first[body_start..].trim_start_matches('\n').to_string()
    } else {
        String::new()
    };

    // Try serde_yaml first, fall back to regex extraction
    match serde_yaml::from_str::<MemoryFrontmatter>(yaml_str) {
        Ok(fm) => Ok((fm, body)),
        Err(_) => {
            // Regex fallback for files with YAML-unfriendly characters
            let name = extract_field(yaml_str, "name")
                .context("missing 'name' field")?;
            let description = extract_field(yaml_str, "description")
                .context("missing 'description' field")?;
            let memory_type = extract_field(yaml_str, "type")
                .context("missing 'type' field")?;

            Ok((
                MemoryFrontmatter {
                    name,
                    description,
                    memory_type,
                },
                body,
            ))
        }
    }
}

/// Extract a field value from YAML-like text using simple line matching
fn extract_field(yaml: &str, field: &str) -> Option<String> {
    let prefix = format!("{}: ", field);
    for line in yaml.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix(&prefix) {
            // Strip surrounding quotes if present
            let value = value.trim();
            let value = if (value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\''))
            {
                &value[1..value.len() - 1]
            } else {
                value
            };
            return Some(value.to_string());
        }
    }
    None
}

/// Format the memory state as a context string for the AI prompt
pub fn format_memory_state(state: &MemoryState) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "## Index (MEMORY.md) - {} lines\n",
        state.index_line_count
    ));
    out.push_str(&state.index_content);
    out.push_str("\n\n## Memory Files\n\n");

    for file in &state.files {
        out.push_str(&format!(
            "### {} ({}, {} lines)\n",
            file.filename, file.frontmatter.memory_type, file.line_count
        ));
        out.push_str("---\n");
        out.push_str(&format!("name: {}\n", file.frontmatter.name));
        out.push_str(&format!("description: {}\n", file.frontmatter.description));
        out.push_str(&format!("type: {}\n", file.frontmatter.memory_type));
        out.push_str("---\n\n");
        out.push_str(&file.body);
        out.push_str("\n\n");
    }

    if !state.orphaned_index_refs.is_empty() {
        out.push_str("## Integrity Issues\n\n");
        out.push_str("Orphaned index references (MEMORY.md links to files that don't exist):\n");
        for r in &state.orphaned_index_refs {
            out.push_str(&format!("- {}\n", r));
        }
        out.push('\n');
    }

    if !state.unindexed_files.is_empty() {
        if state.orphaned_index_refs.is_empty() {
            out.push_str("## Integrity Issues\n\n");
        }
        out.push_str("Unindexed files (exist but not referenced in MEMORY.md):\n");
        for f in &state.unindexed_files {
            out.push_str(&format!("- {}\n", f));
        }
        out.push('\n');
    }

    out
}
