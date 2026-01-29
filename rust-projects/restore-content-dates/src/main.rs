use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use strsim::jaro_winkler;
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(name = "restore-content-dates")]
#[command(about = "Restore file creation dates from Evernote export using multi-strategy matching")]
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

    /// Minimum similarity score for fuzzy matching (0.0-1.0, default: 0.85)
    #[arg(long, default_value = "0.85")]
    similarity_threshold: f64,

    /// Only update files with 2025 dates (skip already-correct files)
    #[arg(long)]
    only_2025: bool,
}

#[derive(Debug, Clone)]
struct EvernoteNote {
    title: String,
    created: String,
}

#[derive(Debug)]
struct MarkdownFile {
    path: PathBuf,
    stem: String,
    has_2025_date: bool,
}

#[derive(Debug)]
struct MatchResult {
    status: MatchStatus,
    note_title: String,
    file_path: Option<PathBuf>,
    match_strategy: Option<String>,
}

#[derive(Debug, PartialEq)]
enum MatchStatus {
    Updated,
    WouldUpdate,
    NoMatch,
    Skipped2024,
    Error(String),
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!("Multi-Strategy Date Restoration Tool");
    println!("====================================\n");
    println!("Reading Evernote export: {}", args.enex_file.display());
    println!("Target directory: {}", args.target_dir.display());
    if args.only_2025 {
        println!("Mode: Only updating files with 2025 dates\n");
    } else {
        println!();
    }

    // Parse Evernote export
    println!("Parsing Evernote notes...");
    let notes = parse_evernote_export(&args.enex_file)?;
    println!("Found {} notes in Evernote export\n", notes.len());

    // Scan target directory for markdown files
    println!("Scanning target directory for markdown files...");
    let markdown_files = scan_markdown_files(&args.target_dir, args.only_2025)?;
    println!("Found {} markdown files", markdown_files.len());
    if args.only_2025 {
        let with_2025 = markdown_files.iter().filter(|f| f.has_2025_date).count();
        println!("  ({} with 2025 dates)\n", with_2025);
    } else {
        println!();
    }

    // Build file indexes
    println!("Building file indexes...");
    let (exact_map, fuzzy_list) = build_file_indexes(&markdown_files, args.only_2025);
    println!("Indexed {} files\n", exact_map.len() + fuzzy_list.len());

    // Match notes to files using multiple strategies
    println!("Matching notes to files...");
    let results = match_notes_multi_strategy(
        &notes,
        &exact_map,
        &fuzzy_list,
        args.dry_run,
        args.verbose,
        args.similarity_threshold,
    )?;

    // Print summary
    print_summary(&results, notes.len(), markdown_files.len(), args.dry_run);

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
            Err(e) => return Err(anyhow::anyhow!("Error parsing XML: {:?}", e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(notes)
}

fn scan_markdown_files(dir: &Path, check_2025: bool) -> Result<Vec<MarkdownFile>> {
    let mut files = Vec::new();

    for entry in WalkDir::new(dir).follow_links(false).into_iter() {
        let entry = entry?;
        if entry.file_type().is_file() {
            if let Some(ext) = entry.path().extension() {
                if ext == "md" {
                    let path = entry.path().to_path_buf();
                    let stem = entry.path().file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();

                    let has_2025_date = if check_2025 {
                        check_file_has_2025_date(&path)
                    } else {
                        false
                    };

                    files.push(MarkdownFile {
                        path,
                        stem,
                        has_2025_date,
                    });
                }
            }
        }
    }

    Ok(files)
}

fn check_file_has_2025_date(path: &Path) -> bool {
    if let Ok(content) = fs::read_to_string(path) {
        content.contains("date created: 2025")
    } else {
        false
    }
}

fn build_file_indexes(
    files: &[MarkdownFile],
    only_2025: bool,
) -> (HashMap<String, Vec<&MarkdownFile>>, Vec<&MarkdownFile>) {
    let mut exact_map: HashMap<String, Vec<&MarkdownFile>> = HashMap::new();
    let mut fuzzy_list = Vec::new();

    for file in files {
        if only_2025 && !file.has_2025_date {
            continue;
        }

        // Add to exact match index
        exact_map
            .entry(file.stem.clone())
            .or_insert_with(Vec::new)
            .push(file);

        // Add to fuzzy match list
        fuzzy_list.push(file);
    }

    (exact_map, fuzzy_list)
}

fn match_notes_multi_strategy(
    notes: &[EvernoteNote],
    exact_map: &HashMap<String, Vec<&MarkdownFile>>,
    fuzzy_list: &[&MarkdownFile],
    dry_run: bool,
    verbose: bool,
    similarity_threshold: f64,
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
        let result = match_note_multi_strategy(
            note,
            exact_map,
            fuzzy_list,
            dry_run,
            verbose,
            idx + 1,
            notes.len(),
            similarity_threshold,
        )?;
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

fn match_note_multi_strategy(
    note: &EvernoteNote,
    exact_map: &HashMap<String, Vec<&MarkdownFile>>,
    fuzzy_list: &[&MarkdownFile],
    dry_run: bool,
    verbose: bool,
    idx: usize,
    total: usize,
    similarity_threshold: f64,
) -> Result<MatchResult> {
    // Strategy 1: Exact filename match
    let sanitized_title = sanitize_filename(&note.title);
    if let Some(files) = exact_map.get(&sanitized_title) {
        if !files.is_empty() {
            return process_match(
                note,
                files[0].path.clone(),
                "exact",
                dry_run,
                verbose,
                idx,
                total,
            );
        }
    }

    // Strategy 2: Try multiple sanitization variations
    for variation in generate_sanitization_variations(&note.title) {
        if let Some(files) = exact_map.get(&variation) {
            if !files.is_empty() {
                return process_match(
                    note,
                    files[0].path.clone(),
                    "sanitization",
                    dry_run,
                    verbose,
                    idx,
                    total,
                );
            }
        }
    }

    // Strategy 3: Fuzzy filename matching
    let mut best_match: Option<(&MarkdownFile, f64)> = None;

    for file in fuzzy_list {
        let similarity = jaro_winkler(&sanitized_title.to_lowercase(), &file.stem.to_lowercase());

        if similarity >= similarity_threshold {
            if let Some((_, best_score)) = best_match {
                if similarity > best_score {
                    best_match = Some((file, similarity));
                }
            } else {
                best_match = Some((file, similarity));
            }
        }
    }

    if let Some((file, score)) = best_match {
        if verbose {
            println!("Fuzzy match: {} -> {} (score: {:.2})", note.title, file.stem, score);
        }
        return process_match(
            note,
            file.path.clone(),
            &format!("fuzzy({:.2})", score),
            dry_run,
            verbose,
            idx,
            total,
        );
    }

    // No match found
    if verbose {
        println!("⊘ [{}/{}] No match: {}", idx, total, note.title);
    }
    Ok(MatchResult {
        status: MatchStatus::NoMatch,
        note_title: note.title.clone(),
        file_path: None,
        match_strategy: None,
    })
}

fn sanitize_filename(title: &str) -> String {
    // Remove or replace characters that are commonly stripped in filenames
    title
        .replace(':', "_")
        .replace('/', "_")
        .replace('\\', "_")
        .replace('|', "_")
        .replace('?', "")
        .replace('*', "")
        .replace('<', "")
        .replace('>', "")
        .replace('"', "")
        .trim()
        .to_string()
}

fn generate_sanitization_variations(title: &str) -> Vec<String> {
    let mut variations = Vec::new();

    // Variation 1: Remove all special chars
    let no_special = title
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    variations.push(no_special);

    // Variation 2: Replace colon with dash + space
    variations.push(title.replace(':', " -").replace("  ", " "));

    // Variation 3: Remove trailing punctuation
    variations.push(title.trim_end_matches(&['?', '!', '.', ',']).to_string());

    // Variation 4: Replace slashes with dashes
    variations.push(title.replace('/', "-"));

    variations
}

fn process_match(
    note: &EvernoteNote,
    file_path: PathBuf,
    strategy: &str,
    dry_run: bool,
    verbose: bool,
    idx: usize,
    total: usize,
) -> Result<MatchResult> {
    // Parse the Evernote timestamp
    let timestamp = match parse_evernote_timestamp(&note.created) {
        Ok(ts) => ts,
        Err(e) => {
            if verbose {
                println!("⚠ [{}/{}] Failed to parse date: {} - {}", idx, total, note.title, e);
            }
            return Ok(MatchResult {
                status: MatchStatus::Error(format!("Failed to parse date: {}", e)),
                note_title: note.title.clone(),
                file_path: Some(file_path),
                match_strategy: Some(strategy.to_string()),
            });
        }
    };

    if dry_run {
        if verbose {
            println!("✓ [{}/{}] Would update ({}):", idx, total, strategy);
            println!("   Evernote: {}", note.title);
            println!("   File: {}", file_path.display());
            println!("   Date: {}", note.created);
        }
        Ok(MatchResult {
            status: MatchStatus::WouldUpdate,
            note_title: note.title.clone(),
            file_path: Some(file_path),
            match_strategy: Some(strategy.to_string()),
        })
    } else {
        // Update YAML frontmatter
        match update_yaml_frontmatter(&file_path, timestamp) {
            Ok(_) => {
                if verbose {
                    println!("✓ [{}/{}] Updated ({}):", idx, total, strategy);
                    println!("   Evernote: {}", note.title);
                    println!("   File: {}", file_path.display());
                    println!("   Date: {}", note.created);
                }
                Ok(MatchResult {
                    status: MatchStatus::Updated,
                    note_title: note.title.clone(),
                    file_path: Some(file_path),
                    match_strategy: Some(strategy.to_string()),
                })
            }
            Err(e) => {
                eprintln!("⚠ [{}/{}] Failed to update YAML: {} - {}", idx, total, note.title, e);
                Ok(MatchResult {
                    status: MatchStatus::Error(format!("Failed to update YAML: {}", e)),
                    note_title: note.title.clone(),
                    file_path: Some(file_path),
                    match_strategy: Some(strategy.to_string()),
                })
            }
        }
    }
}

fn parse_evernote_timestamp(timestamp: &str) -> Result<i64> {
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

fn update_yaml_frontmatter(path: &Path, timestamp: i64) -> Result<()> {
    let datetime: DateTime<Utc> = DateTime::from_timestamp(timestamp, 0)
        .ok_or_else(|| anyhow::anyhow!("Invalid timestamp"))?;
    let date_str = datetime.format("%Y-%m-%d %H:%M").to_string();

    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {}", path.display()))?;

    if !content.starts_with("---\n") {
        let new_content = format!(
            "---\ndate created: {}\ndate modified: {}\n---\n{}",
            date_str, date_str, content
        );
        fs::write(path, new_content)?;
        return Ok(());
    }

    let end_marker = content[4..].find("\n---\n");
    if end_marker.is_none() {
        return Err(anyhow::anyhow!("Malformed YAML frontmatter"));
    }

    let end_pos = end_marker.unwrap() + 4;
    let frontmatter = &content[4..end_pos];
    let rest = &content[end_pos + 4..];

    let mut new_frontmatter = String::new();
    let mut has_created = false;
    let mut has_modified = false;

    for line in frontmatter.lines() {
        if line.starts_with("date created:") {
            new_frontmatter.push_str(&format!("date created: {}\n", date_str));
            has_created = true;
        } else if line.starts_with("date modified:") {
            new_frontmatter.push_str(&format!("date modified: {}\n", date_str));
            has_modified = true;
        } else {
            new_frontmatter.push_str(line);
            new_frontmatter.push('\n');
        }
    }

    if !has_created {
        new_frontmatter.insert_str(0, &format!("date created: {}\n", date_str));
    }
    if !has_modified {
        new_frontmatter.insert_str(0, &format!("date modified: {}\n", date_str));
    }

    let new_content = format!("---\n{}---{}", new_frontmatter, rest);
    fs::write(path, new_content)?;
    Ok(())
}

fn print_summary(results: &[MatchResult], total_notes: usize, total_files: usize, dry_run: bool) {
    let updated = results.iter().filter(|r| matches!(r.status, MatchStatus::Updated | MatchStatus::WouldUpdate)).count();
    let no_match = results.iter().filter(|r| matches!(r.status, MatchStatus::NoMatch)).count();
    let errors = results.iter().filter(|r| matches!(r.status, MatchStatus::Error(_))).count();

    // Count by strategy
    let exact = results.iter().filter(|r| r.match_strategy.as_ref().map_or(false, |s| s == "exact")).count();
    let sanitization = results.iter().filter(|r| r.match_strategy.as_ref().map_or(false, |s| s == "sanitization")).count();
    let fuzzy = results.iter().filter(|r| r.match_strategy.as_ref().map_or(false, |s| s.starts_with("fuzzy"))).count();

    println!("\n=== SUMMARY ===");
    println!("Evernote notes: {}", total_notes);
    println!("Target files: {}", total_files);
    println!();
    if dry_run {
        println!("Files would be updated: {}", updated);
    } else {
        println!("Files updated: {}", updated);
    }
    println!("  - Exact matches: {}", exact);
    println!("  - Sanitization variants: {}", sanitization);
    println!("  - Fuzzy matches: {}", fuzzy);
    println!();
    println!("Files with no match: {}", no_match);
    println!("Errors: {}", errors);
    println!();
    println!("Match rate: {}%", (updated * 100) / total_notes.max(1));
}
