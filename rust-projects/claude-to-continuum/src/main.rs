use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "claude-to-continuum")]
#[command(about = "Convert Claude.ai export to continuum format")]
struct Cli {
    /// Path to Claude.ai conversations.json file
    conversations_json: PathBuf,

    /// Output directory (default: ~/continuum-logs/claude)
    #[arg(short, long)]
    output: Option<PathBuf>,
}

// Claude.ai export structures
#[derive(Debug, Deserialize)]
struct ClaudeConversation {
    uuid: String,
    name: String,
    created_at: String,
    updated_at: String,
    chat_messages: Vec<ClaudeMessage>,
}

#[derive(Debug, Deserialize)]
struct ClaudeMessage {
    uuid: String,
    text: String,
    sender: String,
    created_at: String,
}

// Continuum output structures
#[derive(Debug, Serialize)]
struct ContinuumMessage {
    id: u32,
    role: String,
    content: String,
    timestamp: String,
}

#[derive(Debug, Serialize)]
struct ContinuumSession {
    id: String,
    assistant: String,
    start_time: Option<String>,
    end_time: Option<String>,
    status: Option<String>,
    message_count: Option<u32>,
    created_at: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Determine output directory
    let output_dir = cli.output.unwrap_or_else(|| {
        let home = std::env::var("HOME").expect("HOME not set");
        PathBuf::from(home).join("continuum-logs").join("claude")
    });

    println!("Reading Claude.ai export: {:?}", cli.conversations_json);
    println!("Output directory: {:?}", output_dir);

    // Read and parse conversations.json
    let json_content = fs::read_to_string(&cli.conversations_json)
        .context("Failed to read conversations.json")?;

    let conversations: Vec<ClaudeConversation> = serde_json::from_str(&json_content)
        .context("Failed to parse conversations.json")?;

    println!("Found {} conversations", conversations.len());

    // Process each conversation
    let mut success_count = 0;
    let mut error_count = 0;

    for (idx, conversation) in conversations.iter().enumerate() {
        match process_conversation(conversation, &output_dir) {
            Ok(_) => success_count += 1,
            Err(e) => {
                eprintln!("Error processing conversation {}: {}", idx + 1, e);
                error_count += 1;
            }
        }

        if (idx + 1) % 100 == 0 {
            println!("Processed {} conversations...", idx + 1);
        }
    }

    println!("\nImport complete!");
    println!("  Success: {}", success_count);
    println!("  Errors:  {}", error_count);
    println!("  Output:  {:?}", output_dir);

    Ok(())
}

fn process_conversation(conv: &ClaudeConversation, output_dir: &PathBuf) -> Result<()> {
    // Parse the created_at timestamp
    let datetime: DateTime<Utc> = conv.created_at.parse()
        .context("Invalid timestamp")?;
    let date_str = datetime.format("%Y-%m-%d").to_string();

    // Create session directory
    let session_dir = output_dir.join(&date_str).join(&conv.uuid);
    fs::create_dir_all(&session_dir)
        .with_context(|| format!("Failed to create {:?}", session_dir))?;

    // Convert messages
    let messages = convert_messages(&conv.chat_messages)?;

    if messages.is_empty() {
        return Ok(()); // Skip empty conversations
    }

    // Write messages.jsonl
    let messages_path = session_dir.join("messages.jsonl");
    let mut jsonl_content = String::new();
    for msg in &messages {
        jsonl_content.push_str(&serde_json::to_string(msg)?);
        jsonl_content.push('\n');
    }
    fs::write(&messages_path, jsonl_content)?;

    // Parse update time
    let end_time: Option<DateTime<Utc>> = conv.updated_at.parse().ok();

    // Write session.json
    let session = ContinuumSession {
        id: conv.uuid.clone(),
        assistant: "claude".to_string(),
        start_time: Some(conv.created_at.clone()),
        end_time: end_time.map(|dt| dt.to_rfc3339()),
        status: Some("imported".to_string()),
        message_count: Some(messages.len() as u32),
        created_at: Some(conv.created_at.clone()),
    };

    let session_path = session_dir.join("session.json");
    let session_json = serde_json::to_string_pretty(&session)?;
    fs::write(&session_path, session_json)?;

    Ok(())
}

fn convert_messages(claude_messages: &[ClaudeMessage]) -> Result<Vec<ContinuumMessage>> {
    let mut messages = Vec::new();
    let mut msg_id = 1u32;

    for msg in claude_messages {
        // Skip empty messages
        if msg.text.trim().is_empty() {
            continue;
        }

        // Map sender to role
        let role = match msg.sender.as_str() {
            "human" => "user",
            "assistant" => "assistant",
            _ => &msg.sender, // Keep unknown roles as-is
        };

        messages.push(ContinuumMessage {
            id: msg_id,
            role: role.to_string(),
            content: msg.text.clone(),
            timestamp: msg.created_at.clone(),
        });
        msg_id += 1;
    }

    Ok(messages)
}
