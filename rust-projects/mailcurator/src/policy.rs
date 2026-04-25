// Policy definition + application logic.
//
// A policy has:
//   - match criteria (from, subject, subject_not) — all must hold
//   - on_arrival: tags to add to newly-seen messages (marked by curator-<name>-seen)
//   - quarantine flag: when true, skip archive/delete (observe-only mode)
//   - archive_after: remove `inbox` tag after N days
//   - delete_after: add `trash` tag (and remove `inbox`) after N days
//
// The "seen" marker lets us distinguish "never processed" (apply on_arrival)
// from "already processed" (only check age-based lifecycle transitions).
//
// All operations are idempotent: re-running with the same input does nothing.

use anyhow::Result;
use regex::Regex;
use serde::Deserialize;

use crate::notmuch;

#[derive(Debug, Deserialize)]
pub struct Policy {
    pub name: String,

    #[serde(flatten)]
    pub r#match: MatchSpec,

    #[serde(default)]
    pub on_arrival: OnArrival,

    /// Observe-only mode. Messages get tagged with the seen-tag (and any
    /// on_arrival tags) but archive/delete phases are skipped. Use when
    /// piloting a new policy to check what it catches without destroying
    /// mail. Flip to false once you're happy with the matches.
    #[serde(default)]
    pub quarantine: bool,

    /// Archive (remove `inbox` tag) after this many days from message date.
    /// Omit to never archive based on age.
    pub archive_after_days: Option<u32>,

    /// Trash (add `trash`, remove `inbox`) after this many days from message date.
    /// Omit to never trash based on age.
    pub delete_after_days: Option<u32>,

    /// Karpathy-loop: which labelled categories this policy intends to match.
    /// Used by `mailcurator eval` to compute precision/recall against the
    /// labelled corpus. Omit for policies not yet part of the eval loop.
    #[serde(default)]
    pub intended_categories: Vec<String>,

    /// Content extractors. Each runs over messages this policy matches and
    /// writes structured records to ~/.local/share/mailcurator/<category>.jsonl.
    /// When extractors are present, `delete_after_days` is gated on the
    /// `curator-<name>-extracted` tag so we never destroy unextracted data.
    #[serde(default, rename = "extractor")]
    pub extractors: Vec<Extractor>,
}

#[derive(Debug, Deserialize)]
pub struct Extractor {
    /// JSONL file basename (without .jsonl). Records appended to
    /// ~/.local/share/mailcurator/<category>.jsonl.
    pub category: String,

    /// Field-extraction rules. Each rule produces one named field in the
    /// output JSON object. Order doesn't matter; missing fields are simply
    /// omitted from the record (partial extraction is OK).
    #[serde(default, rename = "field")]
    pub fields: Vec<FieldRule>,
}

/// A single field-extraction rule. Exactly one of (literal, header,
/// body_regex, subject_regex) should be set; validate() enforces this.
#[derive(Debug, Deserialize)]
pub struct FieldRule {
    pub name: String,

    /// Hard-coded value (e.g. {literal = "Royal Mail"} for the carrier field
    /// in a Royal Mail extractor).
    pub literal: Option<String>,

    /// Pull from a named header (case-insensitive: "Subject", "From", "Date").
    pub header: Option<String>,

    /// Regex applied to the decoded, HTML-stripped message body.
    /// First capture group becomes the field value; if no groups, the whole
    /// match is used.
    pub body_regex: Option<String>,

    /// Regex applied to the Subject header. First capture group wins.
    pub subject_regex: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MatchSpec {
    /// Notmuch from: query fragment (e.g. "@royalmail.com" or "noreply@zoom.us")
    pub from: Option<String>,

    /// Single token/phrase match against Subject header.
    pub subject_contains: Option<String>,

    /// OR-of-substrings match against Subject header. Any one matching causes
    /// the message to match. Use for consolidating e.g. verification-code
    /// patterns into a single policy.
    pub subject_contains_any: Option<Vec<String>>,

    /// Convenience: exclude subjects containing this token.
    pub subject_not_contains: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct OnArrival {
    #[serde(default)]
    pub tags_add: Vec<String>,

    #[serde(default)]
    pub tags_remove: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Stats {
    pub tagged_on_arrival: u64,
    pub extracted: u64,
    pub archived: u64,
    pub deleted: u64,
}

impl Policy {
    pub fn summary(&self) -> String {
        let parts = [
            self.r#match.from.as_ref().map(|s| format!("from={s}")),
            self.r#match.subject_contains.as_ref().map(|s| format!("subject~{s}")),
            self.r#match.subject_contains_any.as_ref()
                .map(|v| format!("subject_any~[{}]", v.join("|"))),
            if self.quarantine { Some("QUARANTINE".into()) } else { None },
            self.archive_after_days.map(|d| format!("archive@{d}d")),
            self.delete_after_days.map(|d| format!("delete@{d}d")),
        ];
        parts.iter().flatten().cloned().collect::<Vec<_>>().join(", ")
    }

    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty() {
            anyhow::bail!("name is empty");
        }
        if self.r#match.from.is_none()
            && self.r#match.subject_contains.is_none()
            && self.r#match.subject_contains_any.is_none()
        {
            anyhow::bail!(
                "must have at least one of match.from, match.subject_contains, or match.subject_contains_any"
            );
        }
        if let Some(v) = &self.r#match.subject_contains_any {
            if v.is_empty() {
                anyhow::bail!("subject_contains_any is empty — omit the field entirely if no patterns");
            }
        }
        // Ensure name is a valid tag suffix (safe characters only).
        let re = Regex::new(r"^[a-z0-9][a-z0-9-]*$").unwrap();
        if !re.is_match(&self.name) {
            anyhow::bail!(
                "name must be lowercase alphanumeric with dashes (got: {})",
                self.name
            );
        }
        if self.archive_after_days.is_none() && self.delete_after_days.is_none()
            && self.on_arrival.tags_add.is_empty() && self.on_arrival.tags_remove.is_empty()
            && self.extractors.is_empty()
        {
            anyhow::bail!("policy does nothing: no on_arrival tags, no lifecycle thresholds, no extractors");
        }
        for ex in &self.extractors {
            for f in &ex.fields {
                let set_count = [
                    f.literal.is_some(), f.header.is_some(),
                    f.body_regex.is_some(), f.subject_regex.is_some(),
                ].iter().filter(|b| **b).count();
                if set_count != 1 {
                    anyhow::bail!(
                        "field '{}' in extractor '{}': must set exactly one of literal/header/body_regex/subject_regex (got {})",
                        f.name, ex.category, set_count
                    );
                }
            }
        }
        Ok(())
    }

    /// Tag marker for "this policy has run its extractors against this message."
    pub fn extracted_tag(&self) -> String {
        format!("curator-{}-extracted", self.name)
    }

    pub fn seen_tag(&self) -> String {
        format!("curator-{}-seen", self.name)
    }

    /// Build the notmuch query that matches this policy's criteria
    /// (without the seen/lifecycle qualifiers).
    pub fn base_query(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(f) = &self.r#match.from {
            parts.push(format!("from:{f}"));
        }
        if let Some(s) = &self.r#match.subject_contains {
            parts.push(format!("subject:\"{s}\""));
        }
        if let Some(terms) = &self.r#match.subject_contains_any {
            let or_parts: Vec<String> = terms.iter()
                .map(|t| format!("subject:\"{t}\""))
                .collect();
            parts.push(format!("({})", or_parts.join(" or ")));
        }
        if let Some(s) = &self.r#match.subject_not_contains {
            parts.push(format!("not subject:\"{s}\""));
        }
        parts.join(" and ")
    }
}

/// Apply a policy: handle on_arrival (new matches), extraction, then
/// lifecycle transitions. When `dry_run` is true, nothing is modified;
/// counts are still reported.
pub fn apply(pol: &Policy, dry_run: bool) -> Result<Stats> {
    let mut stats = Stats::default();
    let base = pol.base_query();
    let seen = pol.seen_tag();

    // --- on_arrival: messages matching but not yet marked seen ---
    let arrival_query = format!("({base}) and not tag:{seen}");
    let arrival_count = notmuch::count(&arrival_query)?;
    if arrival_count > 0 {
        stats.tagged_on_arrival = arrival_count;
        let mut add: Vec<&str> = pol.on_arrival.tags_add.iter().map(|s| s.as_str()).collect();
        add.push(&seen);
        let remove: Vec<&str> = pol.on_arrival.tags_remove.iter().map(|s| s.as_str()).collect();
        if !dry_run {
            notmuch::apply_tag_changes(&arrival_query, &add, &remove)?;
        }
    }

    // --- extraction: per-message structured-data capture ---
    // Runs over (base) and not extracted-tag. Independent of seen — extraction
    // applies retroactively to existing matches that haven't been processed.
    if !pol.extractors.is_empty() {
        stats.extracted = crate::extract::run_extractors(pol, dry_run)?;
    }

    // Quarantine short-circuits archive/delete — observe only.
    if pol.quarantine {
        return Ok(stats);
    }

    // --- archive: matching messages past the age threshold, still in inbox ---
    if let Some(days) = pol.archive_after_days {
        let q = format!(
            "({base}) and tag:inbox and date:..{days}d and not tag:trash"
        );
        let n = notmuch::count(&q)?;
        if n > 0 {
            stats.archived = n;
            if !dry_run {
                notmuch::apply_tag_changes(&q, &[], &["inbox"])?;
            }
        }
    }

    // --- delete: matching messages past the trash threshold ---
    // If this policy declares extractors, gate deletion on the extracted-tag:
    // we never destroy messages whose data hasn't been captured.
    if let Some(days) = pol.delete_after_days {
        let q = if pol.extractors.is_empty() {
            format!("({base}) and not tag:trash and date:..{days}d")
        } else {
            let extracted = pol.extracted_tag();
            format!(
                "({base}) and not tag:trash and date:..{days}d and tag:{extracted}"
            )
        };
        let n = notmuch::count(&q)?;
        if n > 0 {
            stats.deleted = n;
            if !dry_run {
                // Log deletions BEFORE applying the tag, so the audit
                // trail captures what was actually trashed.
                if let Err(e) = crate::store::log_deletions(&pol.name, &q) {
                    eprintln!("mailcurator: warning — failed to log deletions: {e}");
                }
                notmuch::apply_tag_changes(&q, &["trash"], &["inbox"])?;
            }
        }
    }

    Ok(stats)
}
