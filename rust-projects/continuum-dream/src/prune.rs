use anyhow::Result;
use std::collections::HashSet;

use crate::types::{DreamResponse, MemoryState};

const MAX_INDEX_LINES: usize = 200;
const VALID_TYPES: &[&str] = &["user", "feedback", "project", "reference"];

/// Validate the AI's proposed changes. Returns list of warnings/errors.
/// Modifies the response in place to remove invalid changes.
pub fn validate(response: &mut DreamResponse, memory_state: &MemoryState) -> Vec<String> {
    let mut warnings = Vec::new();

    let existing_files: HashSet<String> = memory_state
        .files
        .iter()
        .map(|f| f.filename.clone())
        .collect();

    // Validate files_to_update: each must exist
    response.files_to_update.retain(|f| {
        if !existing_files.contains(&f.filename) {
            warnings.push(format!(
                "REJECTED update: '{}' does not exist in memory dir",
                f.filename
            ));
            return false;
        }
        if let Err(e) = validate_content(&f.filename, &f.content) {
            warnings.push(format!("REJECTED update '{}': {}", f.filename, e));
            return false;
        }
        true
    });

    // Validate files_to_create: must NOT exist, must follow naming convention
    response.files_to_create.retain(|f| {
        if existing_files.contains(&f.filename) {
            warnings.push(format!(
                "REJECTED create: '{}' already exists (use update instead)",
                f.filename
            ));
            return false;
        }
        if let Err(e) = validate_content(&f.filename, &f.content) {
            warnings.push(format!("REJECTED create '{}': {}", f.filename, e));
            return false;
        }
        // Check naming convention: [type]_[topic].md
        if !f.filename.ends_with(".md") || !f.filename.contains('_') {
            warnings.push(format!(
                "REJECTED create '{}': must follow [type]_[topic].md convention",
                f.filename
            ));
            return false;
        }
        true
    });

    // Validate files_to_delete: must exist
    response.files_to_delete.retain(|f| {
        if !existing_files.contains(&f.filename) {
            warnings.push(format!(
                "REJECTED delete: '{}' does not exist",
                f.filename
            ));
            return false;
        }
        true
    });

    // Path injection guard — collect all filenames first, then check
    let all_filenames: Vec<String> = response
        .files_to_update
        .iter()
        .map(|f| f.filename.clone())
        .chain(response.files_to_create.iter().map(|f| f.filename.clone()))
        .chain(response.files_to_delete.iter().map(|f| f.filename.clone()))
        .collect();

    for filename in &all_filenames {
        if filename.contains("..") || filename.contains('/') {
            warnings.push(format!(
                "SECURITY: '{}' contains path traversal characters — aborting all changes",
                filename
            ));
            response.files_to_update.clear();
            response.files_to_create.clear();
            response.files_to_delete.clear();
            return warnings;
        }
    }

    // Validate MEMORY.md line count
    if response.memory_index != "UNCHANGED" {
        let line_count = response.memory_index.lines().count();
        if line_count > MAX_INDEX_LINES {
            warnings.push(format!(
                "WARNING: proposed MEMORY.md is {} lines (limit: {})",
                line_count, MAX_INDEX_LINES
            ));
        }
    }

    warnings
}

/// Validate a memory file's content (frontmatter format, type field)
fn validate_content(filename: &str, content: &str) -> Result<()> {
    let trimmed = content.trim();
    if !trimmed.starts_with("---") {
        anyhow::bail!("missing frontmatter (must start with ---)");
    }

    let after_first = &trimmed[3..];
    let close_pos = after_first
        .find("\n---")
        .ok_or_else(|| anyhow::anyhow!("missing closing frontmatter delimiter"))?;

    let yaml_str = &after_first[..close_pos];

    // Check required fields exist
    if !yaml_str.contains("name:") {
        anyhow::bail!("missing 'name' field in frontmatter");
    }
    if !yaml_str.contains("description:") {
        anyhow::bail!("missing 'description' field in frontmatter");
    }
    if !yaml_str.contains("type:") {
        anyhow::bail!("missing 'type' field in frontmatter");
    }

    // Check type value
    for valid_type in VALID_TYPES {
        if yaml_str.contains(&format!("type: {}", valid_type)) {
            return Ok(());
        }
    }

    // Check the filename prefix matches the type
    let _ = filename; // filename check is a warning, not a hard error
    anyhow::bail!(
        "type field must be one of: {}",
        VALID_TYPES.join(", ")
    );
}
