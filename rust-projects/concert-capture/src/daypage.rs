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

/// Append a concert entry to the DayPage for the given date.
/// Inserts before ## Backlinks section if present.
pub fn append_entry(date: &NaiveDate, entry: &str) -> Result<()> {
    let path = get_daypage_path(date);

    if !path.exists() {
        // Create minimal DayPage if it doesn't exist
        let content = format!(
            "# {}\n\n{}\n\n## Backlinks\n",
            date.format("%Y-%m-%d"),
            entry
        );
        std::fs::write(&path, content)
            .with_context(|| format!("Failed to create DayPage: {}", path.display()))?;
        return Ok(());
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read DayPage: {}", path.display()))?;

    let new_content = insert_before_backlinks(&content, entry);

    std::fs::write(&path, new_content)
        .with_context(|| format!("Failed to write DayPage: {}", path.display()))?;

    Ok(())
}

fn insert_before_backlinks(content: &str, entry: &str) -> String {
    // Look for ## Backlinks section
    if let Some(pos) = content.find("## Backlinks") {
        let (before, after) = content.split_at(pos);
        let before = before.trim_end();
        format!("{}\n\n{}\n\n{}", before, entry, after)
    } else {
        // No backlinks section, append at end
        let content = content.trim_end();
        format!("{}\n\n{}\n", content, entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_before_backlinks() {
        let content = "# 2026-01-28\n\nSome notes here.\n\n## Backlinks\n\n- [[Other note]]";
        let entry = "concert:: Test entry";

        let result = insert_before_backlinks(content, entry);

        assert!(result.contains("Some notes here."));
        assert!(result.contains("concert:: Test entry"));
        assert!(result.contains("## Backlinks"));

        // Entry should come before Backlinks
        let entry_pos = result.find("concert::").unwrap();
        let backlinks_pos = result.find("## Backlinks").unwrap();
        assert!(entry_pos < backlinks_pos);
    }

    #[test]
    fn test_insert_no_backlinks() {
        let content = "# 2026-01-28\n\nSome notes here.";
        let entry = "concert:: Test entry";

        let result = insert_before_backlinks(content, entry);

        assert!(result.contains("Some notes here."));
        assert!(result.contains("concert:: Test entry"));
        assert!(result.ends_with('\n'));
    }
}
