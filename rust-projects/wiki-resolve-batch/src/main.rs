//! wiki-resolve-batch - Batch cleanup of ?[[ markers for resolved wiki links
//!
//! Scans markdown files and removes ?[[ prefixes where the target file exists.
//! Handles multiple ? prefixes (??[[, ???[[, etc.) from accumulated marking.

use anyhow::{Context, Result};
use clap::Parser;
use colored::*;
use rayon::prelude::*;
use regex::{Captures, Regex};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(name = "wiki-resolve-batch")]
#[command(about = "Batch cleanup of ?[[ markers for wiki links that now resolve")]
struct Args {
    /// Directories to scan (defaults to ~/Forge, ~/Admin, ~/Assistants)
    #[arg(short, long)]
    dirs: Vec<PathBuf>,

    /// Dry run - show what would be changed without modifying files
    #[arg(short = 'n', long)]
    dry_run: bool,

    /// Verbose output - show each file being processed
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let dirs = if args.dirs.is_empty() {
        let home = dirs::home_dir().context("Could not determine home directory")?;
        vec![
            home.join("Forge"),
            home.join("Admin"),
            home.join("Assistants"),
        ]
    } else {
        args.dirs.clone()
    };

    // Filter to only existing directories
    let dirs: Vec<_> = dirs.into_iter().filter(|d| d.exists()).collect();

    if dirs.is_empty() {
        eprintln!("{}", "No valid directories to scan".red());
        return Ok(());
    }

    println!(
        "{} {}",
        "Scanning directories:".blue().bold(),
        dirs.iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );

    if args.dry_run {
        println!("{}", "(Dry run - no files will be modified)".yellow());
    }
    println!();

    // First pass: collect all markdown filenames (without .md extension)
    // This is our "exists" lookup table
    let existing_files: HashSet<String> = dirs
        .iter()
        .flat_map(|dir| {
            WalkDir::new(dir)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .map_or(false, |ext| ext == "md")
                })
                .filter_map(|e| {
                    e.path()
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_string())
                })
        })
        .collect();

    println!(
        "{} {} markdown files indexed",
        "Found".green(),
        existing_files.len()
    );

    // Collect all markdown files to process
    let files: Vec<PathBuf> = dirs
        .iter()
        .flat_map(|dir| {
            WalkDir::new(dir)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .map_or(false, |ext| ext == "md")
                })
                .map(|e| e.path().to_path_buf())
        })
        .collect();

    println!("{} {} files to scan", "Processing".green(), files.len());
    println!();

    // Pattern to match ?[[link]] with one or more ? prefixes
    // Captures: group 1 = the ?'s, group 2 = the link name (without |alias or #header)
    let pattern = Regex::new(r"(\?+)\[\[([^\]|#]+)([^\]]*)\]\]")?;

    let files_modified = AtomicUsize::new(0);
    let markers_cleaned = AtomicUsize::new(0);
    let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());

    // Process files in parallel
    files.par_iter().for_each(|path| {
        match process_file(path, &pattern, &existing_files, args.dry_run, args.verbose) {
            Ok((modified, cleaned)) => {
                if modified {
                    files_modified.fetch_add(1, Ordering::Relaxed);
                }
                markers_cleaned.fetch_add(cleaned, Ordering::Relaxed);
            }
            Err(e) => {
                errors
                    .lock()
                    .unwrap()
                    .push(format!("{}: {}", path.display(), e));
            }
        }
    });

    // Report results
    let modified = files_modified.load(Ordering::Relaxed);
    let cleaned = markers_cleaned.load(Ordering::Relaxed);
    let errs = errors.lock().unwrap();

    println!();
    if args.dry_run {
        println!(
            "{} {} markers in {} files would be cleaned",
            "Dry run:".yellow().bold(),
            cleaned.to_string().green(),
            modified.to_string().green()
        );
    } else {
        println!(
            "{} {} markers in {} files",
            "Cleaned".green().bold(),
            cleaned.to_string().green(),
            modified.to_string().green()
        );
    }

    if !errs.is_empty() {
        println!();
        println!("{} {} errors:", "Encountered".red().bold(), errs.len());
        for err in errs.iter().take(10) {
            println!("  {}", err.red());
        }
        if errs.len() > 10 {
            println!("  ... and {} more", errs.len() - 10);
        }
    }

    Ok(())
}

fn process_file(
    path: &Path,
    pattern: &Regex,
    existing_files: &HashSet<String>,
    dry_run: bool,
    verbose: bool,
) -> Result<(bool, usize)> {
    let content = fs::read_to_string(path).context("Failed to read file")?;

    let mut cleaned_count = 0;
    let mut modified = false;

    // Check if there are any ?[[ markers first (quick check)
    if !content.contains("?[[") {
        return Ok((false, 0));
    }

    let new_content = pattern.replace_all(&content, |caps: &Captures| {
        let question_marks = &caps[1];
        let link_name_raw = &caps[2];
        let suffix = &caps[3]; // |alias or #header part

        // Strip .md extension if present in link (some links include it, some don't)
        let link_name = link_name_raw.strip_suffix(".md").unwrap_or(link_name_raw);

        // Check if target exists
        if existing_files.contains(link_name) {
            // Target exists - remove the ? prefix(es)
            cleaned_count += 1;
            modified = true;
            if verbose {
                println!(
                    "  {} {}[[{}]] -> [[{}]] in {}",
                    "Cleaning:".cyan(),
                    question_marks,
                    link_name_raw,
                    link_name_raw,
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
            }
            format!("[[{}{}]]", link_name_raw, suffix)
        } else {
            // Target doesn't exist - keep the marker (but normalize to single ?)
            if question_marks.len() > 1 {
                // Normalize multiple ?'s to single ?
                cleaned_count += 1;
                modified = true;
                if verbose {
                    println!(
                        "  {} {}[[{}]] -> ?[[{}]] in {}",
                        "Normalizing:".yellow(),
                        question_marks,
                        link_name_raw,
                        link_name_raw,
                        path.file_name().unwrap_or_default().to_string_lossy()
                    );
                }
                format!("?[[{}{}]]", link_name_raw, suffix)
            } else {
                // Already correct - single ? and target doesn't exist
                caps[0].to_string()
            }
        }
    });

    if modified && !dry_run {
        fs::write(path, new_content.as_ref()).context("Failed to write file")?;
    }

    Ok((modified, cleaned_count))
}
