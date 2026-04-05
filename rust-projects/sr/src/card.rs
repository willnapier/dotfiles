use anyhow::{bail, Context, Result};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config;

/// FSRS scheduling state stored in card frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardFrontmatter {
    pub id: String,
    pub deck: String,
    /// ISO date or datetime when next review is due
    pub due: String,
    pub stability: f64,
    pub difficulty: f64,
    pub reps: u32,
    /// Empty string = never reviewed
    #[serde(default)]
    pub last_review: String,
}

/// A fully parsed card (frontmatter + question/answer).
#[derive(Debug, Clone)]
pub struct Card {
    pub id: String,
    pub deck: String,
    pub due: DateTime<Utc>,
    pub stability: f64,
    pub difficulty: f64,
    pub reps: u32,
    pub last_review: Option<DateTime<Utc>>,
    pub question: String,
    pub answer: String,
    /// File path this card was loaded from
    pub path: PathBuf,
}

impl Card {
    /// Load a card from a `.md` file.
    pub fn load(path: &Path) -> Result<Self> {
        let content =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&content, path)
    }

    /// Parse a card from its raw file content.
    pub fn parse(content: &str, path: &Path) -> Result<Self> {
        // Split frontmatter from body
        let (fm_str, body) = split_frontmatter(content)
            .with_context(|| format!("missing frontmatter in {}", path.display()))?;

        let fm: CardFrontmatter = serde_yaml::from_str(fm_str)
            .with_context(|| format!("invalid frontmatter in {}", path.display()))?;

        let due = parse_date_field(&fm.due)
            .with_context(|| format!("invalid 'due' in {}: {:?}", path.display(), fm.due))?;

        let last_review = if fm.last_review.trim().is_empty() {
            None
        } else {
            Some(parse_date_field(&fm.last_review).with_context(|| {
                format!(
                    "invalid 'last_review' in {}: {:?}",
                    path.display(),
                    fm.last_review
                )
            })?)
        };

        let (question, answer) =
            parse_qa(body).with_context(|| format!("missing Q:/A: in {}", path.display()))?;

        Ok(Card {
            id: fm.id,
            deck: fm.deck,
            due,
            stability: fm.stability,
            difficulty: fm.difficulty,
            reps: fm.reps,
            last_review,
            question,
            answer,
            path: path.to_path_buf(),
        })
    }

    /// Save the card back to its file, updating only frontmatter fields.
    pub fn save(&self) -> Result<()> {
        let content = self.to_file_content();
        fs::write(&self.path, content)
            .with_context(|| format!("writing {}", self.path.display()))?;
        Ok(())
    }

    /// Create a new card file on disk and return the Card.
    pub fn create(deck: &str, question: &str, answer: &str) -> Result<Self> {
        let id = generate_id(deck, question);
        let path = config::card_path(deck, &id);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating deck directory {}", parent.display()))?;
        }

        if path.exists() {
            bail!("card already exists: {}", path.display());
        }

        let due = Utc::now();
        let card = Card {
            id,
            deck: deck.to_string(),
            due,
            stability: 0.0,
            difficulty: 0.0,
            reps: 0,
            last_review: None,
            question: question.trim().to_string(),
            answer: answer.trim().to_string(),
            path,
        };

        card.save()?;
        Ok(card)
    }

    /// Render the card to file content (frontmatter + body).
    pub fn to_file_content(&self) -> String {
        let due_str = self.due.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let last_review_str = self
            .last_review
            .map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string())
            .unwrap_or_default();

        format!(
            "---\nid: {}\ndeck: {}\ndue: {}\nstability: {:.4}\ndifficulty: {:.4}\nreps: {}\nlast_review: {}\n---\n\nQ: {}\nA: {}\n",
            self.id,
            self.deck,
            due_str,
            self.stability,
            self.difficulty,
            self.reps,
            last_review_str,
            self.question,
            self.answer,
        )
    }

    /// Is this card due for review (due <= now)?
    #[allow(dead_code)]
    pub fn is_due(&self) -> bool {
        self.due <= Utc::now()
    }

    /// Days overdue (negative = not yet due)
    #[allow(dead_code)]
    pub fn days_overdue(&self) -> f64 {
        let now = Utc::now();
        let delta = now.signed_duration_since(self.due);
        delta.num_seconds() as f64 / 86400.0
    }

    /// Preview of the question (first 60 chars)
    pub fn question_preview(&self) -> String {
        if self.question.len() <= 60 {
            self.question.clone()
        } else {
            format!("{}…", &self.question[..57])
        }
    }
}

/// Scan a deck directory for all card files, skipping malformed ones with a warning.
pub fn load_deck(deck: &str) -> Result<Vec<Card>> {
    let dir = config::deck_dir(deck);
    if !dir.exists() {
        return Ok(vec![]);
    }
    load_dir(&dir)
}

/// Scan all decks in the sr root directory.
pub fn load_all_cards() -> Result<Vec<Card>> {
    let root = config::sr_dir();
    if !root.exists() {
        return Ok(vec![]);
    }
    load_dir(&root)
}

fn load_dir(dir: &Path) -> Result<Vec<Card>> {
    let mut cards = Vec::new();

    for entry in walkdir(dir)? {
        let path = entry?;
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        match Card::load(&path) {
            Ok(card) => cards.push(card),
            Err(e) => eprintln!("warning: skipping {}: {}", path.display(), e),
        }
    }

    Ok(cards)
}

/// Simple recursive directory walker that returns file paths.
fn walkdir(dir: &Path) -> Result<impl Iterator<Item = Result<PathBuf>>> {
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_files(dir, &mut paths)?;
    Ok(paths.into_iter().map(Ok))
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        fs::read_dir(dir).with_context(|| format!("reading directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

// ── Parsing helpers ────────────────────────────────────────────────────────

/// Split a markdown document into (frontmatter_str, body_str).
/// Returns None if the file does not start with `---`.
fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return None;
    }
    // Skip the opening ---
    let rest = &content[3..];
    // Find the closing ---
    let end = rest.find("\n---")?;
    let fm = &rest[..end];
    let body = &rest[end + 4..]; // skip "\n---"
    Some((fm.trim(), body.trim()))
}

/// Parse Q: and A: lines from card body.
fn parse_qa(body: &str) -> Option<(String, String)> {
    let mut question = None;
    let mut answer_lines: Vec<&str> = Vec::new();
    let mut in_answer = false;

    for line in body.lines() {
        if let Some(stripped) = line.strip_prefix("Q: ") {
            question = Some(stripped.to_string());
            in_answer = false;
        } else if let Some(stripped) = line.strip_prefix("A: ") {
            answer_lines.clear();
            answer_lines.push(stripped);
            in_answer = true;
        } else if in_answer {
            answer_lines.push(line);
        }
    }

    let q = question?;
    if answer_lines.is_empty() {
        return None;
    }

    // Trim trailing blank lines from answer
    while answer_lines
        .last()
        .map(|l| l.trim().is_empty())
        .unwrap_or(false)
    {
        answer_lines.pop();
    }

    Some((q, answer_lines.join("\n")))
}

/// Parse a date field that may be a plain date (2026-04-06) or datetime (2026-04-06T00:00:00Z).
fn parse_date_field(s: &str) -> Result<DateTime<Utc>> {
    let s = s.trim();
    // Try full RFC3339 / ISO8601 datetime first
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    // Try plain date YYYY-MM-DD
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap());
        return Ok(dt);
    }
    bail!("cannot parse date: {:?}", s)
}

/// Generate a stable card ID from deck name and question text.
fn generate_id(deck: &str, question: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    question.hash(&mut hasher);
    let hash = hasher.finish();

    // Sanitise deck name for use in ID
    let deck_slug: String = deck
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();

    format!("{}-{:08x}", deck_slug, hash as u32)
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    fn sample_content(due: &str) -> String {
        format!(
            "---\nid: test-card\ndeck: test\ndue: {}\nstability: 0.0\ndifficulty: 0.0\nreps: 0\nlast_review: \n---\n\nQ: What is 2+2?\nA: 4\n",
            due
        )
    }

    #[test]
    fn parse_card_datetime() {
        let content = sample_content("2026-04-06T00:00:00Z");
        let card = Card::parse(&content, Path::new("test-card.md")).unwrap();
        assert_eq!(card.question, "What is 2+2?");
        assert_eq!(card.answer, "4");
        assert_eq!(card.reps, 0);
        assert!(card.last_review.is_none());
    }

    #[test]
    fn parse_card_plain_date() {
        let content = sample_content("2026-04-06");
        let card = Card::parse(&content, Path::new("test-card.md")).unwrap();
        assert_eq!(card.question, "What is 2+2?");
    }

    #[test]
    fn parse_card_multiline_answer() {
        let content = "---\nid: multi\ndeck: test\ndue: 2026-04-06\nstability: 0.0\ndifficulty: 0.0\nreps: 0\nlast_review: \n---\n\nQ: Name three colours\nA: Red\nBlue\nGreen\n";
        let card = Card::parse(content, Path::new("multi.md")).unwrap();
        assert_eq!(card.answer, "Red\nBlue\nGreen");
    }

    #[test]
    fn roundtrip_file_content() {
        let content = sample_content("2026-04-06T00:00:00Z");
        let card = Card::parse(&content, Path::new("test-card.md")).unwrap();
        let rendered = card.to_file_content();
        // Re-parse
        let card2 = Card::parse(&rendered, Path::new("test-card.md")).unwrap();
        assert_eq!(card2.question, card.question);
        assert_eq!(card2.answer, card.answer);
        assert_eq!(card2.id, card.id);
        assert_eq!(card2.deck, card.deck);
    }

    #[test]
    fn create_and_load_card() {
        let dir = tempdir().unwrap();
        // Temporarily override sr_dir by writing to a known path
        let deck_path = dir.path().join("testdeck");
        fs::create_dir_all(&deck_path).unwrap();

        let id = generate_id("testdeck", "What is the capital of France?");
        let path = deck_path.join(format!("{}.md", id));

        // Write a card manually
        let card = Card {
            id: id.clone(),
            deck: "testdeck".to_string(),
            due: Utc::now(),
            stability: 0.0,
            difficulty: 0.0,
            reps: 0,
            last_review: None,
            question: "What is the capital of France?".to_string(),
            answer: "Paris".to_string(),
            path: path.clone(),
        };
        card.save().unwrap();

        // Load it back
        let loaded = Card::load(&path).unwrap();
        assert_eq!(loaded.question, "What is the capital of France?");
        assert_eq!(loaded.answer, "Paris");
    }

    #[test]
    fn generate_id_is_stable() {
        let id1 = generate_id("german", "What is 'nevertheless'?");
        let id2 = generate_id("german", "What is 'nevertheless'?");
        assert_eq!(id1, id2);
    }

    #[test]
    fn generate_id_is_distinct() {
        let id1 = generate_id("german", "Question A");
        let id2 = generate_id("german", "Question B");
        assert_ne!(id1, id2);
    }
}
