use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(name = "restore-evernote-dates")]
#[command(about = "Restore file creation dates from Evernote export")]
struct Args {
    /// Path to Evernote .enex export file
    #[arg(value_name = "ENEX_FILE")]
    enex_file: PathBuf,

    /// Directory containing files to update (e.g., ~/Forge)
    #[arg(value_name = "TARGET_DIR")]
    target_dir: PathBuf,

    /// Show what would be changed without making changes
    #[arg(long)]
    dry_run: bool,

    /// Show detailed progress
    #[arg(long)]
    verbose: bool,
}

#[derive(Debug)]
struct EvernoteNote {
    title: String,
    created: String,
}

#[derive(Debug)]
struct MatchResult {
    status: MatchStatus,
    title: String,
}

#[derive(Debug, PartialEq)]
enum MatchStatus {
    Updated,
    WouldUpdate,
    NoMatch,
    Error(String),
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!("Reading Evernote export: {}", args.enex_file.display());
    println!("Target directory: {}\n", args.target_dir.display());

    // Parse Evernote export
    println!("Parsing Evernote notes...");
    let notes = parse_evernote_export(&args.enex_file)?;
    println!("Found {} notes in Evernote export\n", notes.len());

    // Scan target directory for markdown files
    println!("Scanning target directory for markdown files...");
    let target_files = find_markdown_files(&args.target_dir)?;
    println!("Found {} markdown files\n", target_files.len());

    // Build file index (HashMap for O(1) lookups)
    println!("Building file index...");
    let file_map = build_file_map(&target_files);
    println!("Indexed {} unique filenames\n", file_map.len());

    // Match notes to files
    println!("Matching Evernote notes to files...");
    let results = match_and_process_notes(
        &notes,
        &file_map,
        args.dry_run,
        args.verbose,
    )?;

    // Print summary
    print_summary(&results, notes.len(), target_files.len(), args.dry_run);

    Ok(())
}

fn parse_evernote_export(path: &Path) -> Result<Vec<EvernoteNote>> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {}", path.display()))?;

    let mut reader = Reader::from_str(&content);
    reader.trim_text(true);

    let mut notes = Vec::new();
    let mut current_title = None;
    let mut current_created = None;
    let mut inside_title = false;
    let mut inside_created = false;

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                match e.name().as_ref() {
                    b"title" => inside_title = true,
                    b"created" => inside_created = true,
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape().unwrap().to_string();
                if inside_title {
                    current_title = Some(text);
                    inside_title = false;
                } else if inside_created {
                    current_created = Some(text);
                    inside_created = false;
                }
            }
            Ok(Event::End(ref e)) => {
                if e.name().as_ref() == b"note" {
                    if let (Some(title), Some(created)) = (current_title.take(), current_created.take()) {
                        notes.push(EvernoteNote { title, created });
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow::anyhow!("Error parsing XML at position {}: {:?}", reader.buffer_position(), e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(notes)
}

fn find_markdown_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in WalkDir::new(dir).follow_links(false).into_iter() {
        let entry = entry?;
        if entry.file_type().is_file() {
            if let Some(ext) = entry.path().extension() {
                if ext == "md" {
                    files.push(entry.path().to_path_buf());
                }
            }
        }
    }
    Ok(files)
}

fn build_file_map(files: &[PathBuf]) -> HashMap<String, Vec<PathBuf>> {
    let mut map: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for file in files {
        if let Some(stem) = file.file_stem() {
            if let Some(name) = stem.to_str() {
                // Store all files with the same name (handles duplicates)
                map.entry(name.to_string())
                    .or_insert_with(Vec::new)
                    .push(file.clone());
            }
        }
    }
    map
}

fn match_and_process_notes(
    notes: &[EvernoteNote],
    file_map: &HashMap<String, Vec<PathBuf>>,
    dry_run: bool,
    verbose: bool,
) -> Result<Vec<MatchResult>> {
    let progress = if !verbose {
        let pb = ProgressBar::new(notes.len() as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("##-"),
        );
        Some(pb)
    } else {
        None
    };

    let mut results = Vec::new();

    for (idx, note) in notes.iter().enumerate() {
        let result = process_note(note, file_map, dry_run, verbose, idx + 1, notes.len())?;
        results.push(result);

        if let Some(ref pb) = progress {
            pb.inc(1);
        }
    }

    if let Some(pb) = progress {
        pb.finish_with_message("Complete!");
    }

    Ok(results)
}

fn process_note(
    note: &EvernoteNote,
    file_map: &HashMap<String, PathBuf>,
    dry_run: bool,
    verbose: bool,
    idx: usize,
    total: usize,
) -> Result<MatchResult> {
    // Try to find matching file
    let file_path = match file_map.get(&note.title) {
        Some(path) => path,
        None => {
            if verbose {
                println!("⊘ [{}/{}] No match: {}", idx, total, note.title);
            }
            return Ok(MatchResult {
                status: MatchStatus::NoMatch,
                title: note.title.clone(),
            });
        }
    };

    // Parse the Evernote timestamp (format: 20151001T080944Z)
    let timestamp = match parse_evernote_timestamp(&note.created) {
        Ok(ts) => ts,
        Err(e) => {
            if verbose {
                println!("⚠ [{}/{}] Failed to parse date: {} - {}", idx, total, note.title, e);
            }
            return Ok(MatchResult {
                status: MatchStatus::Error(format!("Failed to parse date: {}", e)),
                title: note.title.clone(),
            });
        }
    };

    if dry_run {
        if verbose {
            println!("🔍 [{}/{}] Would update: {}", idx, total, note.title);
            println!("   File: {}", file_path.display());
            println!("   Date: {}", note.created);
        }
        Ok(MatchResult {
            status: MatchStatus::WouldUpdate,
            title: note.title.clone(),
        })
    } else {
        // Update YAML frontmatter first
        match update_yaml_frontmatter(file_path, timestamp) {
            Ok(_) => {
                // Then update file timestamp
                match set_file_mtime(file_path, timestamp) {
                    Ok(_) => {
                        if verbose {
                            println!("✓ [{}/{}] Updated: {}", idx, total, note.title);
                            println!("   Date: {}", note.created);
                        } else if idx % 100 == 0 {
                            eprintln!("Progress: {}/{} files processed...", idx, total);
                        }
                        Ok(MatchResult {
                            status: MatchStatus::Updated,
                            title: note.title.clone(),
                        })
                    }
                    Err(e) => {
                        eprintln!("⚠ [{}/{}] Failed to update mtime: {} - {}", idx, total, note.title, e);
                        Ok(MatchResult {
                            status: MatchStatus::Error(format!("Failed to update mtime: {}", e)),
                            title: note.title.clone(),
                        })
                    }
                }
            }
            Err(e) => {
                eprintln!("⚠ [{}/{}] Failed to update YAML: {} - {}", idx, total, note.title, e);
                Ok(MatchResult {
                    status: MatchStatus::Error(format!("Failed to update YAML: {}", e)),
                    title: note.title.clone(),
                })
            }
        }
    }
}

fn parse_evernote_timestamp(timestamp: &str) -> Result<i64> {
    // Format: 20151001T080944Z -> Unix timestamp
    // Extract: YYYYMMDD HHMMSS
    if timestamp.len() < 15 {
        return Err(anyhow::anyhow!("Timestamp too short: {}", timestamp));
    }

    let year: i32 = timestamp[0..4].parse()?;
    let month: u32 = timestamp[4..6].parse()?;
    let day: u32 = timestamp[6..8].parse()?;
    let hour: u32 = timestamp[9..11].parse()?;
    let minute: u32 = timestamp[11..13].parse()?;
    let second: u32 = timestamp[13..15].parse()?;

    let naive = NaiveDateTime::parse_from_str(
        &format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", year, month, day, hour, minute, second),
        "%Y-%m-%d %H:%M:%S"
    )?;

    let datetime: DateTime<Utc> = DateTime::from_naive_utc_and_offset(naive, Utc);
    Ok(datetime.timestamp())
}

fn set_file_mtime(path: &Path, timestamp: i64) -> Result<()> {
    use std::time::UNIX_EPOCH;

    let time = UNIX_EPOCH + std::time::Duration::from_secs(timestamp as u64);
    filetime::set_file_mtime(path, filetime::FileTime::from_system_time(time))?;
    Ok(())
}

fn update_yaml_frontmatter(path: &Path, timestamp: i64) -> Result<()> {
    // Convert timestamp to YAML date format: "YYYY-MM-DD HH:MM"
    let datetime: DateTime<Utc> = DateTime::from_timestamp(timestamp, 0)
        .ok_or_else(|| anyhow::anyhow!("Invalid timestamp"))?;
    let date_str = datetime.format("%Y-%m-%d %H:%M").to_string();

    // Read file content
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {}", path.display()))?;

    // Check if file has YAML frontmatter
    if !content.starts_with("---\n") {
        // No frontmatter - add it at the beginning
        let new_content = format!(
            "---\ndate created: {}\ndate modified: {}\n---\n{}",
            date_str, date_str, content
        );
        fs::write(path, new_content)?;
        return Ok(());
    }

    // Find end of frontmatter
    let end_marker = content[4..].find("\n---\n");
    if end_marker.is_none() {
        return Err(anyhow::anyhow!("Malformed YAML frontmatter"));
    }

    let end_pos = end_marker.unwrap() + 4;
    let frontmatter = &content[4..end_pos];
    let rest = &content[end_pos + 5..]; // Skip "\n---\n"

    // Update or add date fields
    let mut new_frontmatter = frontmatter.to_string();

    // Update date created (only if it doesn't exist or is newer than Evernote date)
    if let Some(existing_date) = extract_date_field(&new_frontmatter, "date created") {
        // Only update if existing date is clearly wrong (e.g., 2025 when Evernote says 2015)
        // We'll update any existing date with the Evernote date since that's authoritative
        new_frontmatter = replace_date_field(&new_frontmatter, "date created", &date_str);
    } else {
        // No date created field - add it at the beginning
        new_frontmatter = format!("date created: {}\n{}", date_str, new_frontmatter);
    }

    // Update date modified (always use Evernote date as it's the last known modification)
    if new_frontmatter.contains("date modified:") {
        new_frontmatter = replace_date_field(&new_frontmatter, "date modified", &date_str);
    } else {
        // Add after date created
        if new_frontmatter.starts_with("date created:") {
            let first_newline = new_frontmatter.find('\n').unwrap_or(new_frontmatter.len());
            new_frontmatter.insert_str(first_newline + 1, &format!("date modified: {}\n", date_str));
        } else {
            new_frontmatter = format!("date modified: {}\n{}", date_str, new_frontmatter);
        }
    }

    // Write back
    let new_content = format!("---\n{}---\n{}", new_frontmatter, rest);
    fs::write(path, new_content)?;
    Ok(())
}

fn extract_date_field(frontmatter: &str, field: &str) -> Option<String> {
    for line in frontmatter.lines() {
        if line.starts_with(&format!("{}: ", field)) {
            return Some(line[field.len() + 2..].trim().to_string());
        }
    }
    None
}

fn replace_date_field(frontmatter: &str, field: &str, new_date: &str) -> String {
    frontmatter
        .lines()
        .map(|line| {
            if line.starts_with(&format!("{}: ", field)) {
                format!("{}: {}", field, new_date)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn print_summary(results: &[MatchResult], total_notes: usize, total_files: usize, dry_run: bool) {
    println!("\n=== SUMMARY ===");
    println!("Evernote notes: {}", total_notes);
    println!("Target files: {}", total_files);

    let matched = results.iter().filter(|r| {
        matches!(r.status, MatchStatus::Updated | MatchStatus::WouldUpdate)
    }).count();

    let no_match = results.iter().filter(|r| {
        matches!(r.status, MatchStatus::NoMatch)
    }).count();

    let errors = results.iter().filter(|r| {
        matches!(r.status, MatchStatus::Error(_))
    }).count();

    if dry_run {
        println!("\nFiles that would be updated: {}", matched);
    } else {
        println!("\nFiles updated: {}", matched);
    }
    println!("Files with no match: {}", no_match);
    println!("Errors: {}", errors);

    if total_notes > 0 {
        let match_rate = (matched * 100) / total_notes;
        println!("\nMatch rate: {}%", match_rate);
    }

    if errors > 0 {
        println!("\nErrors encountered:");
        for result in results {
            if let MatchStatus::Error(msg) = &result.status {
                println!("  - {}: {}", result.title, msg);
            }
        }
    }

    if dry_run {
        println!("\n💡 Run without --dry-run to apply changes");
    }
}
