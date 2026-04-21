// Policy definition + application logic.
//
// A policy has:
//   - match criteria (from, subject, subject_not) — all must hold
//   - on_arrival: tags to add to newly-seen messages (marked by curator-<name>-seen)
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

    /// Archive (remove `inbox` tag) after this many days from message date.
    /// Omit to never archive based on age.
    pub archive_after_days: Option<u32>,

    /// Trash (add `trash`, remove `inbox`) after this many days from message date.
    /// Omit to never trash based on age.
    pub delete_after_days: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct MatchSpec {
    /// Notmuch from: query fragment (e.g. "@royalmail.com" or "noreply@zoom.us")
    pub from: Option<String>,

    /// Regex matched against Subject header (via notmuch subject: query after anchoring).
    /// Notmuch's subject: is whitespace-token-based; for true regex we'd need post-filtering.
    /// For v1, we use notmuch's native subject: search with term(s); see build_match_query.
    pub subject_contains: Option<String>,

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
    pub archived: u64,
    pub deleted: u64,
}

impl Policy {
    pub fn summary(&self) -> String {
        let parts = [
            self.r#match.from.as_ref().map(|s| format!("from={s}")),
            self.r#match.subject_contains.as_ref().map(|s| format!("subject~{s}")),
            self.archive_after_days.map(|d| format!("archive@{d}d")),
            self.delete_after_days.map(|d| format!("delete@{d}d")),
        ];
        parts.iter().flatten().cloned().collect::<Vec<_>>().join(", ")
    }

    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty() {
            anyhow::bail!("name is empty");
        }
        if self.r#match.from.is_none() && self.r#match.subject_contains.is_none() {
            anyhow::bail!("must have at least one of match.from or match.subject_contains");
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
        {
            anyhow::bail!("policy does nothing: no on_arrival tags and no lifecycle thresholds");
        }
        Ok(())
    }

    fn seen_tag(&self) -> String {
        format!("curator-{}-seen", self.name)
    }

    /// Build the notmuch query that matches this policy's criteria
    /// (without the seen/lifecycle qualifiers).
    fn base_query(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(f) = &self.r#match.from {
            parts.push(format!("from:{f}"));
        }
        if let Some(s) = &self.r#match.subject_contains {
            parts.push(format!("subject:\"{s}\""));
        }
        if let Some(s) = &self.r#match.subject_not_contains {
            parts.push(format!("not subject:\"{s}\""));
        }
        parts.join(" and ")
    }
}

/// Apply a policy: handle on_arrival (new matches) + lifecycle transitions.
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
    if let Some(days) = pol.delete_after_days {
        let q = format!(
            "({base}) and not tag:trash and date:..{days}d"
        );
        let n = notmuch::count(&q)?;
        if n > 0 {
            stats.deleted = n;
            if !dry_run {
                notmuch::apply_tag_changes(&q, &["trash"], &["inbox"])?;
            }
        }
    }

    Ok(stats)
}
