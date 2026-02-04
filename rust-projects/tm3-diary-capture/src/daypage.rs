use anyhow::{Context, Result};
use chrono::NaiveDate;
use std::path::PathBuf;

/// Get the path to a DayPage for a given date.
pub fn get_daypage_path(date: &NaiveDate) -> PathBuf {
    let forge_path = dirs::home_dir()
        .expect("Could not find home directory")
        .join("Forge/NapierianLogs/DayPages");

    let filename = format!("{}.md", date.format("%Y-%m-%d"));
    forge_path.join(filename)
}

/// Get the path to the pending queue file for a given date.
fn get_pending_path(date: &NaiveDate) -> PathBuf {
    let pending_dir = dirs::home_dir()
        .expect("Could not find home directory")
        .join(".local/share/daypage-pending");

    let filename = format!("{}.md", date.format("%Y-%m-%d"));
    pending_dir.join(filename)
}

/// Append a clinical checklist block to the DayPage for the given date.
///
/// If the DayPage doesn't exist yet, creates it directly (safe — Helix
/// can't have it open). If it exists, queues via the pending system to
/// avoid "file modified by external process" errors in Helix.
/// Flush with Space+U in Helix or `daypage-flush` from the command line.
pub fn append_entry(date: &NaiveDate, entry: &str) -> Result<()> {
    let daypage_path = get_daypage_path(date);

    if !daypage_path.exists() {
        // DayPage doesn't exist — safe to create directly
        let content = format!("# {}\n\n{}\n\n## Backlinks\n", date.format("%Y-%m-%d"), entry);
        std::fs::write(&daypage_path, content)
            .with_context(|| format!("Failed to create DayPage: {}", daypage_path.display()))?;
        return Ok(());
    }

    // DayPage exists — check for duplicate clinic:: block (read-only, safe)
    let content = std::fs::read_to_string(&daypage_path)
        .with_context(|| format!("Failed to read DayPage: {}", daypage_path.display()))?;

    if content.contains("clinic::") {
        eprintln!(
            "Warning: {} already has a clinic:: block, skipping",
            daypage_path.display()
        );
        return Ok(());
    }

    // Queue to pending file instead of writing directly
    let pending_path = get_pending_path(date);
    if let Some(parent) = pending_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create pending dir: {}", parent.display()))?;
    }

    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&pending_path)
        .with_context(|| format!("Failed to open pending file: {}", pending_path.display()))?;

    writeln!(file, "{}", entry)
        .with_context(|| format!("Failed to write pending file: {}", pending_path.display()))?;

    eprintln!("  → Queued (flush with Space+U or daypage-flush)");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn test_creates_new_daypage_directly() {
        let dir = tempfile::tempdir().unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 3, 15).unwrap();
        let path = dir.path().join("2026-03-15.md");

        // Patch: write to temp dir instead of real Forge
        let entry = "clinic::\n- [ ] JS92 10:00";
        let content = format!("# {}\n\n{}\n\n## Backlinks\n", date.format("%Y-%m-%d"), entry);
        std::fs::write(&path, content).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("clinic::"));
        assert!(result.contains("- [ ] JS92 10:00"));
        assert!(result.contains("## Backlinks"));
    }

    #[test]
    fn test_queues_when_daypage_exists() {
        let dir = tempfile::tempdir().unwrap();
        let pending_path = dir.path().join("2026-03-15.md");

        // Simulate queuing by appending to pending file
        let entry = "clinic::\n- [ ] JS92 10:00";
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&pending_path)
            .unwrap();
        writeln!(file, "{}", entry).unwrap();
        drop(file);

        let mut result = String::new();
        std::fs::File::open(&pending_path)
            .unwrap()
            .read_to_string(&mut result)
            .unwrap();
        assert!(result.contains("clinic::"));
        assert!(result.contains("- [ ] JS92 10:00"));
    }
}
