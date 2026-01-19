use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write as _};
use std::path::PathBuf;

/// Safely truncate a string at a character boundary
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max_chars).collect::<String>())
    }
}

#[derive(Parser)]
#[command(name = "grok-continuum-manage")]
#[command(about = "Manage imported Grok conversations in continuum")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Continuum logs directory (default: ~/Assistants/continuum-logs/grok)
    #[arg(short, long)]
    logs_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// List all imported conversations
    List,

    /// Interactively review and delete conversations
    Review,

    /// Delete specific conversation by ID
    Delete {
        /// Conversation ID to delete
        conversation_id: String,

        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,
    },

    /// Preview export file without importing
    Preview {
        /// Path to prod-grok-backend.json file
        export_file: PathBuf,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct ContinuumSession {
    id: String,
    assistant: String,
    start_time: Option<String>,
    end_time: Option<String>,
    status: Option<String>,
    message_count: Option<u32>,
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContinuumMessage {
    id: u32,
    role: String,
    content: String,
    timestamp: String,
}

// Grok export structures (for preview)
#[derive(Debug, Deserialize)]
struct GrokExport {
    conversations: Vec<ConversationWrapper>,
}

#[derive(Debug, Deserialize)]
struct ConversationWrapper {
    conversation: Conversation,
    responses: Vec<ResponseWrapper>,
}

#[derive(Debug, Deserialize)]
struct Conversation {
    id: String,
    title: String,
    create_time: String,
    #[serde(default)]
    media_types: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ResponseWrapper {
    response: Response,
}

#[derive(Debug, Deserialize)]
struct Response {
    message: String,
    sender: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let logs_dir = cli.logs_dir.unwrap_or_else(|| {
        let home = std::env::var("HOME").expect("HOME not set");
        PathBuf::from(home).join("Assistants").join("continuum-logs").join("grok")
    });

    match cli.command {
        Commands::List => list_conversations(&logs_dir),
        Commands::Review => review_conversations(&logs_dir),
        Commands::Delete { conversation_id, force } => {
            delete_conversation(&logs_dir, &conversation_id, force)
        }
        Commands::Preview { export_file } => preview_export(&export_file),
    }
}

fn list_conversations(logs_dir: &PathBuf) -> Result<()> {
    let sessions = find_all_sessions(logs_dir)?;

    if sessions.is_empty() {
        println!("No imported Grok conversations found in {:?}", logs_dir);
        return Ok(());
    }

    println!("Found {} imported conversations:\n", sessions.len());

    for (path, session) in sessions {
        println!("═══════════════════════════════════════════════════════════════");
        println!("ID: {}", session.id);
        println!("Created: {}", session.created_at.as_deref().unwrap_or("unknown"));
        println!("Messages: {}", session.message_count.unwrap_or(0));
        println!("Path: {:?}", path.parent().unwrap());

        // Show first message if available
        if let Ok(messages) = load_messages(&path.parent().unwrap().join("messages.jsonl")) {
            if let Some(first_msg) = messages.first() {
                let preview = truncate_str(&first_msg.content, 100);
                println!("Preview: {}", preview);
            }
        }
        println!();
    }

    Ok(())
}

fn review_conversations(logs_dir: &PathBuf) -> Result<()> {
    let sessions = find_all_sessions(logs_dir)?;

    if sessions.is_empty() {
        println!("No imported Grok conversations found in {:?}", logs_dir);
        return Ok(());
    }

    println!("Reviewing {} conversations...\n", sessions.len());

    let mut deleted_count = 0;
    let mut kept_count = 0;
    let total_count = sessions.len();

    for (path, session) in sessions {
        println!("═══════════════════════════════════════════════════════════════");
        println!("Conversation {}/{}", deleted_count + kept_count + 1, total_count);
        println!("───────────────────────────────────────────────────────────────");
        println!("ID: {}", session.id);
        println!("Created: {}", session.created_at.as_deref().unwrap_or("unknown"));
        println!("Messages: {}", session.message_count.unwrap_or(0));
        println!();

        // Show messages preview
        if let Ok(messages) = load_messages(&path.parent().unwrap().join("messages.jsonl")) {
            let preview_count = messages.len().min(5);
            for msg in messages.iter().take(preview_count) {
                let role = msg.role.to_uppercase();
                let content = truncate_str(&msg.content, 150);
                println!("  [{}] {}", role, content);
            }

            if messages.len() > preview_count {
                println!("  ... ({} more messages)", messages.len() - preview_count);
            }
        }
        println!();

        // Prompt for deletion
        loop {
            print!("Delete this conversation? [y/n/q]: ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;

            match input.trim().to_lowercase().as_str() {
                "y" | "yes" => {
                    let session_dir = path.parent().unwrap();
                    fs::remove_dir_all(session_dir)
                        .with_context(|| format!("Failed to delete {:?}", session_dir))?;
                    println!("✓ Deleted\n");
                    deleted_count += 1;
                    break;
                }
                "n" | "no" => {
                    println!("Kept\n");
                    kept_count += 1;
                    break;
                }
                "q" | "quit" => {
                    println!("\nReview summary:");
                    println!("  Deleted: {}", deleted_count);
                    println!("  Kept: {}", kept_count);
                    return Ok(());
                }
                _ => {
                    println!("Please enter y (yes), n (no), or q (quit)");
                }
            }
        }
    }

    println!("Review complete!");
    println!("  Deleted: {}", deleted_count);
    println!("  Kept: {}", kept_count);

    Ok(())
}

fn delete_conversation(logs_dir: &PathBuf, conversation_id: &str, force: bool) -> Result<()> {
    let sessions = find_all_sessions(logs_dir)?;

    let session_path = sessions.iter()
        .find(|(_, s)| s.id == conversation_id)
        .map(|(p, _)| p.parent().unwrap())
        .with_context(|| format!("Conversation '{}' not found", conversation_id))?;

    if !force {
        print!("Delete conversation '{}' at {:?}? [y/N]: ", conversation_id, session_path);
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("Cancelled");
            return Ok(());
        }
    }

    fs::remove_dir_all(session_path)
        .with_context(|| format!("Failed to delete {:?}", session_path))?;

    println!("✓ Deleted conversation '{}'", conversation_id);

    Ok(())
}

fn preview_export(export_file: &PathBuf) -> Result<()> {
    println!("Reading Grok export: {:?}\n", export_file);

    let json_content = fs::read_to_string(export_file)
        .context("Failed to read export file")?;

    let export: GrokExport = serde_json::from_str(&json_content)
        .context("Failed to parse export file")?;

    println!("Found {} conversations\n", export.conversations.len());

    for (idx, conv_wrapper) in export.conversations.iter().enumerate() {
        let conv = &conv_wrapper.conversation;

        println!("═══════════════════════════════════════════════════════════════");
        println!("Conversation {}/{}", idx + 1, export.conversations.len());
        println!("───────────────────────────────────────────────────────────────");
        println!("ID: {}", conv.id);
        println!("Title: {}", conv.title);
        println!("Date: {}", conv.create_time);

        if !conv.media_types.is_empty() {
            println!("Media: {}", conv.media_types.join(", "));
        }

        println!("Messages: {}", conv_wrapper.responses.len());
        println!();

        // Show first 3 messages
        let preview_count = conv_wrapper.responses.len().min(3);
        for resp_wrapper in conv_wrapper.responses.iter().take(preview_count) {
            let resp = &resp_wrapper.response;
            let role = match resp.sender.as_str() {
                "human" => "USER",
                "assistant" => "ASSISTANT",
                _ => resp.sender.as_str(),
            };

            let content = truncate_str(&resp.message, 150);
            println!("  [{}] {}", role, content);
        }

        if conv_wrapper.responses.len() > preview_count {
            println!("  ... ({} more messages)", conv_wrapper.responses.len() - preview_count);
        }

        println!();
    }

    Ok(())
}

fn find_all_sessions(logs_dir: &PathBuf) -> Result<Vec<(PathBuf, ContinuumSession)>> {
    let mut sessions = Vec::new();

    if !logs_dir.exists() {
        return Ok(sessions);
    }

    for date_entry in fs::read_dir(logs_dir)? {
        let date_entry = date_entry?;
        if !date_entry.file_type()?.is_dir() {
            continue;
        }

        for session_entry in fs::read_dir(date_entry.path())? {
            let session_entry = session_entry?;
            if !session_entry.file_type()?.is_dir() {
                continue;
            }

            let session_path = session_entry.path().join("session.json");
            if !session_path.exists() {
                continue;
            }

            let session_content = fs::read_to_string(&session_path)?;
            let session: ContinuumSession = serde_json::from_str(&session_content)?;
            sessions.push((session_path, session));
        }
    }

    Ok(sessions)
}

fn load_messages(messages_path: &PathBuf) -> Result<Vec<ContinuumMessage>> {
    let content = fs::read_to_string(messages_path)?;
    let messages: Vec<ContinuumMessage> = content
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    Ok(messages)
}
