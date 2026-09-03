use anyhow::{Context, Result};
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};

use crate::orient::resolve_name;
use crate::types::{DreamResponse, MemoryState, ProposedChange};

/// Create a timestamped backup of the entire memory directory
fn backup_memory_dir(memory_state: &MemoryState) -> Result<PathBuf> {
    let backup_base = dirs::home_dir()
        .context("No home directory")?
        .join(".local/share/continuum-dream/backups");
    backup_memory_dir_to(memory_state, &backup_base)
}

/// Create a timestamped backup of the memory directory under `backup_base`
fn backup_memory_dir_to(memory_state: &MemoryState, backup_base: &Path) -> Result<PathBuf> {
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

    // Copy all memory files. The destination basename is taken from the
    // on-disk path, not from the NFC `filename`: a name is never turned back
    // into a path.
    for file in &memory_state.files {
        let basename = file.path.file_name().context("memory file has no name")?;
        fs::copy(&file.path, backup_dir.join(basename))?;
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
    apply_changes(changes, memory_state)
}

/// Apply proposed changes to disk without taking a backup. Update and delete
/// go through `resolve_name` (NFC lookup → the entry's own path); create first
/// checks the directory for an equivalent existing file so an NFC name never
/// twins an NFD file on Linux.
fn apply_changes(changes: &[ProposedChange], memory_state: &MemoryState) -> Result<()> {
    for change in changes {
        match change {
            ProposedChange::UpdateFile {
                filename,
                new_content,
                ..
            } => {
                let path = resolve_name(memory_state, filename)
                    .with_context(|| format!("{} is not a loaded memory file", filename))?;
                fs::write(path, new_content)
                    .with_context(|| format!("Failed to write {}", path.display()))?;
                eprintln!("  Updated: {}", filename);
            }
            ProposedChange::CreateFile {
                filename, content, ..
            } => match forge_names::find_in_dir(&memory_state.memory_dir, filename) {
                Some(existing) => {
                    fs::write(&existing, content)
                        .with_context(|| format!("Failed to write {}", existing.display()))?;
                    eprintln!("  Created: {} (existing equivalent file updated)", filename);
                }
                None => {
                    let path = memory_state.memory_dir.join(forge_names::nfc(filename));
                    fs::write(&path, content)
                        .with_context(|| format!("Failed to write {}", path.display()))?;
                    eprintln!("  Created: {}", filename);
                }
            },
            ProposedChange::DeleteFile { filename, .. } => {
                let path = resolve_name(memory_state, filename)
                    .with_context(|| format!("{} is not a loaded memory file", filename))?;
                fs::remove_file(path)
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

#[cfg(test)]
mod nfc_tests {
    use super::*;
    use crate::orient::nfc_tests::{nfd_memory_dir, NFC, NFD};
    use crate::orient::scan_memory_at;

    fn memory_files(dir: &Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.file_name().unwrap() != "MEMORY.md")
            .collect();
        v.sort();
        v
    }

    #[test]
    fn update_and_create_with_nfc_name_write_the_nfd_file_not_a_twin() {
        let d = nfd_memory_dir();
        let state = scan_memory_at(d.path()).unwrap();
        let on_disk = state.files[0].path.clone();

        apply_changes(
            &[ProposedChange::UpdateFile {
                filename: NFC.to_string(),
                old_content: String::new(),
                new_content: "updated".to_string(),
                reason: String::new(),
            }],
            &state,
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&on_disk).unwrap(), "updated");
        assert_eq!(memory_files(d.path()), vec![on_disk.clone()]);

        apply_changes(
            &[ProposedChange::CreateFile {
                filename: NFC.to_string(),
                content: "created-over".to_string(),
                reason: String::new(),
            }],
            &state,
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&on_disk).unwrap(), "created-over");
        assert_eq!(memory_files(d.path()), vec![on_disk.clone()], "create must not twin");

        apply_changes(
            &[ProposedChange::CreateFile {
                filename: "project_ne\u{0308}w.md".to_string(),
                content: "fresh".to_string(),
                reason: String::new(),
            }],
            &state,
        )
        .unwrap();
        assert_eq!(memory_files(d.path()).len(), 2);
        let fresh = forge_names::find_in_dir(d.path(), "project_nëw.md")
            .expect("new file resolvable by NFC name");
        assert_eq!(fs::read_to_string(fresh).unwrap(), "fresh");
    }

    #[test]
    fn delete_with_nfc_name_removes_the_nfd_file() {
        let d = nfd_memory_dir();
        let state = scan_memory_at(d.path()).unwrap();
        let on_disk = state.files[0].path.clone();
        apply_changes(
            &[ProposedChange::DeleteFile {
                filename: NFC.to_string(),
                old_content: String::new(),
                reason: String::new(),
            }],
            &state,
        )
        .unwrap();
        assert!(!on_disk.exists());
        assert!(memory_files(d.path()).is_empty());
    }

    #[test]
    fn backup_copies_under_the_on_disk_basename() {
        let d = nfd_memory_dir();
        let state = scan_memory_at(d.path()).unwrap();
        let base = tempfile::tempdir().unwrap();
        let backup_dir = backup_memory_dir_to(&state, base.path()).unwrap();
        let mut names: Vec<std::ffi::OsString> = fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![std::ffi::OsString::from("MEMORY.md"), std::ffi::OsString::from(NFD)]
        );
    }
}
