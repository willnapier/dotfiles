use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Assertion;
use crate::evaluate;
use crate::log_parser::{EntryType, LogEntry};

/// A declarative specification for a mechanical check.
/// Each variant mirrors a helper function in evaluate.rs, but expressed as data
/// so the LLM can propose checks and we can validate them without compiling.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "check_type")]
pub enum CheckSpec {
    /// Pass if Read or Bash tool was used on a path containing the pattern
    #[serde(rename = "file_read")]
    FileRead { path_pattern: String },

    /// Pass if Bash tool contained the command string
    #[serde(rename = "command_run")]
    CommandRun { command: String },

    /// Pass if assistant text contains the keyword (case-insensitive)
    #[serde(rename = "text_contains")]
    TextContains { keyword: String },

    /// Pass if Edit/Write was NOT used on the path pattern
    #[serde(rename = "no_tool_on_path")]
    NoToolOnPath { path_pattern: String },

    /// Pass if Bash tool did NOT contain the pattern
    #[serde(rename = "no_bash_command")]
    NoBashCommand { pattern: String },

    /// Pass if trigger implies followups occurred (vacuously true if trigger absent)
    #[serde(rename = "tool_sequence")]
    ToolSequence {
        trigger: EntryMatcher,
        followups: Vec<EntryMatcher>,
    },

    /// Pass if no bare && or || in assistant text outside bash -c wrappers
    #[serde(rename = "no_bash_syntax")]
    NoBashSyntax,

    /// Pass if Edit tool was used on path pattern >= min_count times
    #[serde(rename = "multiple_edits")]
    MultipleEdits {
        path_pattern: String,
        min_count: usize,
    },

    /// Pass if Read was NOT used on path pattern
    #[serde(rename = "no_read_on_path")]
    NoReadOnPath { path_pattern: String },

    /// Pass if Write (not Edit) was NOT used on path pattern
    #[serde(rename = "no_write_on_path")]
    NoWriteOnPath { path_pattern: String },

    /// Pass if Write/Edit was NOT used on DayPages/
    #[serde(rename = "no_daypage_write")]
    NoDaypageWrite,

    /// No structural signal found — assertion stays with LLM evaluator
    #[serde(rename = "custom")]
    Custom { description: String },
}

/// Declarative matcher for a log entry (used in tool_sequence checks)
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EntryMatcher {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub content_contains: Option<String>,
    #[serde(default)]
    pub input_contains: Option<String>,
}

/// Full proposal with metadata
#[derive(Debug, Serialize, Deserialize)]
pub struct ProposedCheck {
    pub assertion_id: String,
    pub spec: CheckSpec,
    pub description: String,
    pub rationale: String,
}

/// A single corpus sample: log entries + LLM verdict
pub struct CorpusSample {
    pub entries: Vec<LogEntry>,
    pub verdict: evaluate::EvalOutcome,
    pub reason: String,
}

pub struct ValidationResult {
    pub agreements: usize,
    pub disagreements: usize,
    pub agreement_rate: f64,
    pub details: Vec<ValidationDetail>,
}

pub struct ValidationDetail {
    pub sample_index: usize,
    pub mechanical: String,
    pub llm: String,
    pub agrees: bool,
}

impl EntryMatcher {
    fn matches(&self, entry: &LogEntry) -> bool {
        if let Some(ref role) = self.role {
            if entry.role != *role {
                return false;
            }
        }

        match &entry.content_type {
            EntryType::ToolUse { tool_name, input } => {
                if let Some(ref expected) = self.tool_name {
                    if tool_name != expected {
                        return false;
                    }
                }
                if let Some(ref pattern) = self.input_contains {
                    if !input.to_lowercase().contains(&pattern.to_lowercase()) {
                        return false;
                    }
                }
                if let Some(ref pattern) = self.content_contains {
                    if !entry.content.to_lowercase().contains(&pattern.to_lowercase()) {
                        return false;
                    }
                }
                true
            }
            EntryType::Text => {
                if self.tool_name.is_some() {
                    return false;
                }
                if let Some(ref pattern) = self.content_contains {
                    if !entry.content.to_lowercase().contains(&pattern.to_lowercase()) {
                        return false;
                    }
                }
                true
            }
            EntryType::ToolResult { output, .. } => {
                if self.tool_name.is_some() {
                    return false;
                }
                if let Some(ref pattern) = self.content_contains {
                    if !output.to_lowercase().contains(&pattern.to_lowercase()) {
                        return false;
                    }
                }
                true
            }
            _ => {
                if self.tool_name.is_some() {
                    return false;
                }
                if let Some(ref pattern) = self.content_contains {
                    if !entry.content.to_lowercase().contains(&pattern.to_lowercase()) {
                        return false;
                    }
                }
                true
            }
        }
    }
}

/// Execute a CheckSpec against log entries. Returns true for pass, false for fail.
pub fn execute_check(entries: &[LogEntry], spec: &CheckSpec) -> bool {
    match spec {
        CheckSpec::FileRead { path_pattern } => entries.iter().any(|e| {
            matches!(&e.content_type, EntryType::ToolUse { tool_name, input }
                if (tool_name == "Read" || tool_name == "Bash")
                && input.contains(path_pattern.as_str()))
        }),

        CheckSpec::CommandRun { command } => entries.iter().any(|e| {
            matches!(&e.content_type, EntryType::ToolUse { tool_name, input }
                if tool_name == "Bash" && input.contains(command.as_str()))
        }),

        CheckSpec::TextContains { keyword } => {
            let kw_lower = keyword.to_lowercase();
            entries.iter().any(|e| {
                e.role == "assistant"
                    && matches!(&e.content_type, EntryType::Text)
                    && e.content.to_lowercase().contains(&kw_lower)
            })
        }

        CheckSpec::NoToolOnPath { path_pattern } => !entries.iter().any(|e| {
            matches!(&e.content_type, EntryType::ToolUse { tool_name, input }
                if (tool_name == "Edit" || tool_name == "Write")
                && input.contains(path_pattern.as_str()))
        }),

        CheckSpec::NoBashCommand { pattern } => !entries.iter().any(|e| {
            matches!(&e.content_type, EntryType::ToolUse { tool_name, input }
                if tool_name == "Bash" && input.contains(pattern.as_str()))
        }),

        CheckSpec::ToolSequence { trigger, followups } => {
            let trigger_idx = entries.iter().position(|e| trigger.matches(e));
            match trigger_idx {
                None => true, // vacuously true
                Some(idx) => {
                    let after = &entries[idx..];
                    followups.iter().all(|f| after.iter().any(|e| f.matches(e)))
                }
            }
        }

        CheckSpec::NoBashSyntax => !entries.iter().any(|e| {
            e.role == "assistant"
                && matches!(&e.content_type, EntryType::Text)
                && evaluate::text_has_bare_bash_operators(&e.content)
        }),

        CheckSpec::MultipleEdits {
            path_pattern,
            min_count,
        } => {
            let count = entries
                .iter()
                .filter(|e| {
                    matches!(&e.content_type, EntryType::ToolUse { tool_name, input }
                    if tool_name == "Edit" && input.contains(path_pattern.as_str()))
                })
                .count();
            count >= *min_count
        }

        CheckSpec::NoReadOnPath { path_pattern } => !entries.iter().any(|e| {
            matches!(&e.content_type, EntryType::ToolUse { tool_name, input }
                if tool_name == "Read" && input.contains(path_pattern.as_str()))
        }),

        CheckSpec::NoWriteOnPath { path_pattern } => !entries.iter().any(|e| {
            matches!(&e.content_type, EntryType::ToolUse { tool_name, input }
                if tool_name == "Write" && input.contains(path_pattern.as_str()))
        }),

        CheckSpec::NoDaypageWrite => !entries.iter().any(|e| {
            matches!(&e.content_type, EntryType::ToolUse { tool_name, input }
                if (tool_name == "Write" || tool_name == "Edit")
                && input.contains("DayPages/"))
        }),

        CheckSpec::Custom { .. } => false, // can't execute mechanically
    }
}

/// Build the analysis prompt that asks the LLM to identify a structural pattern
pub fn build_analysis_prompt(
    assertion: &Assertion,
    pass_summaries: &[String],
    fail_summaries: &[String],
) -> String {
    let mut prompt = format!(
        r#"You are analyzing AI conversation logs to find a deterministic, structural check that can replace LLM evaluation for an assertion.

## Assertion
{}: {}

"#,
        assertion.id, assertion.assert_text
    );

    prompt.push_str("## PASS examples (the assertion was satisfied)\n");
    for (i, summary) in pass_summaries.iter().enumerate() {
        prompt.push_str(&format!("### Example {}\n```\n{}\n```\n\n", i + 1, summary));
    }

    prompt.push_str("## FAIL examples (the assertion was NOT satisfied)\n");
    for (i, summary) in fail_summaries.iter().enumerate() {
        prompt.push_str(&format!("### Example {}\n```\n{}\n```\n\n", i + 1, summary));
    }

    prompt.push_str(
        r#"## Available check types

Each maps to a deterministic function examining conversation log structure (tool calls, file paths, text content) — no LLM needed at eval time.

1. **file_read** — Pass if Read or Bash tool was used on a path containing the pattern
   `{ "check_type": "file_read", "path_pattern": "ASSISTANT-HANDOFF" }`

2. **command_run** — Pass if Bash tool contained the command string
   `{ "check_type": "command_run", "command": "date" }`

3. **text_contains** — Pass if assistant text contains the keyword (case-insensitive)
   `{ "check_type": "text_contains", "keyword": "Risk" }`

4. **no_tool_on_path** — Pass if Edit/Write was NOT used on the path pattern
   `{ "check_type": "no_tool_on_path", "path_pattern": "/.config/" }`

5. **no_bash_command** — Pass if Bash tool did NOT contain the pattern
   `{ "check_type": "no_bash_command", "pattern": "push --force" }`

6. **tool_sequence** — Pass if trigger condition implies followup actions occurred.
   Trigger and followups are entry matchers with optional fields: role, tool_name, content_contains, input_contains.
   If trigger never fires, result is Pass (vacuously true).
   `{ "check_type": "tool_sequence", "trigger": { "role": "user", "content_contains": "goodbye" }, "followups": [{ "tool_name": "Bash", "input_contains": "daypage-append" }] }`

7. **no_bash_syntax** — Pass if assistant text has no bare && or || outside bash -c wrappers
   `{ "check_type": "no_bash_syntax" }`

8. **multiple_edits** — Pass if Edit tool was used on path pattern >= min_count times
   `{ "check_type": "multiple_edits", "path_pattern": "Clinical/clients/", "min_count": 2 }`

9. **no_read_on_path** — Pass if Read was NOT used on path pattern
   `{ "check_type": "no_read_on_path", "path_pattern": "/private/" }`

10. **no_write_on_path** — Pass if Write (not Edit) was NOT used on path pattern
    `{ "check_type": "no_write_on_path", "path_pattern": "Clinical/clients/" }`

11. **no_daypage_write** — Pass if Write/Edit was NOT used on DayPages/
    `{ "check_type": "no_daypage_write" }`

12. **custom** — Use ONLY when no structural signal exists (genuinely subjective quality judgment)
    `{ "check_type": "custom", "description": "why this can't be mechanicalized" }`

## Task

Identify the structural feature in the conversation log that reliably separates PASS from FAIL results. Look for:
- Presence/absence of specific tool calls (Read, Edit, Write, Bash, Grep, Glob)
- Specific file paths or path patterns in tool inputs
- Keywords or phrases in assistant text output
- Sequences of actions (trigger then required followup)
- Patterns in Bash command inputs

Output a single JSON object matching one of the check types above. Include two additional fields:
- "description": one sentence describing what the check looks for
- "rationale": why this structural feature reliably distinguishes PASS from FAIL

If the distinction is genuinely subjective (about quality, nuance, clinical judgment, writing style) with no structural log signal, use the "custom" type.

Output JSON only, no markdown fencing:"#,
    );

    prompt
}

/// Parse the LLM analyzer response into a ProposedCheck
pub fn parse_proposal(response: &str, assertion_id: &str) -> Result<ProposedCheck> {
    let clean = crate::log_parser::strip_fences(response.trim());

    let json_str =
        extract_json_object(clean).context("No JSON object found in analyzer response")?;

    let v: serde_json::Value = serde_json::from_str(json_str)
        .with_context(|| format!("Failed to parse analyzer JSON: {}", json_str))?;

    let description = v
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();
    let rationale = v
        .get("rationale")
        .and_then(|r| r.as_str())
        .unwrap_or("")
        .to_string();

    let spec: CheckSpec = serde_json::from_value(v.clone()).with_context(|| {
        let check_type = v
            .get("check_type")
            .and_then(|c| c.as_str())
            .unwrap_or("(missing)");
        format!(
            "Failed to parse CheckSpec (check_type: {}): {}",
            check_type, json_str
        )
    })?;

    Ok(ProposedCheck {
        assertion_id: assertion_id.to_string(),
        spec,
        description,
        rationale,
    })
}

/// Validate a proposed check against the corpus. Skips N/A samples.
pub fn validate(proposal: &ProposedCheck, corpus: &[CorpusSample]) -> ValidationResult {
    let mut agreements = 0;
    let mut disagreements = 0;
    let mut details = Vec::new();

    for (i, sample) in corpus.iter().enumerate() {
        if sample.verdict == evaluate::EvalOutcome::NotApplicable {
            continue;
        }

        let mechanical_pass = execute_check(&sample.entries, &proposal.spec);
        let llm_pass = sample.verdict == evaluate::EvalOutcome::Pass;

        let agrees = mechanical_pass == llm_pass;
        if agrees {
            agreements += 1;
        } else {
            disagreements += 1;
        }

        details.push(ValidationDetail {
            sample_index: i,
            mechanical: if mechanical_pass { "PASS" } else { "FAIL" }.to_string(),
            llm: match sample.verdict {
                evaluate::EvalOutcome::Pass => "PASS",
                evaluate::EvalOutcome::Fail => "FAIL",
                evaluate::EvalOutcome::NotApplicable => "N/A",
            }
            .to_string(),
            agrees,
        });
    }

    let total = agreements + disagreements;
    let agreement_rate = if total > 0 {
        agreements as f64 / total as f64
    } else {
        0.0
    };

    ValidationResult {
        agreements,
        disagreements,
        agreement_rate,
        details,
    }
}

/// Extract the first JSON object from a string, handling nesting
fn extract_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let bytes = s.as_bytes();
    let mut depth = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for i in start..bytes.len() {
        if escape_next {
            escape_next = false;
            continue;
        }
        match bytes[i] {
            b'\\' if in_string => escape_next = true,
            b'"' => in_string = !in_string,
            b'{' if !in_string => depth += 1,
            b'}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_object() {
        let input = r#"Here is the check: {"check_type": "text_contains", "keyword": "Risk"} done"#;
        let result = extract_json_object(input).unwrap();
        assert!(result.contains("text_contains"));
        assert!(result.starts_with('{'));
        assert!(result.ends_with('}'));
    }

    #[test]
    fn test_extract_json_nested() {
        let input = r#"{"check_type": "tool_sequence", "trigger": {"role": "user"}, "followups": [{"tool_name": "Bash"}]}"#;
        let result = extract_json_object(input).unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn test_entry_matcher_text() {
        let matcher = EntryMatcher {
            role: Some("assistant".to_string()),
            tool_name: None,
            content_contains: Some("hello".to_string()),
            input_contains: None,
        };

        let entry = LogEntry {
            role: "assistant".to_string(),
            content_type: EntryType::Text,
            content: "Hello world".to_string(),
            timestamp: None,
        };

        assert!(matcher.matches(&entry));
    }

    #[test]
    fn test_entry_matcher_tool() {
        let matcher = EntryMatcher {
            role: None,
            tool_name: Some("Bash".to_string()),
            content_contains: None,
            input_contains: Some("daypage-append".to_string()),
        };

        let entry = LogEntry {
            role: "assistant".to_string(),
            content_type: EntryType::ToolUse {
                tool_name: "Bash".to_string(),
                input: "daypage-append \"dev:: test\"".to_string(),
            },
            content: String::new(),
            timestamp: None,
        };

        assert!(matcher.matches(&entry));
    }

    #[test]
    fn test_execute_text_contains() {
        let spec = CheckSpec::TextContains {
            keyword: "risk".to_string(),
        };

        let entries = vec![LogEntry {
            role: "assistant".to_string(),
            content_type: EntryType::Text,
            content: "**Risk Assessment:** Low risk of harm.".to_string(),
            timestamp: None,
        }];

        assert!(execute_check(&entries, &spec));
    }

    #[test]
    fn test_execute_tool_sequence_vacuous() {
        let spec = CheckSpec::ToolSequence {
            trigger: EntryMatcher {
                role: Some("user".to_string()),
                tool_name: None,
                content_contains: Some("goodbye".to_string()),
                input_contains: None,
            },
            followups: vec![EntryMatcher {
                role: None,
                tool_name: Some("Bash".to_string()),
                content_contains: None,
                input_contains: Some("daypage-append".to_string()),
            }],
        };

        // No trigger = vacuously true
        let entries = vec![LogEntry {
            role: "user".to_string(),
            content_type: EntryType::Text,
            content: "Hello".to_string(),
            timestamp: None,
        }];

        assert!(execute_check(&entries, &spec));
    }

    #[test]
    fn test_parse_check_spec_text_contains() {
        let json = r#"{"check_type": "text_contains", "keyword": "ACT", "description": "test", "rationale": "test"}"#;
        let spec: CheckSpec = serde_json::from_str(json).unwrap();
        assert!(matches!(spec, CheckSpec::TextContains { keyword } if keyword == "ACT"));
    }

    #[test]
    fn test_parse_check_spec_custom() {
        let json = r#"{"check_type": "custom", "description": "requires clinical judgment"}"#;
        let spec: CheckSpec = serde_json::from_str(json).unwrap();
        assert!(matches!(spec, CheckSpec::Custom { .. }));
    }
}
