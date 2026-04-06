use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use clap::Parser;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser)]
#[command(about = "Publish Grok conversations to GitHub Pages for cross-session context")]
struct Cli {
    /// Date to publish (YYYY-MM-DD). Defaults to today.
    #[arg(short, long)]
    date: Option<String>,

    /// Publish all unpublished conversations, not just today's
    #[arg(long)]
    all: bool,

    /// Dry run — show what would be published without pushing
    #[arg(long)]
    dry_run: bool,

    /// Path to conversations repo (default: ~/Projects/conversations)
    #[arg(long)]
    repo: Option<PathBuf>,
}

#[derive(Deserialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct Session {
    id: String,
    #[serde(default)]
    message_count: usize,
}

fn grok_log_dir() -> PathBuf {
    dirs::home_dir()
        .expect("no home dir")
        .join("Assistants/continuum-logs/grok")
}

fn conversations_repo(cli: &Cli) -> PathBuf {
    cli.repo.clone().unwrap_or_else(|| {
        dirs::home_dir()
            .expect("no home dir")
            .join("Projects/conversations")
    })
}

/// Find all grok conversation directories for a given date
fn find_conversations(date_str: &str) -> Result<Vec<PathBuf>> {
    let day_dir = grok_log_dir().join(date_str);
    if !day_dir.exists() {
        return Ok(vec![]);
    }

    let mut convos: Vec<PathBuf> = fs::read_dir(&day_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("messages.jsonl").exists())
        .collect();

    convos.sort();
    Ok(convos)
}

/// Find all date directories in the grok log
fn find_all_dates() -> Result<Vec<String>> {
    let log_dir = grok_log_dir();
    if !log_dir.exists() {
        return Ok(vec![]);
    }

    let mut dates: Vec<String> = fs::read_dir(&log_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            // Validate it's a date
            NaiveDate::parse_from_str(&name, "%Y-%m-%d").ok()?;
            Some(name)
        })
        .collect();

    dates.sort();
    Ok(dates)
}

/// Check which conversation IDs are already published in the repo
fn published_ids(repo: &Path) -> Result<Vec<String>> {
    let tracker = repo.join(".published");
    if !tracker.exists() {
        return Ok(vec![]);
    }
    let content = fs::read_to_string(&tracker)?;
    Ok(content.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
}

fn mark_published(repo: &Path, ids: &[String]) -> Result<()> {
    let tracker = repo.join(".published");
    let mut existing = published_ids(repo)?;
    existing.extend(ids.iter().cloned());
    existing.sort();
    existing.dedup();
    fs::write(&tracker, existing.join("\n") + "\n")?;
    Ok(())
}

/// Read a conversation and return (session, messages)
fn read_conversation(dir: &Path) -> Result<(Session, Vec<Message>)> {
    let session_path = dir.join("session.json");
    let messages_path = dir.join("messages.jsonl");

    let session: Session = serde_json::from_str(
        &fs::read_to_string(&session_path).context("reading session.json")?,
    )
    .context("parsing session.json")?;

    let messages: Vec<Message> = fs::read_to_string(&messages_path)
        .context("reading messages.jsonl")?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    Ok((session, messages))
}

/// Convert messages to readable markdown
fn messages_to_markdown(messages: &[Message]) -> String {
    let mut md = String::new();

    for msg in messages {
        let content = msg.content.trim();
        if content.is_empty() {
            continue;
        }

        match msg.role.as_str() {
            "user" => {
                md.push_str(&format!("**William**: {}\n\n", content));
            }
            "assistant" => {
                md.push_str(&format!("**Grok**: {}\n\n", content));
            }
            _ => {
                md.push_str(&format!("*{}*: {}\n\n", msg.role, content));
            }
        }
    }

    md
}

/// Generate a slug from the first user message
fn generate_slug(messages: &[Message], date: &str) -> String {
    let first_user = messages
        .iter()
        .find(|m| m.role == "user" && !m.content.trim().is_empty())
        .map(|m| m.content.trim().to_string())
        .unwrap_or_else(|| "conversation".to_string());

    // Take first few words, lowercase, replace spaces with hyphens
    let words: Vec<&str> = first_user
        .split_whitespace()
        .take(6)
        .collect();

    let slug: String = words
        .join("-")
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect();

    // Truncate to reasonable length
    let slug = if slug.len() > 50 {
        slug[..50].trim_end_matches('-').to_string()
    } else {
        slug
    };

    format!("{}-{}", date, slug)
}

/// Build a unified document from multiple conversations on the same date
fn build_document(date: &str, conversations: &[(Session, Vec<Message>)]) -> String {
    let mut doc = String::new();

    doc.push_str(&format!(
        "# Grok Conversations — {}\n\n",
        date
    ));
    doc.push_str(&format!(
        "*{} conversation(s), unified from Tesla/Grok voice sessions.*\n\n---\n\n",
        conversations.len()
    ));

    for (i, (_session, messages)) in conversations.iter().enumerate() {
        if conversations.len() > 1 {
            doc.push_str(&format!("## Session {}\n\n", i + 1));
        }
        doc.push_str(&messages_to_markdown(messages));
        if i < conversations.len() - 1 {
            doc.push_str("---\n\n*[Context lost — new session started]*\n\n---\n\n");
        }
    }

    doc
}

/// Update README.md with new entry
fn update_readme(repo: &Path, slug: &str, date: &str, session_count: usize) -> Result<()> {
    let readme_path = repo.join("README.md");
    let content = fs::read_to_string(&readme_path).unwrap_or_default();

    // Find the index section and add new entry
    let entry = format!(
        "- [{}: Grok voice ({} session{})]({}.md)",
        date,
        session_count,
        if session_count == 1 { "" } else { "s" },
        slug
    );

    if content.contains(slug) {
        // Already listed
        return Ok(());
    }

    // Insert after "## Index" line
    let new_content = if content.contains("## Index") {
        content.replacen("## Index\n", &format!("## Index\n\n{}\n", entry), 1)
    } else {
        format!(
            "# Conversations\n\nRationalized transcripts of AI conversations.\n\n## Index\n\n{}\n",
            entry
        )
    };

    fs::write(&readme_path, new_content)?;
    Ok(())
}

/// Git add, commit, push
fn git_push(repo: &Path, slug: &str) -> Result<()> {
    let run = |args: &[&str]| -> Result<()> {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .context("running git")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("git {} failed: {}", args[0], stderr);
        }
        Ok(())
    };

    run(&["add", "-A"])?;
    run(&["commit", "-m", &format!("Add {}", slug)])?;
    run(&["push"])?;

    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let repo = conversations_repo(&cli);

    if !repo.exists() {
        bail!(
            "Conversations repo not found at {}. Clone it first:\n  git clone https://github.com/willnapier/conversations.git ~/Projects/conversations",
            repo.display()
        );
    }

    // Pull latest
    let _ = Command::new("git")
        .args(["pull", "--rebase"])
        .current_dir(&repo)
        .output();

    let already_published = published_ids(&repo)?;

    // Determine which dates to scan
    let dates = if cli.all {
        find_all_dates()?
    } else if let Some(ref d) = cli.date {
        vec![d.clone()]
    } else {
        // Today
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        vec![today]
    };

    let mut total_published = 0;

    for date in &dates {
        let convo_dirs = find_conversations(date)?;
        if convo_dirs.is_empty() {
            continue;
        }

        // Filter out already-published conversations
        let mut conversations = Vec::new();
        let mut new_ids = Vec::new();

        for dir in &convo_dirs {
            let (session, messages) = read_conversation(dir)?;
            if already_published.contains(&session.id) {
                continue;
            }
            // Skip very short conversations (< 4 messages)
            if messages.len() < 4 {
                continue;
            }
            new_ids.push(session.id.clone());
            conversations.push((session, messages));
        }

        if conversations.is_empty() {
            continue;
        }

        // Generate slug from first conversation's first message
        let slug = generate_slug(&conversations[0].1, date);
        let filename = format!("{}.md", slug);
        let filepath = repo.join(&filename);

        // If file already exists, append a counter
        let (slug, filepath) = if filepath.exists() {
            let mut n = 2;
            loop {
                let s = format!("{}-{}", slug, n);
                let p = repo.join(format!("{}.md", s));
                if !p.exists() {
                    break (s, p);
                }
                n += 1;
            }
        } else {
            (slug, filepath)
        };

        let url = format!(
            "https://willnapier.github.io/conversations/{}",
            slug
        );

        if cli.dry_run {
            println!("Would publish: {} ({} sessions)", filename, conversations.len());
            println!("URL: {}", url);
        } else {
            // Build the document
            let doc = build_document(date, &conversations);
            fs::write(&filepath, &doc)?;

            // Update README
            update_readme(&repo, &slug, date, conversations.len())?;

            // Track published IDs
            mark_published(&repo, &new_ids)?;

            println!("Published: {} ({} sessions)", filename, conversations.len());
            println!("URL: {}", url);
            println!("Speak: willnapier dot github dot I O slash conversations slash {}", slug);
        }

        total_published += conversations.len();
    }

    if total_published == 0 {
        println!("No unpublished Grok conversations found.");
        return Ok(());
    }

    // Git push
    if !cli.dry_run {
        git_push(&repo, "grok conversations")?;
        println!("\nPushed to GitHub. Pages will update in ~30 seconds.");
    }

    Ok(())
}
