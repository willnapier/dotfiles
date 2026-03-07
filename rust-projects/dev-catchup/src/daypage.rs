use anyhow::Result;
use chrono::NaiveDate;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::matching::normalize_term;
use crate::types::DevEntry;

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "this", "that", "from", "into", "was", "were", "has", "had",
    "have", "not", "but", "are", "can", "its", "all", "also", "been", "more", "when", "will",
    "each", "then", "than", "use", "used", "using", "via", "after", "before", "new", "added",
    "add", "set", "get", "run", "fixed", "fix", "built", "updated", "update", "moved", "based",
    "some", "one", "two", "now", "how", "out", "about", "just", "did", "does", "done",
];

/// Build the DayPage path for a given date.
pub fn daypage_path(date: NaiveDate) -> PathBuf {
    let home = dirs::home_dir().expect("no home directory");
    home.join("Forge/NapierianLogs/DayPages")
        .join(format!("{}.md", date.format("%Y-%m-%d")))
}

/// Read a DayPage and extract dev:: entries with their terms.
pub fn extract_dev_entries(date: NaiveDate) -> Result<Vec<DevEntry>> {
    let path = daypage_path(date);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };

    let entries = content
        .lines()
        .filter(|line| line.contains("dev::"))
        .map(|line| {
            let terms = extract_entry_terms(line);
            DevEntry {
                raw: line.to_string(),
                terms,
            }
        })
        .collect();

    Ok(entries)
}

/// Extract terms from a dev:: entry line.
fn extract_entry_terms(line: &str) -> HashSet<String> {
    let stopwords: HashSet<&str> = STOPWORDS.iter().copied().collect();

    // Strip "dev::" prefix and common patterns
    let text = line
        .trim()
        .trim_start_matches("- ")
        .trim_start_matches("dev::")
        .trim();

    let mut terms = HashSet::new();
    for word in text.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_') {
        let w = normalize_term(word);
        // Skip empty, short, stopwords, and numeric patterns (Nx, Mmin)
        if w.len() < 3 {
            continue;
        }
        if stopwords.contains(w.as_str()) {
            continue;
        }
        // Skip exchange count (e.g. "10x") and duration (e.g. "19min")
        if w.ends_with('x') && w[..w.len() - 1].parse::<u32>().is_ok() {
            continue;
        }
        if w.ends_with("min") && w[..w.len() - 3].parse::<u32>().is_ok() {
            continue;
        }
        terms.insert(w);
    }
    terms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entry_term_extraction() {
        let terms = extract_entry_terms(
            "dev:: 10x 19min - Fixed collect-entries reminder bug: relative dates",
        );
        assert!(terms.contains("collect-entrie")); // normalized: strip trailing 's'
        assert!(terms.contains("reminder"));
        assert!(terms.contains("relative"));
        assert!(terms.contains("date")); // "dates" normalized
        // Should NOT contain stopwords or numeric patterns
        assert!(!terms.contains("10x"));
        assert!(!terms.contains("19min"));
        assert!(!terms.contains("the"));
    }
}
