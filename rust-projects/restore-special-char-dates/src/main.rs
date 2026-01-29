use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use clap::Parser;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(name = "restore-special-char-dates")]
#[command(about = "Restore dates for files with special characters (?, !, :, /) that were replaced with _ in filenames")]
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
    evernote_title: String,
    file_title: String,
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

    println!("Restore dates for files with special character substitutions");
    println!("Reading Evernote export: {}", args.enex_file.display());
    println!("Target directory: {}\n", args.target_dir.display());

    // Parse Evernote export
    println!("Parsing Evernote notes...");
    let notes = parse_evernote_export(&args.enex_file)?;
    println!("Found {} notes in Evernote export\n", notes.len());

    // Find notes with special characters
    println!("Filtering notes with special characters (?, !, :, /)...");
    let special_char_notes: Vec<&EvernoteNote> = notes
        .iter()
        .filter(|note| {
            note.title.contains('?')
                || note.title.contains('!')
                || note.title.contains(':')
                || note.title.contains('/')
        })
        .collect();
    println!("Found {} notes with special characters\n", special_char_notes.len());

    // Scan target directory for markdown files
    println!("Scanning target directory for markdown files...");
    let target_files = find_markdown_files(&args.target_dir)?;
    println!("Found {} markdown files\n", target_files.len());

    // Build file index with normalized names
    println!("Building file index with special character mapping...");
    let file_map = build_file_map(&target_files);
    println!("Indexed {} unique filenames\n", file_map.len());

    // Match notes to files using fuzzy matching
    println!("Matching Evernote notes to files with special character substitutions...");
    let results = match_and_process_notes(
        &special_char_notes,
        &file_map,
        args.dry_run,
        args.verbose,
    )?;

    // Print summary
    print_summary(&results, special_char_notes.len(), args.dry_run);

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

fn normalize_title(title: &str) -> String {
    // Replace special characters that macOS/Linux don't allow in filenames with underscore
    title
        .replace('?', "_")
        .replace('!', "_")
        .replace(':', "_")
        .replace('/', "_")
}

fn build_file_map(files: &[PathBuf]) -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();
    for file in files {
        if let Some(stem) = file.file_stem() {
            if let Some(name) = stem.to_str() {
                // Store both original and normalized versions
                map.insert(name.to_string(), file.clone());
            }
        }
    }
    map
}

fn match_and_process_notes(
    notes: &[&EvernoteNote],
    file_map: &HashMap<String, PathBuf>,
    dry_run: bool,
    verbose: bool,
) -> Result<Vec<MatchResult>> {
    let mut results = Vec::new();

    for (idx, note) in notes.iter().enumerate() {
        let result = process_note(note, file_map, dry_run, verbose, idx + 1, notes.len())?;
        results.push(result);
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
    // Normalize the Evernote title by replacing special chars with _
    let normalized_title = normalize_title(&note.title);

    // Try to find matching file using normalized title
    let file_path = match file_map.get(&normalized_title) {
        Some(path) => path,
        None => {
            // Try partial matches - check if any filename starts with the normalized title
            let partial_match = file_map.iter().find(|(k, _)| {
                k.starts_with(&normalized_title) || normalized_title.starts_with(*k)
            });

            match partial_match {
                Some((matched_name, path)) => {
                    if verbose {
                        println!("📝 [{}/{}] Partial match:", idx, total);
                        println!("   Evernote: {}", note.title);
                        println!("   File: {}", matched_name);
                    }
                    path
                }
                None => {
                    if verbose {
                        println!("⊘ [{}/{}] No match:", idx, total);
                        println!("   Evernote: {}", note.title);
                        println!("   Looking for: {}", normalized_title);
                    }
                    return Ok(MatchResult {
                        status: MatchStatus::NoMatch,
                        evernote_title: note.title.clone(),
                        file_title: normalized_title,
                    });
                }
            }
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
                evernote_title: note.title.clone(),
                file_title: normalized_title,
            });
        }
    };

    if dry_run {
        if verbose {
            println!("🔍 [{}/{}] Would update:", idx, total);
            println!("   Evernote: {}", note.title);
            println!("   File: {}", file_path.display());
            println!("   Date: {}", note.created);
        }
        Ok(MatchResult {
            status: MatchStatus::WouldUpdate,
            evernote_title: note.title.clone(),
            file_title: normalized_title,
        })
    } else {
        // Update YAML frontmatter
        match update_yaml_frontmatter(file_path, timestamp) {
            Ok(_) => {
                // Then update file timestamp
                match set_file_mtime(file_path, timestamp) {
                    Ok(_) => {
                        if verbose {
                            println!("✓ [{}/{}] Updated:", idx, total);
                            println!("   Evernote: {}", note.title);
                            println!("   File: {}", file_path.display());
                            println!("   Date: {}", note.created);
                        }
                        Ok(MatchResult {
                            status: MatchStatus::Updated,
                            evernote_title: note.title.clone(),
                            file_title: normalized_title,
                        })
                    }
                    Err(e) => {
                        eprintln!("⚠ [{}/{}] Failed to update mtime: {} - {}", idx, total, note.title, e);
                        Ok(MatchResult {
                            status: MatchStatus::Error(format!("Failed to update mtime: {}", e)),
                            evernote_title: note.title.clone(),
                            file_title: normalized_title,
                        })
                    }
                }
            }
            Err(e) => {
                eprintln!("⚠ [{}/{}] Failed to update YAML: {} - {}", idx, total, note.title, e);
                Ok(MatchResult {
                    status: MatchStatus::Error(format!("Failed to update YAML: {}", e)),
                    evernote_title: note.title.clone(),
                    file_title: normalized_title,
                })
            }
        }
    }
}

fn parse_evernote_timestamp(timestamp: &str) -> Result<i64> {
    // Format: 20151001T080944Z -> Unix timestamp
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

    // Update date created
    if let Some(_) = extract_date_field(&new_frontmatter, "date created") {
        new_frontmatter = replace_date_field(&new_frontmatter, "date created", &date_str);
    } else {
        new_frontmatter = format!("date created: {}\n{}", date_str, new_frontmatter);
    }

    // Update date modified
    if new_frontmatter.contains("date modified:") {
        new_frontmatter = replace_date_field(&new_frontmatter, "date modified", &date_str);
    } else {
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

fn print_summary(results: &[MatchResult], total_notes: usize, dry_run: bool) {
    println!("\n=== SUMMARY ===");
    println!("Notes with special characters: {}", total_notes);

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
                println!("  - {}: {}", result.evernote_title, msg);
            }
        }
    }

    if dry_run {
        println!("\n💡 Run without --dry-run to apply changes");
    }
}
