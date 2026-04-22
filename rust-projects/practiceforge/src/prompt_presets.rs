//! Named prompt presets for clinical note generation.
//!
//! Presets live as markdown files at `~/.config/practiceforge/prompt-presets/<name>.md`.
//! The first H1 line (`# Display Label`) is the preset's display label; the body
//! below occupies the "practitioner additions" slot in the system prompt.
//!
//! A preset with an empty body is valid — "default" means "no extra practitioner
//! additions"; the preset's purpose is to have a named, editable slot.

use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize, Debug, Clone)]
pub struct PresetMeta {
    pub name: String,
    pub label: String,
    pub summary: String,
}

/// Seed content written to `default.md` on first call when the directory is missing.
const DEFAULT_SEED: &str = r#"# Default

<!-- Default prompt preset for clinical note generation.
     This content is appended to the system prompt at the
     practitioner-additions slot. Edit freely; keep the
     first H1 line — it's the display label. -->
"#;

/// Return the directory where prompt presets live.
fn presets_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("practiceforge")
        .join("prompt-presets")
}

/// Ensure the presets directory exists; if it was missing, seed a `default.md`.
fn ensure_dir_seeded() {
    let dir = presets_dir();
    if dir.exists() {
        return;
    }
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("[prompt_presets] could not create {}: {}", dir.display(), e);
        return;
    }
    let default_path = dir.join("default.md");
    if !default_path.exists() {
        if let Err(e) = std::fs::write(&default_path, DEFAULT_SEED) {
            eprintln!(
                "[prompt_presets] could not seed {}: {}",
                default_path.display(),
                e
            );
        }
    }
}

/// Extract (label, body) from a preset file's raw contents.
///
/// The label is the first H1 line (`# Foo`). If absent, falls back to the
/// derived name. The body is everything after the first H1 (trimmed).
fn parse_preset(name: &str, raw: &str) -> (String, String) {
    let mut lines = raw.lines();
    let mut label: Option<String> = None;

    // Scan for the first H1; everything before it is ignored.
    let mut body_start_idx = 0usize;
    for (idx, line) in raw.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            label = Some(rest.trim().to_string());
            body_start_idx = idx + 1;
            break;
        } else if trimmed == "#" {
            // Edge case: H1 marker with no text.
            label = Some(String::new());
            body_start_idx = idx + 1;
            break;
        }
    }
    let _ = lines; // silence unused var if lines() iterator is later needed

    let label = label.unwrap_or_else(|| {
        // Default label: capitalize name.
        let mut chars = name.chars();
        match chars.next() {
            Some(c) => c.to_uppercase().chain(chars).collect(),
            None => name.to_string(),
        }
    });
    let label = if label.is_empty() {
        let mut chars = name.chars();
        match chars.next() {
            Some(c) => c.to_uppercase().chain(chars).collect(),
            None => name.to_string(),
        }
    } else {
        label
    };

    let body: String = raw
        .lines()
        .skip(body_start_idx)
        .collect::<Vec<_>>()
        .join("\n");

    (label, body.trim().to_string())
}

/// Flatten whitespace and take the first ~100 chars of the body, ignoring
/// pure HTML comment blocks.
fn summarize(body: &str) -> String {
    // Strip HTML comments (simple, non-nested) before flattening.
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);

    // Flatten whitespace.
    let flat: String = out
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    if flat.chars().count() <= 100 {
        return flat;
    }
    // Take first 100 characters (char-safe).
    flat.chars().take(100).collect::<String>() + "…"
}

/// List all `.md` files in the presets directory as metadata records.
///
/// Guarantees at least one entry: `{"name":"default","label":"Default"}`.
/// If the directory is missing it will be created and seeded.
pub fn list_presets() -> Vec<PresetMeta> {
    ensure_dir_seeded();

    let dir = presets_dir();
    let mut out: Vec<PresetMeta> = Vec::new();
    let mut saw_default = false;

    if let Ok(read) = std::fs::read_dir(&dir) {
        for entry in read.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let name = stem.to_string();
            let raw = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!(
                        "[prompt_presets] could not read {}: {}",
                        path.display(),
                        e
                    );
                    continue;
                }
            };
            let (label, body) = parse_preset(&name, &raw);
            let summary = summarize(&body);
            if name == "default" {
                saw_default = true;
            }
            out.push(PresetMeta {
                name,
                label,
                summary,
            });
        }
    }

    if !saw_default {
        out.push(PresetMeta {
            name: "default".to_string(),
            label: "Default".to_string(),
            summary: String::new(),
        });
    }

    // Sort: "default" first, then alphabetical by name for stable UI ordering.
    out.sort_by(|a, b| match (a.name.as_str(), b.name.as_str()) {
        ("default", "default") => std::cmp::Ordering::Equal,
        ("default", _) => std::cmp::Ordering::Less,
        (_, "default") => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });

    out
}

/// Load a preset by name, returning its body (with the H1 label line stripped).
///
/// Returns an empty string when the preset is missing or unreadable (and logs
/// a warning to stderr). An empty-body preset (e.g. the seeded "default") is a
/// valid no-op addition to the system prompt.
pub fn load_preset(name: &str) -> String {
    // Defensive: reject path-segment characters.
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        eprintln!("[prompt_presets] refusing to load preset with suspicious name: {name:?}");
        return String::new();
    }

    ensure_dir_seeded();
    let path = presets_dir().join(format!("{name}.md"));
    if !path.exists() {
        eprintln!(
            "[prompt_presets] preset {:?} not found at {} — using empty addition",
            name,
            path.display()
        );
        return String::new();
    }
    match std::fs::read_to_string(&path) {
        Ok(raw) => {
            let (_label, body) = parse_preset(name, &raw);
            body
        }
        Err(e) => {
            eprintln!(
                "[prompt_presets] could not read preset {:?} at {}: {}",
                name,
                path.display(),
                e
            );
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_preset_extracts_label_and_body() {
        let raw = "# Terse\n\nShort tight notes.\n";
        let (label, body) = parse_preset("terse", raw);
        assert_eq!(label, "Terse");
        assert_eq!(body, "Short tight notes.");
    }

    #[test]
    fn parse_preset_falls_back_to_name_when_no_h1() {
        let raw = "Just some body text\n";
        let (label, body) = parse_preset("foo", raw);
        assert_eq!(label, "Foo");
        assert_eq!(body, "Just some body text");
    }

    #[test]
    fn summarize_strips_comments_and_flattens() {
        let body = "<!-- ignore this -->   Keep\nthis text.";
        let s = summarize(body);
        assert_eq!(s, "Keep this text.");
    }

    #[test]
    fn summarize_truncates_long_bodies() {
        let body = "a".repeat(250);
        let s = summarize(&body);
        assert!(s.ends_with('…'));
        assert!(s.chars().count() <= 101);
    }
}
