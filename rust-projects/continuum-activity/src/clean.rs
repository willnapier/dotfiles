use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct SessionMeta {
    id: String,
    assistant: String,
    message_count: Option<u32>,
}

struct CleanResult {
    assistant: String,
    session_id: String,
    date: String,
    before_lines: usize,
    after_lines: usize,
    before_bytes: u64,
    after_bytes: u64,
}

fn hash_content(role: &str, content: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    role.hash(&mut hasher);
    content.hash(&mut hasher);
    hasher.finish()
}

pub fn clean_logs(dry_run: bool, no_backup: bool) -> Result<()> {
    let base_dir = dirs::home_dir()
        .context("No home directory")?
        .join("Assistants/continuum-logs");

    if !base_dir.exists() {
        anyhow::bail!("Continuum logs directory not found: {}", base_dir.display());
    }

    // Backup first (unless dry-run or explicitly skipped)
    if !dry_run && !no_backup {
        let backup_path = dirs::home_dir()
            .context("No home directory")?
            .join("Assistants/continuum-logs-backup-pre-clean");

        if backup_path.exists() {
            eprintln!("Backup already exists at {}", backup_path.display());
            eprintln!("Remove it first if you want a fresh backup, or use --no-backup");
            anyhow::bail!("Backup directory already exists");
        }

        eprintln!("Creating backup at {}...", backup_path.display());
        let status = std::process::Command::new("cp")
            .args(["-a", &base_dir.to_string_lossy(), &backup_path.to_string_lossy()])
            .status()
            .context("Failed to create backup")?;

        if !status.success() {
            anyhow::bail!("Backup failed");
        }
        eprintln!("Backup complete.\n");
    }

    // Collect all sessions
    let sessions = collect_all_sessions(&base_dir)?;
    eprintln!("Scanning {} sessions...\n", sessions.len());

    let mut results: Vec<CleanResult> = Vec::new();
    let mut total_before_bytes: u64 = 0;
    let mut total_after_bytes: u64 = 0;
    let mut total_before_lines: usize = 0;
    let mut total_after_lines: usize = 0;
    let mut sessions_modified: usize = 0;

    for (session_dir, meta) in &sessions {
        let messages_path = session_dir.join("messages.jsonl");
        if !messages_path.exists() {
            continue;
        }

        let before_bytes = std::fs::metadata(&messages_path)
            .map(|m| m.len())
            .unwrap_or(0);

        if before_bytes == 0 {
            continue;
        }

        // Read and deduplicate
        let (unique_lines, before_count, after_count) = deduplicate_messages(&messages_path)?;

        total_before_bytes += before_bytes;
        total_before_lines += before_count;

        if before_count == after_count {
            // No duplicates in this session
            total_after_bytes += before_bytes;
            total_after_lines += after_count;
            continue;
        }

        // Calculate what the new size would be
        let new_content: Vec<u8> = unique_lines.iter().flat_map(|l| {
            let mut bytes = l.as_bytes().to_vec();
            bytes.push(b'\n');
            bytes
        }).collect();
        let after_bytes = new_content.len() as u64;

        total_after_bytes += after_bytes;
        total_after_lines += after_count;
        sessions_modified += 1;

        let date = session_dir
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();

        results.push(CleanResult {
            assistant: meta.assistant.clone(),
            session_id: meta.id.clone(),
            date,
            before_lines: before_count,
            after_lines: after_count,
            before_bytes,
            after_bytes,
        });

        // Write deduped file (unless dry-run)
        if !dry_run {
            let mut file = std::fs::File::create(&messages_path)
                .with_context(|| format!("Failed to write {}", messages_path.display()))?;
            file.write_all(&new_content)?;

            // Update session.json with correct message count
            update_session_meta(session_dir, after_count)?;
        }
    }

    // Sort results by bytes saved (descending)
    results.sort_by(|a, b| {
        (b.before_bytes - b.after_bytes).cmp(&(a.before_bytes - a.after_bytes))
    });

    // Report
    let mode = if dry_run { "DRY RUN" } else { "CLEANED" };
    eprintln!("=== {} ===\n", mode);

    if results.is_empty() {
        eprintln!("No duplicates found. All sessions are clean.");
        return Ok(());
    }

    eprintln!("{} sessions with duplicates:\n", results.len());
    eprintln!(
        "  {:14} {:10} {:>10} {:>10} {:>10} {:>8}",
        "Assistant", "Date", "Before", "After", "Removed", "Saved"
    );
    eprintln!("  {}", "-".repeat(72));

    for r in &results {
        let saved_bytes = r.before_bytes - r.after_bytes;
        let saved_mb = saved_bytes as f64 / (1024.0 * 1024.0);
        let removed = r.before_lines - r.after_lines;

        eprintln!(
            "  {:14} {:10} {:>7} msg {:>7} msg {:>7} msg {:>6.1}MB",
            r.assistant, r.date, r.before_lines, r.after_lines, removed, saved_mb,
        );
    }

    let total_saved = total_before_bytes.saturating_sub(total_after_bytes);
    let total_saved_mb = total_saved as f64 / (1024.0 * 1024.0);
    let total_removed = total_before_lines.saturating_sub(total_after_lines);

    eprintln!("\n  {}", "=".repeat(72));
    eprintln!(
        "  {:14} {:10} {:>7} msg {:>7} msg {:>7} msg {:>6.1}MB",
        "TOTAL", "", total_before_lines, total_after_lines, total_removed, total_saved_mb,
    );

    eprintln!(
        "\nSessions scanned: {} | Modified: {} | Space recovered: {:.1}MB",
        sessions.len(),
        sessions_modified,
        total_saved_mb,
    );

    if dry_run {
        eprintln!("\nThis was a dry run. Re-run without --dry-run to apply changes.");
    }

    Ok(())
}

fn deduplicate_messages(messages_path: &Path) -> Result<(Vec<String>, usize, usize)> {
    let file = std::fs::File::open(messages_path)
        .with_context(|| format!("Failed to open {}", messages_path.display()))?;
    let reader = std::io::BufReader::new(file);

    let mut seen: HashSet<u64> = HashSet::new();
    let mut unique_lines: Vec<String> = Vec::new();
    let mut total_count: usize = 0;

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        total_count += 1;

        // Try to parse as a message for content-based dedup
        if let Ok(msg) = serde_json::from_str::<Message>(&line) {
            let hash = hash_content(&msg.role, &msg.content);
            if seen.insert(hash) {
                unique_lines.push(line);
            }
        } else {
            // Non-message lines (malformed JSON etc.) — keep them
            unique_lines.push(line);
        }
    }

    let unique_count = unique_lines.len();
    Ok((unique_lines, total_count, unique_count))
}

fn update_session_meta(session_dir: &Path, message_count: usize) -> Result<()> {
    let session_json_path = session_dir.join("session.json");
    if !session_json_path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(&session_json_path)?;
    let mut meta: serde_json::Value = serde_json::from_str(&content)?;

    if let Some(obj) = meta.as_object_mut() {
        obj.insert(
            "message_count".to_string(),
            serde_json::json!(message_count),
        );
    }

    let mut file = std::fs::File::create(&session_json_path)?;
    serde_json::to_writer_pretty(&mut file, &meta)?;

    Ok(())
}

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
