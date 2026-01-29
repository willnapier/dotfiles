use clap::Parser;
use rayon::prelude::*;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use walkdir::WalkDir;

#[derive(Parser)]
#[command(name = "backlinks-init")]
#[command(about = "One-time bulk backlink population for markdown files")]
struct Args {
    /// Dry run - show what would be changed without modifying files
    #[arg(long)]
    dry_run: bool,

    /// Directories to scan (defaults to ~/Forge, ~/Admin, ~/Archives, ~/Assistants)
    #[arg(short, long)]
    dirs: Vec<PathBuf>,
}

fn main() {
    let args = Args::parse();

    let home = std::env::var("HOME").expect("HOME not set");
    let dirs: Vec<PathBuf> = if args.dirs.is_empty() {
        vec!["Forge", "Admin", "Archives", "Assistants"]
            .into_iter()
            .map(|d| PathBuf::from(&home).join(d))
            .filter(|p| p.exists())
            .collect()
    } else {
        args.dirs.into_iter().filter(|p| p.exists()).collect()
    };

    println!("🔗 Backlinks Initialization (Rust)");
    println!(
        "   Scanning: {}",
        dirs.iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if args.dry_run {
        println!("   Mode: DRY RUN (no changes)");
    } else {
        println!("   Mode: LIVE (will modify files)");
    }
    println!();

    // Phase 1: Build file index (filename -> full path)
    // Only index unique filenames; duplicates are skipped for determinism
    println!("📂 Building file index...");
    let mut file_index: HashMap<String, PathBuf> = HashMap::new();
    let mut duplicates: HashSet<String> = HashSet::new();
    let mut all_files: Vec<PathBuf> = Vec::new();

    for dir in &dirs {
        for entry in WalkDir::new(dir)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "md") {
                let filename = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();

                // Track duplicates - don't index them for deterministic behavior
                if file_index.contains_key(&filename) {
                    duplicates.insert(filename);
                } else {
                    file_index.insert(filename, path.to_path_buf());
                }
                all_files.push(path.to_path_buf());
            }
        }
    }

    // Remove duplicates from index
    for dup in &duplicates {
        file_index.remove(dup);
    }

    println!("   Found {} markdown files", all_files.len());
    if !duplicates.is_empty() {
        println!("   Skipping {} duplicate filenames for determinism", duplicates.len());
    }
    println!();

    // Phase 2: Extract links from all files (parallel)
    println!("🔍 Scanning for wikilinks...");
    let link_re = Regex::new(r"\[\[([^\]|#]+)").unwrap();

    let backlinks_map: Mutex<HashMap<PathBuf, HashSet<PathBuf>>> = Mutex::new(HashMap::new());

    all_files.par_iter().for_each(|source_path| {
        if let Ok(content) = fs::read_to_string(source_path) {
            // Exclude the ## Backlinks section from link scanning to prevent cascade
            let content_to_scan = if let Some(backlinks_pos) = content.find("## Backlinks") {
                &content[..backlinks_pos]
            } else {
                &content[..]
            };

            for cap in link_re.captures_iter(content_to_scan) {
                let link_name = cap[1].trim();

                // Look up target in index
                if let Some(target_path) = file_index.get(link_name) {
                    if target_path != source_path {
                        let mut map = backlinks_map.lock().unwrap();
                        map.entry(target_path.clone())
                            .or_insert_with(HashSet::new)
                            .insert(source_path.clone());
                    }
                }
            }
        }
    });

    let backlinks_map = backlinks_map.into_inner().unwrap();
    println!("   Found {} files with incoming backlinks", backlinks_map.len());
    println!();

    if backlinks_map.is_empty() {
        println!("✅ No backlinks to populate");
        return;
    }

    // Phase 3: Update files
    let mut updated_count = 0;
    let mut skipped_count = 0;

    for (target_path, sources) in &backlinks_map {
        let mut backlink_lines: Vec<String> = sources
            .iter()
            .map(|p| {
                let name = p.file_stem().unwrap_or_default().to_string_lossy();
                format!("- [[{}]]", name)
            })
            .collect();
        backlink_lines.sort();
        backlink_lines.dedup();
        let backlinks_text = backlink_lines.join("\n");

        if args.dry_run {
            println!(
                "Would update: {} with {} backlinks",
                target_path.file_name().unwrap_or_default().to_string_lossy(),
                sources.len()
            );
            continue;
        }

        match update_backlinks_section(target_path, &backlinks_text) {
            Ok(true) => {
                updated_count += 1;
                if updated_count % 100 == 0 {
                    println!("   Updated {} files...", updated_count);
                }
            }
            Ok(false) => skipped_count += 1,
            Err(_) => skipped_count += 1,
        }
    }

    println!();
    if args.dry_run {
        println!(
            "🔍 Dry run complete. Would update {} files.",
            backlinks_map.len()
        );
    } else {
        println!("✅ Backlinks initialization complete");
        println!("   Updated: {} files", updated_count);
        println!("   Skipped: {} files (no change needed)", skipped_count);
    }
}

fn update_backlinks_section(file_path: &Path, backlinks_text: &str) -> Result<bool, std::io::Error> {
    let content = fs::read_to_string(file_path)?;
    let new_section = format!("## Backlinks\n\n{}\n", backlinks_text);

    let updated = if let Some(start) = content.find("## Backlinks") {
        // Find the end of the backlinks section (next ## heading or end of file)
        let after_header = start + "## Backlinks".len();
        let section_end = content[after_header..]
            .find("\n## ")
            .map(|pos| after_header + pos)
            .unwrap_or(content.len());

        // Replace the section
        format!("{}{}{}", &content[..start], new_section, &content[section_end..])
    } else {
        // Add section at end
        format!("{}\n\n{}", content.trim_end(), new_section)
    };

    if updated != content {
        fs::write(file_path, updated)?;
        Ok(true)
    } else {
        Ok(false)
    }
}
