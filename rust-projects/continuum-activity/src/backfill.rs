use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct SessionMeta {
    id: String,
    assistant: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    skills: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Message {
    role: String,
    content: String,
}

struct BackfillResult {
    assistant: String,
    session_id: String,
    title: Option<String>,
    new_skills: Vec<String>,
    source: String,
}

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

/// Match a candidate string against known skills and aliases
fn match_skills_for_candidate(
    candidate: &str,
    known_skills: &[String],
    aliases: &HashMap<String, String>,
) -> Vec<String> {
    let mut skills = Vec::new();
    let candidate_lower = candidate.to_lowercase();

    // Direct skill name match
    for skill in known_skills {
        if candidate_lower == *skill || candidate_lower.contains(skill) {
            if !skills.contains(skill) {
                skills.push(skill.clone());
            }
        }
    }

    // Alias match (case-insensitive)
    for (alias, skill) in aliases {
        if candidate_lower.contains(&alias.to_lowercase()) {
            if !skills.contains(skill) {
                skills.push(skill.clone());
            }
        }
    }

    skills
}

/// Extract skill names from CC JSONL files by finding Skill tool_use blocks
fn extract_skills_from_jsonl(jsonl_path: &Path) -> Vec<String> {
    let file = match std::fs::File::open(jsonl_path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let reader = std::io::BufReader::new(file);
    let mut skills = Vec::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.is_empty() {
            continue;
        }

        let entry: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if entry_type != "assistant" {
            continue;
        }

        if let Some(content) = entry.pointer("/message/content").and_then(|v| v.as_array()) {
            for block in content {
                if block.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
                    continue;
                }
                let tool_name = block.get("name").and_then(|v| v.as_str()).unwrap_or("");
                if tool_name == "Skill" {
                    if let Some(skill) = block.pointer("/input/skill").and_then(|v| v.as_str()) {
                        if !skills.contains(&skill.to_string()) {
                            skills.push(skill.to_string());
                        }
                    }
                }
            }
        }
    }

    skills
}

/// Find the CC JSONL file matching a session ID in ~/.claude/projects/
fn find_cc_jsonl(session_id: &str) -> Option<PathBuf> {
    let projects_dir = dirs::home_dir()?.join(".claude/projects");
    if !projects_dir.exists() {
        return None;
    }

    for project_entry in std::fs::read_dir(&projects_dir).ok()?.flatten() {
        let project_dir = project_entry.path();
        if !project_dir.is_dir() {
            continue;
        }

        // Look for JSONL files matching the session ID
        let jsonl_name = format!("{}.jsonl", session_id);
        let jsonl_path = project_dir.join(&jsonl_name);
        if jsonl_path.exists() {
            return Some(jsonl_path);
        }
    }

    None
}

/// Scan first N user messages in messages.jsonl for /skill-name patterns
fn scan_messages_for_skills(
    session_dir: &Path,
    known_skills: &[String],
    aliases: &HashMap<String, String>,
) -> Vec<String> {
    let messages_path = session_dir.join("messages.jsonl");
    if !messages_path.exists() {
        return Vec::new();
    }

    let file = match std::fs::File::open(&messages_path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let reader = std::io::BufReader::new(file);
    let mut skills = Vec::new();
    let mut user_msg_count = 0;

    for line in reader.lines() {
        if user_msg_count >= 3 {
            break;
        }

        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }

        if let Ok(msg) = serde_json::from_str::<Message>(&line) {
            if msg.role != "user" {
                continue;
            }
            user_msg_count += 1;

            // Check for /skill-name patterns
            for skill in known_skills {
                let pattern = format!("/{}", skill);
                if msg.content.contains(&pattern) && !skills.contains(skill) {
                    skills.push(skill.clone());
                }
            }

            // Also check content against aliases
            for (alias, skill) in aliases {
                if msg.content.to_lowercase().contains(&alias.to_lowercase())
                    && !skills.contains(skill)
                {
                    // Only match aliases in user messages if they look intentional
                    // (e.g., "please be Geoff" or "/philosophy-tutor")
                    // We'll keep this broad for now since it's only first 3 messages
                    skills.push(skill.clone());
                }
            }
        }
    }

    skills
}

/// Collect all sessions from continuum-logs
fn collect_all_sessions(base_dir: &Path) -> Result<Vec<(PathBuf, SessionMeta)>> {
    let mut sessions = Vec::new();

    for assistant_entry in std::fs::read_dir(base_dir)?.flatten() {
        let assistant_dir = assistant_entry.path();
        if !assistant_dir.is_dir() {
            continue;
        }

        for date_entry in std::fs::read_dir(&assistant_dir)?.flatten() {
            let date_dir = date_entry.path();
            if !date_dir.is_dir() {
                continue;
            }

            for session_entry in std::fs::read_dir(&date_dir)?.flatten() {
                let session_dir = session_entry.path();
                let session_json = session_dir.join("session.json");
                if !session_json.exists() {
                    continue;
                }

                if let Ok(content) = std::fs::read_to_string(&session_json) {
                    if let Ok(meta) = serde_json::from_str::<SessionMeta>(&content) {
                        sessions.push((session_dir, meta));
                    }
                }
            }
        }
    }

    Ok(sessions)
}

/// Update session.json with skills field
fn update_session_skills(session_dir: &Path, skills: &[String]) -> Result<()> {
    let session_json_path = session_dir.join("session.json");
    if !session_json_path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(&session_json_path)?;
    let mut meta: serde_json::Value = serde_json::from_str(&content)?;

    if let Some(obj) = meta.as_object_mut() {
        obj.insert(
            "skills".to_string(),
            serde_json::json!(skills),
        );
    }

    let file = std::fs::File::create(&session_json_path)?;
    serde_json::to_writer_pretty(file, &meta)?;

    Ok(())
}

pub fn run(dry_run: bool) -> Result<()> {
    let base_dir = dirs::home_dir()
        .context("No home directory")?
        .join("Assistants/continuum-logs");

    if !base_dir.exists() {
        anyhow::bail!("Continuum logs directory not found: {}", base_dir.display());
    }

    let known_skills = read_skill_dirs();
    let aliases = read_aliases();

    eprintln!("Known skills: {}", known_skills.join(", "));
    eprintln!(
        "Aliases loaded: {} entries",
        aliases.len()
    );

    let sessions = collect_all_sessions(&base_dir)?;
    eprintln!("Scanning {} sessions...\n", sessions.len());

    let mut results: Vec<BackfillResult> = Vec::new();
    let mut already_have_skills: usize = 0;
    let mut no_match: usize = 0;

    for (session_dir, meta) in &sessions {
        // Skip sessions that already have skills
        if !meta.skills.is_empty() {
            already_have_skills += 1;
            continue;
        }

        let mut new_skills: Vec<String> = Vec::new();
        let mut source = String::new();

        // Strategy 1: For claude-code sessions, find JSONL and extract Skill tool_use
        if meta.assistant == "claude-code" {
            if let Some(jsonl_path) = find_cc_jsonl(&meta.id) {
                let jsonl_skills = extract_skills_from_jsonl(&jsonl_path);
                if !jsonl_skills.is_empty() {
                    source = "jsonl-tool-use".to_string();
                    for s in jsonl_skills {
                        if !new_skills.contains(&s) {
                            new_skills.push(s);
                        }
                    }
                }
            }
        }

        // Strategy 2: Match title against aliases + known skill names
        if new_skills.is_empty() {
            if let Some(title) = &meta.title {
                let title_skills =
                    match_skills_for_candidate(title, &known_skills, &aliases);
                if !title_skills.is_empty() {
                    source = "title-match".to_string();
                    for s in title_skills {
                        if !new_skills.contains(&s) {
                            new_skills.push(s);
                        }
                    }
                }
            }
        }

        // Strategy 3: Scan first 3 user messages for /skill-name or alias triggers
        if new_skills.is_empty() {
            let msg_skills = scan_messages_for_skills(session_dir, &known_skills, &aliases);
            if !msg_skills.is_empty() {
                source = "message-scan".to_string();
                for s in msg_skills {
                    if !new_skills.contains(&s) {
                        new_skills.push(s);
                    }
                }
            }
        }

        if new_skills.is_empty() {
            no_match += 1;
            continue;
        }

        // Update session.json (unless dry-run)
        if !dry_run {
            update_session_skills(session_dir, &new_skills)?;
        }

        results.push(BackfillResult {
            assistant: meta.assistant.clone(),
            session_id: meta.id.clone(),
            title: meta.title.clone(),
            new_skills,
            source,
        });
    }

    // Report
    let mode = if dry_run { "DRY RUN" } else { "BACKFILL" };
    eprintln!("=== {} ===\n", mode);

    if results.is_empty() {
        eprintln!("No sessions to update.");
        eprintln!(
            "  Already have skills: {}",
            already_have_skills
        );
        eprintln!("  No match found: {}", no_match);
        return Ok(());
    }

    eprintln!(
        "{} sessions {} skills:\n",
        results.len(),
        if dry_run { "would get" } else { "updated with" }
    );

    for r in &results {
        let title_display = r
            .title
            .as_deref()
            .map(|t| {
                if t.len() > 40 {
                    format!("{}...", &t[..40])
                } else {
                    t.to_string()
                }
            })
            .unwrap_or_else(|| "—".to_string());

        eprintln!(
            "  {:14} {:>14} [{:13}] \"{}\" → [{}]",
            r.assistant,
            r.session_id.chars().take(8).collect::<String>(),
            r.source,
            title_display,
            r.new_skills.join(", "),
        );
    }

    // Aggregate skill counts
    let mut skill_counts: HashMap<String, usize> = HashMap::new();
    for r in &results {
        for skill in &r.new_skills {
            *skill_counts.entry(skill.clone()).or_insert(0) += 1;
        }
    }

    let mut sorted_skills: Vec<_> = skill_counts.into_iter().collect();
    sorted_skills.sort_by(|a, b| b.1.cmp(&a.1));

    eprintln!("\nSkill summary:");
    for (skill, count) in &sorted_skills {
        eprintln!("  {:20} {} sessions", skill, count);
    }

    eprintln!(
        "\nSessions scanned: {} | Updated: {} | Already had skills: {} | No match: {}",
        sessions.len(),
        results.len(),
        already_have_skills,
        no_match,
    );

    if dry_run {
        eprintln!("\nThis was a dry run. Re-run without --dry-run to apply changes.");
    }

    Ok(())
}
