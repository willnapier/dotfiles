use anyhow::{Context, Result};
use chrono::Utc;
use std::fs;
use std::path::PathBuf;

use crate::types::{DreamResponse, MemoryState, ProposedChange};

/// Create a timestamped backup of the entire memory directory
fn backup_memory_dir(memory_state: &MemoryState) -> Result<PathBuf> {
    let backup_base = dirs::home_dir()
        .context("No home directory")?
        .join(".local/share/continuum-dream/backups");

    let timestamp = Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();
    let backup_dir = backup_base.join(&timestamp);
    fs::create_dir_all(&backup_dir)
        .with_context(|| format!("Failed to create backup dir: {}", backup_dir.display()))?;

    // Copy MEMORY.md
    if memory_state.index_path.exists() {
        fs::copy(
            &memory_state.index_path,
            backup_dir.join("MEMORY.md"),
        )?;
    }

    // Copy all memory files
    for file in &memory_state.files {
        fs::copy(&file.path, backup_dir.join(&file.filename))?;
    }

    Ok(backup_dir)
}

/// Apply all proposed changes to disk
pub fn write_changes(
    changes: &[ProposedChange],
    _response: &DreamResponse,
    memory_state: &MemoryState,
) -> Result<()> {
    // Step 1: Backup
    let backup_dir = backup_memory_dir(memory_state)?;
    eprintln!("Backup created: {}", backup_dir.display());

    // Step 2: Apply changes
    for change in changes {
        match change {
            ProposedChange::UpdateFile {
                filename,
                new_content,
                ..
            } => {
                let path = memory_state.memory_dir.join(filename);
                fs::write(&path, new_content)
                    .with_context(|| format!("Failed to write {}", path.display()))?;
                eprintln!("  Updated: {}", filename);
            }
            ProposedChange::CreateFile {
                filename, content, ..
            } => {
                let path = memory_state.memory_dir.join(filename);
                fs::write(&path, content)
                    .with_context(|| format!("Failed to write {}", path.display()))?;
                eprintln!("  Created: {}", filename);
            }
            ProposedChange::DeleteFile { filename, .. } => {
                let path = memory_state.memory_dir.join(filename);
                fs::remove_file(&path)
                    .with_context(|| format!("Failed to delete {}", path.display()))?;
                eprintln!("  Deleted: {}", filename);
            }
            ProposedChange::UpdateIndex { new_content, .. } => {
                fs::write(&memory_state.index_path, new_content)
                    .context("Failed to write MEMORY.md")?;
                eprintln!("  Updated: MEMORY.md");
            }
        }
    }

    Ok(())
}
