use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};

/// Top-level activity report for a single day (from continuum-activity --json).
#[derive(Debug, Deserialize)]
pub struct DayActivity {
    pub date: String,
    pub cc_sessions: Vec<CcSession>,
    pub continuum_sessions: Vec<ContinuumSession>,
}

/// A CC session with rich extracted data.
#[derive(Debug, Deserialize)]
pub struct CcSession {
    pub session_id: String,
    pub slug: String,
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    pub skills: Vec<String>,
    /// file_path -> edit count
    pub files_modified: BTreeMap<String, u32>,
    /// tool_name -> use count
    pub tool_usage: BTreeMap<String, u32>,
    /// (timestamp, user message text)
    pub user_messages: Vec<(DateTime<Utc>, String)>,
}

/// A session from the Continuum archive (ChatGPT, Grok, Gemini, etc.).
#[derive(Debug, Deserialize)]
pub struct ContinuumSession {
    pub assistant: String,
    pub session_id: String,
    pub title: Option<String>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub message_count: Option<u32>,
}

/// A parsed dev:: entry from a DayPage.
#[derive(Debug)]
pub struct DevEntry {
    pub raw: String,
    pub terms: HashSet<String>,
}

/// A session (either CC or Continuum) unified for matching.
#[derive(Debug)]
pub struct UnifiedSession {
    pub source: SessionSource,
    pub session_id: String,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub message_count: usize,
    pub terms: HashSet<String>,
    pub files_summary: String,
    pub skills_summary: String,
    /// Full session data for drafting prompt
    pub detail: String,
}

#[derive(Debug, Clone)]
pub enum SessionSource {
    Cc,
    Continuum(String), // assistant name
}

/// Match result for a session.
#[derive(Debug)]
pub enum MatchResult {
    Matched {
        entry_raw: String,
        overlap_terms: Vec<String>,
    },
    Unmatched,
    Trivial,
    /// Clinical session — not dev work, skip drafting
    Clinical,
}

/// A drafted entry from claude -p.
#[derive(Debug, Clone)]
pub struct DraftEntry {
    pub date: NaiveDate,
    pub entry: String,
}

/// Report for a single day.
#[derive(Debug)]
pub struct DayReport {
    pub date: NaiveDate,
    pub sessions: Vec<(UnifiedSession, MatchResult)>,
    pub drafts: Vec<DraftEntry>,
}
