//! PHI pseudonymization for prompts sent to external AI backends.
//!
//! Builds a substitution map from a client's identity.yaml and applies it
//! to prompt text before any API call, then reverts placeholders in the
//! generated output before display or saving.
//!
//! Guarantees:
//! - Real names never reach the AI API
//! - Generated output never contains full names — only first name is restored
//! - Observation text is also scanned (belt-and-suspenders for practitioner slip)

/// A map of (real_string → placeholder) pairs used to pseudonymize prompts
/// and restore generated output.
#[derive(Clone)]
pub struct PseudonymMap {
    /// Sorted longest-first so longer strings replace before their substrings.
    entries: Vec<(String, String)>,
}

impl PseudonymMap {
    /// Build from a client's identity.yaml. Silent on missing or malformed files.
    pub fn from_identity_file(path: &std::path::Path) -> Self {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Self { entries: vec![] },
        };
        Self::from_identity_content(&content)
    }

    pub fn from_identity_content(content: &str) -> Self {
        let mut entries: Vec<(String, String)> = Vec::new();

        // Client full name → [CLIENT]; also register first name alone.
        if let Some(name) = field(content, &["name"]) {
            let first = name.split_whitespace().next().unwrap_or("").to_string();
            entries.push((name, "[CLIENT]".to_string()));
            if !first.is_empty() {
                entries.push((first, "[CLIENT]".to_string()));
            }
        }

        // Date of birth — rarely in notes but strip if present.
        if let Some(dob) = field(content, &["dob"]) {
            entries.push((dob, "[DOB]".to_string()));
        }

        // Referrer name (full + first).
        if let Some(rname) = field(content, &["referrer", "name"]) {
            let first = rname.split_whitespace().next().unwrap_or("").to_string();
            entries.push((rname, "[REFERRER]".to_string()));
            if !first.is_empty() {
                entries.push((first, "[REFERRER]".to_string()));
            }
        }

        // Referrer practice name.
        if let Some(practice) = field(content, &["referrer", "practice"]) {
            entries.push((practice, "[REFERRER_PRACTICE]".to_string()));
        }

        // Entities list — named employers, third parties, organisations.
        for (i, entity) in list(content, "entities").into_iter().enumerate() {
            entries.push((entity, format!("[ORG_{}]", i + 1)));
        }

        // Sort longest-first, then deduplicate on the real string.
        entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        entries.dedup_by(|a, b| a.0 == b.0);
        // Drop empty keys.
        entries.retain(|(real, _)| !real.is_empty());

        Self { entries }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Apply pseudonymization to outbound prompt text.
    /// Matches exact case, title case, and lowercase variants.
    pub fn apply(&self, text: &str) -> String {
        if self.entries.is_empty() {
            return text.to_string();
        }
        let mut out = text.to_string();
        for (real, placeholder) in &self.entries {
            // Exact case
            out = out.replace(real.as_str(), placeholder.as_str());
            // Title case (first char uppercase, rest as-is)
            let title = title_case(real);
            if title != *real {
                out = out.replace(&title, placeholder.as_str());
            }
            // All lowercase
            let lower = real.to_lowercase();
            if lower != *real && lower != title {
                out = out.replace(&lower, placeholder.as_str());
            }
        }
        out
    }

    /// Revert placeholders in AI output to display names.
    /// [CLIENT] → first name only — full name is never emitted.
    pub fn revert(&self, text: &str) -> String {
        if self.entries.is_empty() {
            return text.to_string();
        }
        let mut out = text.to_string();
        // Collect unique placeholder → display pairs.
        // Build in reverse-length order to avoid double substitution.
        let mut seen = std::collections::HashSet::new();
        let mut pairs: Vec<(&str, String)> = Vec::new();
        for (real, placeholder) in &self.entries {
            if seen.contains(placeholder.as_str()) {
                continue;
            }
            seen.insert(placeholder.as_str());
            let display = if placeholder == "[CLIENT]" {
                real.split_whitespace().next().unwrap_or(real.as_str()).to_string()
            } else {
                real.clone()
            };
            pairs.push((placeholder.as_str(), display));
        }
        for (placeholder, display) in &pairs {
            out = out.replace(placeholder, display.as_str());
        }
        out
    }

    /// Addendum for the system prompt instructing the model to use tokens verbatim.
    /// Returns None if the map is empty (no identifiers to protect).
    pub fn system_addendum(&self) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let mut tokens: Vec<&str> = self.entries.iter()
            .map(|(_, ph)| ph.as_str())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        tokens.sort();
        Some(format!(
            "\n\nIMPORTANT — placeholder tokens: the prompt uses {} in place of real names. \
             Use these tokens verbatim in your output wherever a name would appear. \
             Do not substitute, expand, or guess the real names behind them.",
            tokens.join(", ")
        ))
    }
}

// ─── Line-scan YAML helpers ───────────────────────────────────────────────────
// Avoids serde_yaml's multi-document hang (files ending with ---).

fn field(content: &str, path: &[&str]) -> Option<String> {
    match path {
        [key] => top_level(content, key),
        [section, key] => nested(content, section, key),
        _ => None,
    }
}

fn top_level(content: &str, key: &str) -> Option<String> {
    let prefix = format!("{}:", key);
    content.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .find(|l| {
            let t = l.trim_start();
            t.starts_with(&prefix) && !t.contains("null") && !t.contains('~')
        })
        .and_then(|l| l.splitn(2, ':').nth(1))
        .map(|v| v.trim().trim_matches('"').trim_matches('\'').to_string())
        .filter(|v| !v.is_empty())
}

fn nested(content: &str, section: &str, key: &str) -> Option<String> {
    let section_prefix = format!("{}:", section);
    let key_prefix = format!("{}:", key);
    let mut in_section = false;
    for line in content.lines() {
        if line.trim_start().starts_with('#') { continue; }
        let trimmed = line.trim_start();
        if !in_section {
            if trimmed.starts_with(&section_prefix) {
                in_section = true;
            }
        } else {
            if !line.is_empty() && !line.starts_with(' ') && !line.starts_with('\t') {
                break;
            }
            if trimmed.starts_with(&key_prefix) && !trimmed.contains("null") && !trimmed.contains('~') {
                return trimmed.splitn(2, ':').nth(1)
                    .map(|v| v.trim().trim_matches('"').trim_matches('\'').to_string())
                    .filter(|v| !v.is_empty());
            }
        }
    }
    None
}

fn list(content: &str, key: &str) -> Vec<String> {
    let prefix = format!("{}:", key);
    let mut in_list = false;
    let mut items = Vec::new();
    for line in content.lines() {
        if line.trim_start().starts_with('#') { continue; }
        let trimmed = line.trim_start();
        if !in_list {
            if trimmed.starts_with(&prefix) {
                in_list = true;
            }
        } else {
            if !line.is_empty() && !line.starts_with(' ') && !line.starts_with('\t') {
                break;
            }
            if let Some(rest) = trimmed.strip_prefix("- ") {
                let item = rest.trim().trim_matches('"').trim_matches('\'').to_string();
                if !item.is_empty() {
                    items.push(item);
                }
            }
        }
    }
    items
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
name: Alice Smith
dob: 1985-03-14
referrer:
  name: Dr James Brown
  practice: Harley Street Clinic
entities:
  - Accenture
  - Goldman Sachs
"#;

    #[test]
    fn apply_strips_names() {
        let map = PseudonymMap::from_identity_content(SAMPLE);
        let text = "Alice Smith was referred by Dr James Brown from Harley Street Clinic.";
        let stripped = map.apply(text);
        assert!(!stripped.contains("Alice"));
        assert!(!stripped.contains("Brown"));
        assert!(!stripped.contains("Harley Street Clinic"));
        assert!(stripped.contains("[CLIENT]"));
        assert!(stripped.contains("[REFERRER]"));
        assert!(stripped.contains("[REFERRER_PRACTICE]"));
    }

    #[test]
    fn revert_restores_first_name_only() {
        let map = PseudonymMap::from_identity_content(SAMPLE);
        let note = "[CLIENT] explored her relationship with anxiety this session.";
        let reverted = map.revert(note);
        assert_eq!(reverted, "Alice explored her relationship with anxiety this session.");
        assert!(!reverted.contains("Smith")); // full name never emitted
    }

    #[test]
    fn apply_catches_title_case() {
        let map = PseudonymMap::from_identity_content(SAMPLE);
        let text = "alice smith was seen today.";
        let stripped = map.apply(text);
        assert!(!stripped.contains("alice"));
        assert!(!stripped.contains("smith"));
    }
}
