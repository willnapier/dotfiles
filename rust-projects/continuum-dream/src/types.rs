use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// YAML frontmatter from a memory file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFrontmatter {
    pub name: String,
    pub description: String,
    #[serde(rename = "type")]
    pub memory_type: String,
}

/// A parsed memory file (frontmatter + body + path)
#[derive(Debug, Clone)]
pub struct MemoryFile {
    pub path: PathBuf,
    pub filename: String,
    pub frontmatter: MemoryFrontmatter,
    pub body: String,
    pub line_count: usize,
}

/// Current state of the memory system
#[derive(Debug)]
pub struct MemoryState {
    pub memory_dir: PathBuf,
    pub index_path: PathBuf,
    pub index_content: String,
    pub index_line_count: usize,
    pub files: Vec<MemoryFile>,
    pub orphaned_index_refs: Vec<String>,
    pub unindexed_files: Vec<String>,
}

/// A continuum session (simplified for context building)
#[derive(Debug, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub relative_path: String,
    pub assistant: String,
    pub date: String,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub message_count: usize,
    pub user_messages: Vec<String>,
    pub assistant_first_reply: String,
    pub skills: Vec<String>,
}

/// Session metadata from session.json
#[derive(Debug, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub assistant: String,
    #[serde(default)]
    pub start_time: Option<String>,
    #[serde(default)]
    pub end_time: Option<String>,
    #[serde(default)]
    pub message_count: Option<u32>,
    #[serde(default)]
    pub skills: Option<Vec<String>>,
}

/// A message from messages.jsonl
#[derive(Debug, Deserialize)]
pub struct LogMessage {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub timestamp: Option<String>,
}

/// The structured response expected from the AI
#[derive(Debug, Deserialize, Serialize)]
pub struct DreamResponse {
    pub files_to_update: Vec<FileUpdate>,
    pub files_to_create: Vec<FileCreate>,
    pub files_to_delete: Vec<FileDelete>,
    pub memory_index: String,
    pub summary: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FileUpdate {
    pub filename: String,
    pub content: String,
    pub reason: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FileCreate {
    pub filename: String,
    pub content: String,
    pub reason: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct FileDelete {
    pub filename: String,
    pub reason: String,
}

/// Persistent state between runs
#[derive(Debug, Serialize, Deserialize)]
pub struct DreamState {
    pub last_dream_time: Option<String>,
    pub last_session_count: usize,
    pub sessions_processed: Vec<String>,
    pub total_dreams: usize,
    pub last_dream_summary: Option<String>,
}

impl Default for DreamState {
    fn default() -> Self {
        Self {
            last_dream_time: None,
            last_session_count: 0,
            sessions_processed: Vec::new(),
            total_dreams: 0,
            last_dream_summary: None,
        }
    }
}

/// A proposed change for the diff display
#[derive(Debug)]
pub enum ProposedChange {
    UpdateFile {
        filename: String,
        old_content: String,
        new_content: String,
        reason: String,
    },
    CreateFile {
        filename: String,
        content: String,
        reason: String,
    },
    DeleteFile {
        filename: String,
        old_content: String,
        reason: String,
    },
    UpdateIndex {
        old_content: String,
        new_content: String,
    },
}
