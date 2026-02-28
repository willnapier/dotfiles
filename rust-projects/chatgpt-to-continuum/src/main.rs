use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

// ============================================================================
// Skill matching
// ============================================================================

/// Read skill alias mappings from ~/.config/continuum/skill-aliases.json
fn read_aliases() -> HashMap<String, String> {
    let path = dirs::home_dir()
        .map(|h| h.join(".config/continuum/skill-aliases.json"))
        .unwrap_or_default();

    if !path.exists() {
        return HashMap::new();
    }

    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

/// Read known skill names from ~/.claude/skills/ directory
fn read_skill_dirs() -> Vec<String> {
    let path = dirs::home_dir()
        .map(|h| h.join(".claude/skills"))
        .unwrap_or_default();

    if !path.exists() {
        return Vec::new();
    }

    std::fs::read_dir(&path)
        .ok()
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| {
                    let name = e.file_name().to_str()?.to_string();
                    if name.starts_with('.') { None } else { Some(name) }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Match title and/or project against known skills and aliases
fn match_skills(title: Option<&str>, project: Option<&str>) -> Vec<String> {
    let known_skills = read_skill_dirs();
    let aliases = read_aliases();
    let mut skills = Vec::new();

    for candidate in [project, title].into_iter().flatten() {
        let candidate_lower = candidate.to_lowercase();

        // Direct skill name match
        for skill in &known_skills {
            if candidate_lower.contains(skill) && !skills.contains(skill) {
                skills.push(skill.clone());
            }
        }

        // Alias match (case-insensitive)
        for (alias, skill) in &aliases {
            if candidate_lower.contains(&alias.to_lowercase()) && !skills.contains(skill) {
                skills.push(skill.clone());
            }
        }
    }

    skills
}

#[derive(Parser)]
#[command(name = "chatgpt-to-continuum")]
#[command(about = "Convert ChatGPT/Grok export to continuum format")]
struct Cli {
    /// Path to JSON file (ChatGPT or Grok export)
    input: PathBuf,

    /// Output directory (default: auto-detected based on source)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Force assistant type (chatgpt, grok) - auto-detected if not specified
    #[arg(short, long)]
    assistant: Option<String>,
}

// ============================================================================
// Browser Exporter format (ChatGPT Exporter / Grok Exporter)
// ============================================================================

#[derive(Debug, Deserialize)]
struct ExporterConversation {
    metadata: ExporterMetadata,
    messages: Vec<ExporterMessage>,
    /// Grok has title at root level
    #[serde(default)]
    title: Option<String>,
    /// Project/folder name from browser extension
    #[serde(default)]
    project: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExporterMetadata {
    /// ChatGPT has title in metadata
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    user: Option<ExporterUser>,
    dates: ExporterDates,
    #[serde(default)]
    link: Option<String>,
    #[serde(default)]
    powered_by: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExporterUser {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExporterDates {
    created: String,
    updated: String,
    #[serde(default)]
    exported: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExporterMessage {
    role: String,
    say: String,
}

// ============================================================================
// Browser Extension v2.4+ format (Grok Exporter, etc.)
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserExtensionExport {
    export_date: String,
    #[serde(default)]
    export_version: Option<String>,
    platform: String,
    #[serde(default)]
    message_count: Option<u32>,
    #[serde(default)]
    url: Option<String>,
    conversation: Vec<BrowserExtensionMessage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserExtensionMessage {
    #[serde(default)]
    id: Option<String>,
    speaker: String,
    content: String,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    debug_info: Option<BrowserExtensionDebugInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserExtensionDebugInfo {
    scores: Option<BrowserExtensionScores>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserExtensionScores {
    grok_score: Option<i32>,
    human_score: Option<i32>,
}

// ============================================================================
// Official OpenAI export format
// ============================================================================

#[derive(Debug, Deserialize)]
struct OfficialConversation {
    title: String,
    create_time: f64,
    update_time: Option<f64>,
    mapping: HashMap<String, Node>,
    #[serde(default)]
    current_node: Option<String>,
    id: String,
}

#[derive(Debug, Deserialize)]
struct Node {
    #[serde(default)]
    id: String,
    message: Option<NodeMessage>,
    parent: Option<String>,
    #[serde(default)]
    children: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct NodeMessage {
    #[serde(default)]
    id: String,
    author: Author,
    create_time: Option<f64>,
    content: Content,
}

#[derive(Debug, Deserialize)]
struct Author {
    role: String,
}

#[derive(Debug, Deserialize)]
struct Content {
    parts: Option<Vec<serde_json::Value>>,
}

// ============================================================================
// Continuum output structures
// ============================================================================

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
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_url: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    skills: Vec<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    println!("Reading: {:?}", cli.input);

    let json_content = fs::read_to_string(&cli.input)
        .context("Failed to read input file")?;

    // Try to detect format and process accordingly
    if let Ok(exporter_conv) = serde_json::from_str::<ExporterConversation>(&json_content) {
        // Detect source from powered_by or CLI flag
        let assistant = cli.assistant.clone().unwrap_or_else(|| {
            detect_assistant(&exporter_conv)
        });

        let output_dir = cli.output.unwrap_or_else(|| {
            let home = std::env::var("HOME").expect("HOME not set");
            PathBuf::from(home)
                .join("Assistants")
                .join("continuum-logs")
                .join(&assistant)
        });

        println!("Detected: {} Exporter format", assistant);
        println!("Output:  {:?}", output_dir);

        process_exporter_conversation(&exporter_conv, &output_dir, &assistant)?;
        println!("\nImport complete!");
        println!("  Assistant:     {}", assistant);
        println!("  Conversations: 1");
        println!("  Messages:      {}", exporter_conv.messages.len());
        println!("  Output:        {:?}", output_dir);
    } else if let Ok(browser_ext) = serde_json::from_str::<BrowserExtensionExport>(&json_content) {
        // Browser extension v2.4+ format (Grok Exporter, etc.)
        let assistant = cli.assistant.clone().unwrap_or_else(|| browser_ext.platform.to_lowercase());

        let output_dir = cli.output.unwrap_or_else(|| {
            let home = std::env::var("HOME").expect("HOME not set");
            PathBuf::from(home)
                .join("Assistants")
                .join("continuum-logs")
                .join(&assistant)
        });

        let version = browser_ext.export_version.as_deref().unwrap_or("unknown");
        println!("Detected: Browser Extension v{} format ({})", version, browser_ext.platform);
        println!("Output:  {:?}", output_dir);

        process_browser_extension_export(&browser_ext, &output_dir, &assistant)?;
        println!("\nImport complete!");
        println!("  Assistant:     {}", assistant);
        println!("  Conversations: 1");
        println!("  Messages:      {}", browser_ext.conversation.len());
        println!("  Output:        {:?}", output_dir);
    } else if let Ok(official_convs) = serde_json::from_str::<Vec<OfficialConversation>>(&json_content) {
        let output_dir = cli.output.unwrap_or_else(|| {
            let home = std::env::var("HOME").expect("HOME not set");
            PathBuf::from(home)
                .join("Assistants")
                .join("continuum-logs")
                .join("chatgpt")
        });

        println!("Detected: Official OpenAI export format");
        println!("Output:  {:?}", output_dir);
        println!("Found {} conversations", official_convs.len());

        let mut success_count = 0;
        let mut error_count = 0;

        for (idx, conversation) in official_convs.iter().enumerate() {
            match process_official_conversation(conversation, &output_dir) {
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
    } else {
        anyhow::bail!("Unrecognized JSON format. Expected ChatGPT/Grok Exporter or official OpenAI export.");
    }

    Ok(())
}

fn detect_assistant(conv: &ExporterConversation) -> String {
    if let Some(powered_by) = &conv.metadata.powered_by {
        let lower = powered_by.to_lowercase();
        if lower.contains("grok") {
            return "grok".to_string();
        }
        if lower.contains("chatgpt") {
            return "chatgpt".to_string();
        }
        if lower.contains("gemini") {
            return "gemini".to_string();
        }
    }
    // Default to chatgpt
    "chatgpt".to_string()
}

// ============================================================================
// Process Browser Exporter format (ChatGPT / Grok)
// ============================================================================

fn parse_exporter_date(date_str: &str) -> Option<DateTime<Utc>> {
    // Try format with seconds: "11/24/2025 11:32:17"
    if let Ok(dt) = NaiveDateTime::parse_from_str(date_str, "%m/%d/%Y %H:%M:%S") {
        return Some(dt.and_utc());
    }
    // Try format without seconds: "9/11/2025 15:14"
    if let Ok(dt) = NaiveDateTime::parse_from_str(date_str, "%m/%d/%Y %H:%M") {
        return Some(dt.and_utc());
    }
    None
}

fn get_title(conv: &ExporterConversation) -> String {
    // Try root-level title first (Grok), then metadata title (ChatGPT)
    conv.title.clone()
        .or_else(|| conv.metadata.title.clone())
        .unwrap_or_else(|| "untitled".to_string())
}

fn process_exporter_conversation(conv: &ExporterConversation, output_dir: &PathBuf, assistant: &str) -> Result<()> {
    let created = parse_exporter_date(&conv.metadata.dates.created)
        .unwrap_or_else(Utc::now);
    let updated = parse_exporter_date(&conv.metadata.dates.updated);

    let date_str = created.format("%Y-%m-%d").to_string();
    let title = get_title(conv);

    // Generate ID from title (sanitized)
    let id = sanitize_id(&title);

    let session_dir = output_dir.join(&date_str).join(&id);
    fs::create_dir_all(&session_dir)
        .with_context(|| format!("Failed to create {:?}", session_dir))?;

    // Convert messages
    let mut continuum_messages = Vec::new();
    for (idx, msg) in conv.messages.iter().enumerate() {
        // Map roles: "Prompt" -> "user", "Response" -> "assistant"
        let role = match msg.role.as_str() {
            "Prompt" => "user".to_string(),
            "Response" => "assistant".to_string(),
            other => other.to_lowercase(),
        };

        // Clean up the content (remove trailing timestamps like "11:32 AM11:32")
        let content = clean_message_content(&msg.say);

        if !content.trim().is_empty() {
            continuum_messages.push(ContinuumMessage {
                id: (idx + 1) as u32,
                role,
                content,
                timestamp: created.to_rfc3339(),
            });
        }
    }

    if continuum_messages.is_empty() {
        return Ok(());
    }

    // Write messages.jsonl
    let messages_path = session_dir.join("messages.jsonl");
    let mut jsonl_content = String::new();
    for msg in &continuum_messages {
        jsonl_content.push_str(&serde_json::to_string(msg)?);
        jsonl_content.push('\n');
    }
    fs::write(&messages_path, jsonl_content)?;

    // Match skills from title and project
    let skills = match_skills(
        Some(&title),
        conv.project.as_deref(),
    );

    // Write session.json
    let session = ContinuumSession {
        id: id.clone(),
        assistant: assistant.to_string(),
        start_time: Some(created.to_rfc3339()),
        end_time: updated.map(|dt| dt.to_rfc3339()),
        status: Some("imported".to_string()),
        message_count: Some(continuum_messages.len() as u32),
        created_at: Some(created.to_rfc3339()),
        title: Some(title),
        source_url: conv.metadata.link.clone(),
        skills,
    };

    let session_path = session_dir.join("session.json");
    let session_json = serde_json::to_string_pretty(&session)?;
    fs::write(&session_path, session_json)?;

    println!("  Created: {}/{}", date_str, id);
    Ok(())
}

fn sanitize_id(title: &str) -> String {
    title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else if c.is_whitespace() {
                '-'
            } else {
                '_'
            }
        })
        .collect::<String>()
        .chars()
        .take(64)
        .collect()
}

fn clean_message_content(content: &str) -> String {
    let mut result = content.to_string();

    // Remove trailing timestamp patterns like "11:32 AM11:32" or "11:54 AM11:54"
    let timestamp_re = regex::Regex::new(r"\n\n\d{1,2}:\d{2}\s*[AP]M\d{1,2}:\d{2}\s*$").unwrap();
    result = timestamp_re.replace(&result, "").to_string();

    // Remove Gemini UI artifacts
    // "Edit" on its own line
    let edit_re = regex::Regex::new(r"(?m)^Edit\s*$").unwrap();
    result = edit_re.replace_all(&result, "").to_string();

    // "Retry" and "WN" markers
    let retry_re = regex::Regex::new(r"(?m)^Retry\s*$").unwrap();
    result = retry_re.replace_all(&result, "").to_string();
    let wn_re = regex::Regex::new(r"(?m)^WN\s*$").unwrap();
    result = wn_re.replace_all(&result, "").to_string();

    // Timestamp indicators like "9s", "0s", "4s", "25s" on their own line
    let time_indicator_re = regex::Regex::new(r"(?m)^\d+s\s*$").unwrap();
    result = time_indicator_re.replace_all(&result, "").to_string();

    // "X results" search indicators
    let results_re = regex::Regex::new(r"(?m)^\d+\s+results?\s*$").unwrap();
    result = results_re.replace_all(&result, "").to_string();

    // Collapse multiple newlines into at most two
    let multi_newline_re = regex::Regex::new(r"\n{3,}").unwrap();
    result = multi_newline_re.replace_all(&result, "\n\n").to_string();

    result.trim().to_string()
}

// ============================================================================
// Process Browser Extension v2.4+ format
// ============================================================================

fn process_browser_extension_export(export: &BrowserExtensionExport, output_dir: &PathBuf, assistant: &str) -> Result<()> {
    // Parse export date
    let created = DateTime::parse_from_rfc3339(&export.export_date)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());

    let date_str = created.format("%Y-%m-%d").to_string();

    // Generate ID from URL or timestamp
    let id = if let Some(url) = &export.url {
        // Extract conversation ID from URL like "https://grok.com/c/b36eb0c0-..."
        url.split('/').last()
            .and_then(|s| s.split('?').next())
            .unwrap_or("unknown")
            .to_string()
    } else {
        format!("{}-{}", assistant, created.timestamp())
    };

    let session_dir = output_dir.join(&date_str).join(&id);
    fs::create_dir_all(&session_dir)
        .with_context(|| format!("Failed to create {:?}", session_dir))?;

    // Convert messages using alternating pattern (user starts, then alternates)
    // The browser extension's debug scores are unreliable, so we use conversation flow
    let mut continuum_messages = Vec::new();
    for (idx, msg) in export.conversation.iter().enumerate() {
        // Conversations typically start with user, then alternate
        // This is more reliable than the extension's scoring heuristics
        let role = if idx % 2 == 0 {
            "user".to_string()
        } else {
            "assistant".to_string()
        };

        let timestamp = msg.timestamp.clone()
            .unwrap_or_else(|| created.to_rfc3339());

        if !msg.content.trim().is_empty() {
            continuum_messages.push(ContinuumMessage {
                id: (idx + 1) as u32,
                role,
                content: msg.content.clone(),
                timestamp,
            });
        }
    }

    if continuum_messages.is_empty() {
        return Ok(());
    }

    // Write messages.jsonl
    let messages_path = session_dir.join("messages.jsonl");
    let mut jsonl_content = String::new();
    for msg in &continuum_messages {
        jsonl_content.push_str(&serde_json::to_string(msg)?);
        jsonl_content.push('\n');
    }
    fs::write(&messages_path, jsonl_content)?;

    // Get end time from last message
    let end_time = continuum_messages.last()
        .map(|msg| msg.timestamp.clone());

    // Match skills (browser extension format has no title, but future versions may)
    let skills = match_skills(None, None);

    // Write session.json
    let session = ContinuumSession {
        id: id.clone(),
        assistant: assistant.to_string(),
        start_time: Some(created.to_rfc3339()),
        end_time,
        status: Some("imported".to_string()),
        message_count: Some(continuum_messages.len() as u32),
        created_at: Some(created.to_rfc3339()),
        title: None, // Browser extension format doesn't include title
        source_url: export.url.clone(),
        skills,
    };

    let session_path = session_dir.join("session.json");
    let session_json = serde_json::to_string_pretty(&session)?;
    fs::write(&session_path, session_json)?;

    println!("  Created: {}/{}", date_str, id);
    Ok(())
}

fn map_speaker_to_role(speaker: &str) -> String {
    match speaker.to_lowercase().as_str() {
        "grok" | "assistant" | "gemini" | "chatgpt" => "assistant".to_string(),
        "human" | "user" | "you" => "user".to_string(),
        _ => "user".to_string(), // Default to user for unknown speakers
    }
}

// ============================================================================
// Process Official OpenAI export format
// ============================================================================

fn process_official_conversation(conv: &OfficialConversation, output_dir: &PathBuf) -> Result<()> {
    let datetime = DateTime::<Utc>::from_timestamp(conv.create_time as i64, 0)
        .context("Invalid timestamp")?;
    let date_str = datetime.format("%Y-%m-%d").to_string();

    let session_dir = output_dir.join(&date_str).join(&conv.id);
    fs::create_dir_all(&session_dir)
        .with_context(|| format!("Failed to create {:?}", session_dir))?;

    let messages = extract_messages_from_tree(conv)?;

    if messages.is_empty() {
        return Ok(());
    }

    // Write messages.jsonl
    let messages_path = session_dir.join("messages.jsonl");
    let mut jsonl_content = String::new();
    for msg in &messages {
        jsonl_content.push_str(&serde_json::to_string(msg)?);
        jsonl_content.push('\n');
    }
    fs::write(&messages_path, jsonl_content)?;

    // Match skills from title
    let skills = match_skills(Some(&conv.title), None);

    // Write session.json
    let session = ContinuumSession {
        id: conv.id.clone(),
        assistant: "chatgpt".to_string(),
        start_time: Some(datetime.to_rfc3339()),
        end_time: conv.update_time.and_then(|t| {
            DateTime::<Utc>::from_timestamp(t as i64, 0)
                .map(|dt| dt.to_rfc3339())
        }),
        status: Some("imported".to_string()),
        message_count: Some(messages.len() as u32),
        created_at: Some(datetime.to_rfc3339()),
        title: Some(conv.title.clone()),
        source_url: None,
        skills,
    };

    let session_path = session_dir.join("session.json");
    let session_json = serde_json::to_string_pretty(&session)?;
    fs::write(&session_path, session_json)?;

    Ok(())
}

fn extract_text_from_part(part: &serde_json::Value) -> Option<String> {
    if let Some(text) = part.as_str() {
        return Some(text.to_string());
    }

    if let Some(obj) = part.as_object() {
        if let Some(content_type) = obj.get("content_type").and_then(|v| v.as_str()) {
            return Some(format!("[{}]", content_type));
        }
    }

    None
}

fn extract_messages_from_tree(conv: &OfficialConversation) -> Result<Vec<ContinuumMessage>> {
    let mut messages = Vec::new();

    let root_id = conv.mapping.iter()
        .find(|(_, node)| node.parent.is_none())
        .map(|(id, _)| id.clone())
        .context("No root node found")?;

    let mut to_visit = vec![root_id];
    let mut msg_id = 1u32;

    while let Some(node_id) = to_visit.pop() {
        if let Some(node) = conv.mapping.get(&node_id) {
            if let Some(msg) = &node.message {
                if let Some(parts) = &msg.content.parts {
                    let text_parts: Vec<String> = parts.iter()
                        .filter_map(extract_text_from_part)
                        .collect();

                    if !text_parts.is_empty() {
                        let content = text_parts.join("\n");
                        if !content.trim().is_empty() {
                            let timestamp = msg.create_time
                                .and_then(|t| DateTime::<Utc>::from_timestamp(t as i64, 0))
                                .map(|dt| dt.to_rfc3339())
                                .unwrap_or_else(|| Utc::now().to_rfc3339());

                            messages.push(ContinuumMessage {
                                id: msg_id,
                                role: msg.author.role.clone(),
                                content,
                                timestamp,
                            });
                            msg_id += 1;
                        }
                    }
                }
            }

            for child_id in node.children.iter().rev() {
                to_visit.push(child_id.clone());
            }
        }
    }

    Ok(messages)
}
