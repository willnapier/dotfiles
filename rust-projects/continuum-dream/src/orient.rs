use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::types::{MemoryFile, MemoryFrontmatter, MemoryState};

/// Default memory directory — derived from $HOME using Claude Code's slug convention.
/// CC converts the home path to a slug by replacing every '/' with '-', giving
/// e.g. /home/will → -home-will, /Users/williamnapier → -Users-williamnapier.
pub fn memory_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("No home directory")?;
    let slug = home.to_string_lossy().replace('/', "-");
    Ok(home.join(format!(".claude/projects/{}/memory", slug)))
}

/// The path→name boundary for memory files: the file name as NFC text.
/// Every `MemoryFile.filename`, and every name compared against one, comes
/// through here; `MemoryFile.path` stays as the OS listed it and is the only
/// thing opened, copied or removed.
pub fn memory_name(path: &Path) -> String {
    forge_names::file_name(path)
}

/// Look a typed (or LLM-emitted) file name up among the loaded memory files,
/// NFC to NFC, and return that entry's own path. Never build a path from the
/// name: on Linux an NFC name joined onto the directory would miss an NFD
/// file and create a twin.
pub fn resolve_name<'a>(state: &'a MemoryState, name: &str) -> Option<&'a Path> {
    let want = forge_names::nfc(name);
    state
        .files
        .iter()
        .find(|f| f.filename == want)
        .map(|f| f.path.as_path())
}

/// Scan the memory directory and return the full state
pub fn scan_memory() -> Result<MemoryState> {
    scan_memory_at(&memory_dir()?)
}

/// Scan `dir` as a memory directory and return the full state
pub fn scan_memory_at(dir: &Path) -> Result<MemoryState> {
    let dir = dir.to_path_buf();
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
            let filename = memory_name(&path);
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

/// Extract filenames referenced in MEMORY.md as markdown links: [Title](filename.md).
/// Link text may be in either Unicode form (pasted, or written by an older
/// run); it is NFC'd here so it compares with `memory_name` output.
fn extract_index_refs(content: &str) -> Vec<String> {
    let re = Regex::new(r"\[[^\]]+\]\(([^)]+\.md)\)").unwrap();
    re.captures_iter(content)
        .map(|c| forge_names::nfc(&c[1]))
        .collect()
}

/// Parse a memory file into a MemoryFile struct
fn parse_memory_file(path: &PathBuf) -> Result<MemoryFile> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let filename = memory_name(path);

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

#[cfg(test)]
pub(crate) mod nfc_tests {
    use super::*;

    pub const NFD: &str = "feedback_zoe\u{0308}.md";
    pub const NFC: &str = "feedback_zoë.md";
    const BODY: &str = "---\nname: Zoë\ndescription: d\ntype: feedback\n---\n\nbody\n";

    /// A memory dir holding one NFD-named file that MEMORY.md links in NFC.
    pub fn nfd_memory_dir() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join(NFD), BODY).unwrap();
        fs::write(
            d.path().join("MEMORY.md"),
            format!("# Memory\n- [Zoë]({NFC}) — hook\n"),
        )
        .unwrap();
        d
    }

    #[test]
    fn names_are_nfc_paths_are_as_listed_and_index_matches() {
        let d = nfd_memory_dir();
        let state = scan_memory_at(d.path()).unwrap();
        assert_eq!(state.files.len(), 1);
        let file = &state.files[0];
        assert_eq!(file.filename, NFC);
        assert_eq!(file.path.file_name().unwrap(), std::ffi::OsStr::new(NFD));
        assert!(state.orphaned_index_refs.is_empty(), "{:?}", state.orphaned_index_refs);
        assert!(state.unindexed_files.is_empty(), "{:?}", state.unindexed_files);
        assert_eq!(resolve_name(&state, NFC), Some(file.path.as_path()));
        assert_eq!(resolve_name(&state, NFD), Some(file.path.as_path()));
        assert_eq!(resolve_name(&state, "feedback_other.md"), None);
    }
}
