use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::html::Concert;

/// Get the archive directory path.
pub fn get_archive_dir() -> PathBuf {
    dirs::home_dir()
        .expect("Could not find home directory")
        .join("Captures/concerts")
}

/// Get the full archive path for a filename.
pub fn get_archive_path(filename: &str) -> PathBuf {
    get_archive_dir().join(filename)
}

/// Generate archive filename from concert data.
/// Format: YYYY-MM-DD-slugified-performers.html
pub fn generate_filename(concert: &Concert) -> String {
    let date_str = concert.date.format("%Y-%m-%d").to_string();

    // Create slug from performers
    let performer_slug = if concert.performers.is_empty() {
        "concert".to_string()
    } else {
        // Use first performer or ensemble name
        let first = &concert.performers[0];
        slug::slugify(first)
    };

    format!("{}-{}.html", date_str, performer_slug)
}

/// Move HTML file to archive directory.
pub fn move_to_archive(source: &PathBuf, dest: &PathBuf) -> Result<()> {
    // Ensure archive directory exists
    let archive_dir = get_archive_dir();
    if !archive_dir.exists() {
        std::fs::create_dir_all(&archive_dir)
            .with_context(|| format!("Failed to create archive directory: {}", archive_dir.display()))?;
    }

    // Move (rename) the file
    std::fs::rename(source, dest).with_context(|| {
        format!(
            "Failed to move {} to {}",
            source.display(),
            dest.display()
        )
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn test_generate_filename() {
        let concert = Concert {
            date: NaiveDate::from_ymd_opt(2026, 1, 28).unwrap(),
            performers: vec!["The English Concert".to_string()],
            works: vec![],
        };

        let filename = generate_filename(&concert);
        assert_eq!(filename, "2026-01-28-the-english-concert.html");
    }

    #[test]
    fn test_generate_filename_empty_performers() {
        let concert = Concert {
            date: NaiveDate::from_ymd_opt(2026, 1, 28).unwrap(),
            performers: vec![],
            works: vec![],
        };

        let filename = generate_filename(&concert);
        assert_eq!(filename, "2026-01-28-concert.html");
    }
}
