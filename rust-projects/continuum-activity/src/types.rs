use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::Serialize;

/// A CC session with rich extracted data.
#[derive(Debug, Serialize)]
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
    /// (timestamp, truncated user message)
    pub user_messages: Vec<(DateTime<Utc>, String)>,
}

/// A session from the Continuum archive (ChatGPT, Grok, Gemini, etc.).
#[derive(Debug, Serialize)]
pub struct ContinuumSession {
    pub assistant: String,
    pub session_id: String,
    pub title: Option<String>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub message_count: Option<u32>,
}

/// Top-level activity report for a single day.
#[derive(Debug, Serialize)]
pub struct DayActivity {
    pub date: String,
    pub cc_sessions: Vec<CcSession>,
    pub continuum_sessions: Vec<ContinuumSession>,
}
