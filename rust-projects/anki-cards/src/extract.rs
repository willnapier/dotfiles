use anyhow::{Context, Result};
use std::process::Command;

use crate::Card;

const EXTRACTION_PROMPT: &str = r#"You are a flashcard extraction engine. Given the input text, extract key technical concepts, terms, ideas, and patterns as Anki flashcards.

Rules:
- Create atomic flashcards (one concept per card)
- Write specific questions (not vague "what is X?" — ask about the specific insight)
- Keep answers concise but complete
- Skip trivial or obvious content
- If the text contains no meaningful concepts to extract, return an empty array

Output ONLY a JSON array, no other text:
[{"front": "question", "back": "answer"}, ...]"#;

pub fn extract_cards(input: &str) -> Result<Vec<Card>> {
    let mut cmd = Command::new("claude");
    cmd.args(["-p", EXTRACTION_PROMPT]);
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().context("Failed to start 'claude' — is Claude Code installed?")?;

    if let Some(ref mut stdin) = child.stdin {
        use std::io::Write;
        stdin.write_all(input.as_bytes()).context("Failed to write to claude stdin")?;
    }
    // Close stdin by dropping it
    drop(child.stdin.take());

    let output = child.wait_with_output().context("Failed to read claude output")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("claude -p failed (exit {}): {}", output.status, stderr.trim());
    }

    let stdout = String::from_utf8(output.stdout).context("claude output was not valid UTF-8")?;
    parse_cards(&stdout)
}

fn parse_cards(text: &str) -> Result<Vec<Card>> {
    // Claude might wrap JSON in markdown code fences — strip them
    let trimmed = text.trim();
    let json_str = if trimmed.starts_with("```") {
        let inner = trimmed
            .strip_prefix("```json")
            .or_else(|| trimmed.strip_prefix("```"))
            .unwrap_or(trimmed);
        inner
            .strip_suffix("```")
            .unwrap_or(inner)
            .trim()
    } else {
        trimmed
    };

    // Find the JSON array boundaries in case there's surrounding text
    let start = json_str.find('[').context("No JSON array found in claude output")?;
    let end = json_str.rfind(']').context("No closing bracket in claude output")?;
    let array_str = &json_str[start..=end];

    let cards: Vec<Card> =
        serde_json::from_str(array_str).context("Failed to parse cards JSON from claude output")?;

    Ok(cards)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_clean_json() {
        let input = r#"[{"front": "Q1", "back": "A1"}, {"front": "Q2", "back": "A2"}]"#;
        let cards = parse_cards(input).unwrap();
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].front, "Q1");
    }

    #[test]
    fn parse_fenced_json() {
        let input = "```json\n[{\"front\": \"Q\", \"back\": \"A\"}]\n```";
        let cards = parse_cards(input).unwrap();
        assert_eq!(cards.len(), 1);
    }

    #[test]
    fn parse_with_surrounding_text() {
        let input = "Here are the cards:\n[{\"front\": \"Q\", \"back\": \"A\"}]\nDone.";
        let cards = parse_cards(input).unwrap();
        assert_eq!(cards.len(), 1);
    }

    #[test]
    fn parse_empty_array() {
        let input = "[]";
        let cards = parse_cards(input).unwrap();
        assert!(cards.is_empty());
    }
}
