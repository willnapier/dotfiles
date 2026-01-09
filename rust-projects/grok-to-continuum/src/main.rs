use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "grok-to-continuum")]
#[command(about = "Convert Grok export to continuum format with interactive selection")]
struct Cli {
    /// Path to Grok prod-grok-backend.json file
    conversations_json: PathBuf,

    /// Output directory (default: ~/continuum-logs/grok)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Non-interactive mode - import all conversations
    #[arg(long)]
    all: bool,
}

// Grok export structures
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
    create_time: MongoDate,
}

#[derive(Debug, Deserialize)]
struct MongoDate {
    #[serde(rename = "$date")]
    date: MongoLong,
}

#[derive(Debug, Deserialize)]
struct MongoLong {
    #[serde(rename = "$numberLong")]
    number_long: String,
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
        PathBuf::from(home).join("continuum-logs").join("grok")
    });

    println!("Reading Grok export: {:?}", cli.conversations_json);
    println!("Output directory: {:?}\n", output_dir);

    // Read and parse conversations
    let json_content = fs::read_to_string(&cli.conversations_json)
        .context("Failed to read conversations.json")?;

    let export: GrokExport = serde_json::from_str(&json_content)
        .context("Failed to parse conversations.json")?;

    println!("Found {} conversations\n", export.conversations.len());

    if export.conversations.is_empty() {
        println!("No conversations to import");
        return Ok(());
    }

    // Interactive selection or import all
    let selected = if cli.all {
        println!("Importing all conversations...\n");
        (0..export.conversations.len()).collect()
    } else {
        select_conversations(&export.conversations)?
    };

    if selected.is_empty() {
        println!("\nNo conversations selected for import");
        return Ok(());
    }

    println!("\nImporting {} conversations...\n", selected.len());

    // Import selected conversations
    let mut success_count = 0;
    let mut error_count = 0;

    for idx in selected {
        let conv_wrapper = &export.conversations[idx];
        match import_conversation(conv_wrapper, &output_dir) {
            Ok(_) => {
                success_count += 1;
                println!("  ✓ Imported: {}", conv_wrapper.conversation.title);
            }
            Err(e) => {
                error_count += 1;
                eprintln!("  ✗ Error importing {}: {}", conv_wrapper.conversation.title, e);
            }
        }
    }

    println!("\nImport complete!");
    println!("  Success: {}", success_count);
    println!("  Errors:  {}", error_count);
    println!("  Output:  {:?}", output_dir);

    Ok(())
}

fn select_conversations(conversations: &[ConversationWrapper]) -> Result<Vec<usize>> {
    let mut selected = Vec::new();

    for (idx, conv_wrapper) in conversations.iter().enumerate() {
        let conv = &conv_wrapper.conversation;

        // Show conversation preview
        println!("═══════════════════════════════════════════════════════════════");
        println!("Conversation {}/{}", idx + 1, conversations.len());
        println!("───────────────────────────────────────────────────────────────");
        println!("Title: {}", conv.title);
        println!("Date:  {}", conv.create_time);

        // Show media types if present
        if !conv.media_types.is_empty() {
            println!("Media: {}", conv.media_types.join(", "));
        }

        println!("Messages: {}", conv_wrapper.responses.len());
        println!();

        // Show first 3 messages as preview
        let preview_count = conv_wrapper.responses.len().min(3);
        for (i, resp_wrapper) in conv_wrapper.responses.iter().take(preview_count).enumerate() {
            let resp = &resp_wrapper.response;
            let role = match resp.sender.as_str() {
                "human" => "USER",
                "assistant" => "ASSISTANT",
                _ => resp.sender.as_str(),
            };

            // Truncate long messages
            let content = if resp.message.len() > 150 {
                format!("{}...", &resp.message[..150])
            } else {
                resp.message.clone()
            };

            println!("  [{}] {}", role, content);
            if i < preview_count - 1 {
                println!();
            }
        }

        if conv_wrapper.responses.len() > preview_count {
            println!("  ... ({} more messages)", conv_wrapper.responses.len() - preview_count);
        }

        println!();

        // Prompt for action
        loop {
            print!("Import this conversation? [y/n/q]: ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;

            match input.trim().to_lowercase().as_str() {
                "y" | "yes" => {
                    selected.push(idx);
                    println!();
                    break;
                }
                "n" | "no" => {
                    println!("Skipped\n");
                    break;
                }
                "q" | "quit" => {
                    println!("\nQuitting selection...");
                    return Ok(selected);
                }
                _ => {
                    println!("Please enter y (yes), n (no), or q (quit)");
                }
            }
        }
    }

    Ok(selected)
}

fn import_conversation(conv_wrapper: &ConversationWrapper, output_dir: &PathBuf) -> Result<()> {
    let conv = &conv_wrapper.conversation;

    // Parse the created_at timestamp
    let datetime: DateTime<Utc> = conv.create_time.parse()
        .context("Invalid timestamp")?;
    let date_str = datetime.format("%Y-%m-%d").to_string();

    // Create session directory
    let session_dir = output_dir.join(&date_str).join(&conv.id);
    fs::create_dir_all(&session_dir)
        .with_context(|| format!("Failed to create {:?}", session_dir))?;

    // Convert messages
    let messages = convert_messages(&conv_wrapper.responses)?;

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

    // Find last message timestamp for end_time
    let end_time = messages.last()
        .map(|msg| msg.timestamp.clone());

    // Write session.json
    let session = ContinuumSession {
        id: conv.id.clone(),
        assistant: "grok".to_string(),
        start_time: Some(conv.create_time.clone()),
        end_time,
        status: Some("imported".to_string()),
        message_count: Some(messages.len() as u32),
        created_at: Some(conv.create_time.clone()),
    };

    let session_path = session_dir.join("session.json");
    let session_json = serde_json::to_string_pretty(&session)?;
    fs::write(&session_path, session_json)?;

    Ok(())
}

fn convert_messages(responses: &[ResponseWrapper]) -> Result<Vec<ContinuumMessage>> {
    let mut messages = Vec::new();
    let mut msg_id = 1u32;

    for resp_wrapper in responses {
        let resp = &resp_wrapper.response;

        // Skip empty messages
        if resp.message.trim().is_empty() {
            continue;
        }

        // Map sender to role
        let role = match resp.sender.as_str() {
            "human" => "user",
            "assistant" => "assistant",
            _ => &resp.sender,
        };

        // Parse MongoDB timestamp to ISO 8601
        let millis: i64 = resp.create_time.date.number_long.parse()
            .context("Failed to parse timestamp")?;
        let datetime = DateTime::from_timestamp_millis(millis)
            .context("Invalid timestamp milliseconds")?;
        let timestamp = datetime.to_rfc3339();

        messages.push(ContinuumMessage {
            id: msg_id,
            role: role.to_string(),
            content: resp.message.clone(),
            timestamp,
        });
        msg_id += 1;
    }

    Ok(messages)
}
