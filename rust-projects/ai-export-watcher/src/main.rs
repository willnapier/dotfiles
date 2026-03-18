use anyhow::{Context, Result};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher, Event, EventKind};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::channel;
use std::time::Duration;

fn main() -> Result<()> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let downloads_dir = PathBuf::from(&home).join("Downloads");

    println!("AI/Clinical Export Watcher starting...");
    println!("Watching: {:?}", downloads_dir);
    println!("Patterns: ChatGPT-*.json, Grok-*.json, Gemini-*.json, *TM3*Diary*.html");

    let (tx, rx) = channel();

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        Config::default().with_poll_interval(Duration::from_secs(2)),
    )?;

    watcher.watch(&downloads_dir, RecursiveMode::NonRecursive)?;

    // Regex to match AI assistant export files (case-insensitive)
    let export_pattern = Regex::new(r"(?i)^(ChatGPT|Grok|Gemini)-.*\.json$")?;
    // TM3 diary HTML exports (SingleFile captures)
    let tm3_pattern = Regex::new(r"(?i)TM3.*Diary.*\.html$")?;

    println!("Watching for new exports...\n");

    for event in rx {
        if let EventKind::Create(_) | EventKind::Modify(_) = event.kind {
            for path in event.paths {
                if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                    if !path.exists() {
                        continue;
                    }

                    // Small delay to ensure file is fully written
                    std::thread::sleep(Duration::from_millis(500));

                    if export_pattern.is_match(filename) {
                        if let Err(e) = process_export(&path) {
                            eprintln!("Error processing {:?}: {}", path, e);
                        }
                    } else if tm3_pattern.is_match(filename) {
                        if let Err(e) = process_tm3(&path) {
                            eprintln!("Error processing {:?}: {}", path, e);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn process_export(path: &Path) -> Result<()> {
    println!("📥 Detected: {:?}", path.file_name().unwrap_or_default());

    // Run chatgpt-to-continuum (handles ChatGPT, Grok, Gemini)
    let output = Command::new("chatgpt-to-continuum")
        .arg(path)
        .output()
        .context("Failed to run chatgpt-to-continuum")?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        println!("✅ Converted successfully");
        for line in stdout.lines() {
            if line.contains("Created:") || line.contains("Messages:") || line.contains("Assistant:") {
                println!("   {}", line.trim());
            }
        }

        // Rename to indicate it's been processed
        let processed_name = path.with_extension("json.imported");
        if let Err(e) = std::fs::rename(path, &processed_name) {
            eprintln!("   Warning: couldn't rename file: {}", e);
        } else {
            println!("   Renamed to {:?}", processed_name.file_name().unwrap_or_default());
        }

    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Notify failure via messageboard
        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        let _ = Command::new("messageboard-edit")
            .args(["insert", &format!("AI import FAILED: {}", filename)])
            .output();

        anyhow::bail!("chatgpt-to-continuum failed: {}", stderr);
    }

    println!();
    Ok(())
}

fn process_tm3(path: &Path) -> Result<()> {
    println!("📋 TM3 diary detected: {:?}", path.file_name().unwrap_or_default());

    let output = Command::new("tm3-diary-capture")
        .arg("--latest")
        .output()
        .context("Failed to run tm3-diary-capture")?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        println!("✅ DayPage updated");
        for line in stdout.lines() {
            if line.starts_with("clinic::") || line.contains("unmapped") {
                println!("   {}", line.trim());
            }
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);

        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        let _ = Command::new("messageboard-edit")
            .args(["insert", &format!("TM3 import FAILED: {}", filename)])
            .output();

        anyhow::bail!("tm3-diary-capture failed: {}", stderr);
    }

    println!();
    Ok(())
}
