use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct Json3File {
    events: Option<Vec<Event>>,
}

#[derive(Debug, Deserialize)]
struct Event {
    #[serde(rename = "tStartMs")]
    t_start_ms: Option<u64>,
    segs: Option<Vec<Segment>>,
}

#[derive(Debug, Deserialize)]
struct Segment {
    utf8: Option<String>,
}

/// A deduplicated text segment with timing.
#[derive(Debug)]
struct TimedText {
    start_ms: u64,
    text: String,
}

/// Parse a json3 subtitle file into clean paragraphed text.
pub fn parse_json3(path: &Path) -> Result<String> {
    let data = std::fs::read_to_string(path).context("Failed to read json3 subtitle file")?;
    let file: Json3File = serde_json::from_str(&data).context("Failed to parse json3 format")?;

    let events = file.events.unwrap_or_default();

    // Extract timed text segments
    let mut segments: Vec<TimedText> = Vec::new();
    for event in &events {
        let start_ms = match event.t_start_ms {
            Some(t) => t,
            None => continue,
        };

        let segs = match &event.segs {
            Some(s) => s,
            None => continue,
        };

        let text: String = segs
            .iter()
            .filter_map(|s| s.utf8.as_deref())
            .collect::<Vec<_>>()
            .join("");

        let text = text.trim().to_string();
        if text.is_empty() || text == "\n" {
            continue;
        }

        segments.push(TimedText { start_ms, text });
    }

    // Deduplicate overlapping auto-caption segments
    let deduped = deduplicate(&segments);

    // Build paragraphs based on time gaps
    let paragraphs = build_paragraphs(&deduped);

    Ok(paragraphs.join("\n\n"))
}

/// Deduplicate segments where auto-captions produce overlapping/repeated text.
/// Strategy: skip a segment if its text is contained within the previous segment's text.
fn deduplicate(segments: &[TimedText]) -> Vec<&TimedText> {
    if segments.is_empty() {
        return Vec::new();
    }

    let mut result: Vec<&TimedText> = vec![&segments[0]];

    for seg in &segments[1..] {
        let prev = result.last().unwrap();

        // Skip if this segment's text is a substring of the previous one
        if prev.text.contains(&seg.text) {
            continue;
        }

        // Skip if previous text is a substring of this one and they overlap in time
        // (auto-captions often build up incrementally)
        if seg.text.contains(&prev.text) {
            // Replace previous with this more complete version
            *result.last_mut().unwrap() = seg;
            continue;
        }

        result.push(seg);
    }

    result
}

/// Group segments into paragraphs. Break on gaps > 2 seconds.
fn build_paragraphs(segments: &[&TimedText]) -> Vec<String> {
    if segments.is_empty() {
        return Vec::new();
    }

    let gap_threshold_ms: u64 = 2000;

    let mut paragraphs: Vec<String> = Vec::new();
    let mut current_para: Vec<&str> = vec![&segments[0].text];
    let mut prev_start = segments[0].start_ms;

    for seg in &segments[1..] {
        let gap = seg.start_ms.saturating_sub(prev_start);

        if gap > gap_threshold_ms {
            paragraphs.push(join_paragraph(&current_para));
            current_para = Vec::new();
        }

        current_para.push(&seg.text);
        prev_start = seg.start_ms;
    }

    if !current_para.is_empty() {
        paragraphs.push(join_paragraph(&current_para));
    }

    paragraphs
}

/// Join text fragments into a single paragraph, normalizing whitespace.
fn join_paragraph(parts: &[&str]) -> String {
    let joined = parts.join(" ");
    // Collapse multiple spaces
    let mut result = String::with_capacity(joined.len());
    let mut prev_space = false;
    for ch in joined.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                result.push(' ');
            }
            prev_space = true;
        } else {
            result.push(ch);
            prev_space = false;
        }
    }
    result.trim().to_string()
}
