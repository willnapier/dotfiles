use anyhow::{bail, Result};
use std::io::{self, BufRead};

use crate::card::Card;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportFormat {
    Text,
    AnkiCards,
}

pub fn run(deck: &str, format: ImportFormat) -> Result<()> {
    let stdin = io::stdin();
    let content: String = stdin
        .lock()
        .lines()
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");

    let pairs = match format {
        ImportFormat::Text => parse_text_pairs(&content),
        ImportFormat::AnkiCards => parse_anki_cards_json(&content)?,
    };

    if pairs.is_empty() {
        bail!("no Q/A pairs found in input");
    }

    let mut added = 0;
    let mut skipped = 0;

    for (question, answer) in pairs {
        match Card::create(deck, &question, &answer) {
            Ok(card) => {
                println!("Added: {}", card.id);
                added += 1;
            }
            Err(e) if e.to_string().contains("already exists") => {
                // Duplicate — skip silently
                skipped += 1;
            }
            Err(e) => {
                eprintln!("warning: skipping {:?}: {}", question, e);
                skipped += 1;
            }
        }
    }

    println!("\nImported {} card(s), {} skipped.", added, skipped);
    Ok(())
}

/// Parse Q: / A: pairs from plain text.
///
/// Accepted formats:
///   Q: question text\nA: answer text
///   (blank line between cards)
fn parse_text_pairs(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut current_q: Option<String> = None;
    let mut current_a_lines: Vec<String> = Vec::new();
    let mut in_answer = false;

    let flush = |q: &mut Option<String>, a: &mut Vec<String>, pairs: &mut Vec<(String, String)>| {
        if let Some(question) = q.take() {
            if !a.is_empty() {
                // Trim trailing blank lines
                while a
                    .last()
                    .map(|l: &String| l.trim().is_empty())
                    .unwrap_or(false)
                {
                    a.pop();
                }
                if !a.is_empty() {
                    pairs.push((question, a.join("\n")));
                }
            }
            a.clear();
        }
    };

    for line in text.lines() {
        if let Some(q) = line.strip_prefix("Q: ") {
            // Start new card — flush previous if any
            flush(&mut current_q, &mut current_a_lines, &mut pairs);
            current_q = Some(q.trim().to_string());
            in_answer = false;
        } else if let Some(a) = line.strip_prefix("A: ") {
            current_a_lines.clear();
            current_a_lines.push(a.trim().to_string());
            in_answer = true;
        } else if in_answer && !line.trim().is_empty() {
            current_a_lines.push(line.to_string());
        } else if line.trim().is_empty() && in_answer {
            // Blank line may signal end of current answer — keep going in case
            // there are multi-line answers that use blank separation
        }
    }

    flush(&mut current_q, &mut current_a_lines, &mut pairs);
    pairs
}

/// Parse JSON output from the `anki-cards` tool.
/// Expected format: array of objects with "question" and "answer" fields.
fn parse_anki_cards_json(text: &str) -> Result<Vec<(String, String)>> {
    let text = text.trim();

    // Very simple JSON array parser — avoids serde_json dependency
    // Handles: [{"question": "...", "answer": "..."}, ...]
    if !text.starts_with('[') {
        bail!("expected JSON array, got: {}", &text[..text.len().min(40)]);
    }

    let mut pairs = Vec::new();

    // Find all "question":"..." and "answer":"..." pairs using basic scanning
    let mut pos = 0;
    let bytes = text.as_bytes();

    while pos < bytes.len() {
        // Find "question":
        if let Some(q_start) = find_json_field(text, pos, "question") {
            let (q_val, q_end) = extract_json_string(text, q_start)?;
            // Find "answer": starting after question field
            if let Some(a_start) = find_json_field(text, q_end, "answer") {
                let (a_val, a_end) = extract_json_string(text, a_start)?;
                pairs.push((q_val, a_val));
                pos = a_end;
            } else {
                pos = q_end;
            }
        } else {
            break;
        }
    }

    if pairs.is_empty() {
        bail!("no question/answer pairs found in JSON input");
    }

    Ok(pairs)
}

/// Find the position of the value start for a JSON field name.
fn find_json_field(text: &str, from: usize, field: &str) -> Option<usize> {
    let needle = format!("\"{}\"", field);
    let found = text[from..].find(&needle)?;
    let after_key = from + found + needle.len();
    // Skip optional whitespace and colon
    let rest = &text[after_key..];
    let colon_offset = rest.find(':')?;
    let after_colon = after_key + colon_offset + 1;
    // Skip whitespace
    let rest2 = &text[after_colon..];
    let quote_offset = rest2.find('"')?;
    Some(after_colon + quote_offset + 1) // position of first char of value
}

/// Extract a JSON string value starting at `from` (after the opening quote).
/// Returns (value, position after closing quote).
fn extract_json_string(text: &str, from: usize) -> Result<(String, usize)> {
    let mut result = String::new();
    let mut chars = text[from..].char_indices();

    while let Some((i, c)) = chars.next() {
        match c {
            '"' => {
                return Ok((result, from + i + 1));
            }
            '\\' => {
                if let Some((_, escaped)) = chars.next() {
                    match escaped {
                        '"' => result.push('"'),
                        '\\' => result.push('\\'),
                        'n' => result.push('\n'),
                        'r' => result.push('\r'),
                        't' => result.push('\t'),
                        other => {
                            result.push('\\');
                            result.push(other);
                        }
                    }
                }
            }
            other => result.push(other),
        }
    }

    bail!("unterminated JSON string at position {}", from)
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_text_simple() {
        let text = "Q: What is 2+2?\nA: 4\n\nQ: Capital of France?\nA: Paris\n";
        let pairs = parse_text_pairs(text);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "What is 2+2?");
        assert_eq!(pairs[0].1, "4");
        assert_eq!(pairs[1].0, "Capital of France?");
        assert_eq!(pairs[1].1, "Paris");
    }

    #[test]
    fn parse_text_multiline_answer() {
        let text = "Q: Name three colours\nA: Red\nBlue\nGreen\n\nQ: Next?\nA: Yes\n";
        let pairs = parse_text_pairs(text);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].1, "Red\nBlue\nGreen");
    }

    #[test]
    fn parse_json_simple() {
        let json = r#"[{"question": "What is 2+2?", "answer": "4"}]"#;
        let pairs = parse_anki_cards_json(json).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "What is 2+2?");
        assert_eq!(pairs[0].1, "4");
    }

    #[test]
    fn parse_json_multiple() {
        let json = r#"[
            {"question": "Q1", "answer": "A1"},
            {"question": "Q2", "answer": "A2"}
        ]"#;
        let pairs = parse_anki_cards_json(json).unwrap();
        assert_eq!(pairs.len(), 2);
    }

    #[test]
    fn parse_json_escaped_chars() {
        let json = r#"[{"question": "Say \"hello\"", "answer": "Line1\nLine2"}]"#;
        let pairs = parse_anki_cards_json(json).unwrap();
        assert_eq!(pairs[0].0, "Say \"hello\"");
        assert_eq!(pairs[0].1, "Line1\nLine2");
    }
}
