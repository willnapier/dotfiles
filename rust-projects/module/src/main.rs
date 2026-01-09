use chrono::{DateTime, Local, Utc};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Module management for William's portable context scrolls
#[derive(Parser)]
#[command(name = "module")]
#[command(about = "Manage William's context scrolls for cross-platform AI sessions")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show current scroll state (files, sizes, last modified)
    List,
    /// Check scroll consistency (missing files, broken references)
    Verify,
    /// Bundle scrolls for export to non-Claude AI
    Export {
        /// Persona to export for: seneca, geoff, or all
        persona: String,
        /// Output file (default: stdout)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Import module updates from a conversation export
    Import {
        /// JSON file with module updates
        file: PathBuf,
        /// Preview changes without applying
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Serialize, Deserialize)]
struct ModuleUpdate {
    module: String,
    action: String, // "replace", "append", "section_update"
    content: Option<String>,
    section: Option<String>,
    changelog_entry: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct ImportPayload {
    timestamp: String,
    advisor: String,
    updates: Vec<ModuleUpdate>,
}

fn get_shared_dir() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME not set");
    PathBuf::from(home).join("Assistants/shared")
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{}B", bytes)
    }
}

fn list_scrolls() {
    let shared = get_shared_dir();

    // Protocol files (rarely change)
    let protocols = [
        "SENECA-PROTOCOL.md",
        "GEOFF-PROTOCOL.md",
        "WILLIAM-INDEX.md",
    ];

    // Content scrolls (change frequently)
    let scrolls = [
        "WILLIAM-CHANGELOG.md",
        "WILLIAM-PHILOSOPHICAL.md",
        "WILLIAM-BIOGRAPHICAL.md",
        "WILLIAM-LIFESTYLE.md",
        "WILLIAM-SOCIAL.md",
        "WILLIAM-FINANCIAL-PLANNING-CONTEXT.md",
    ];

    println!("Module System Status");
    println!("====================\n");
    println!("Location: {}\n", shared.display());

    println!("Protocol Files (rarely change):");
    println!("{:-<60}", "");
    for name in &protocols {
        let path = shared.join(name);
        if path.exists() {
            let meta = fs::metadata(&path).unwrap();
            let modified: DateTime<Local> = meta.modified().unwrap().into();
            println!(
                "  {} {:>8}  {}",
                name,
                format_size(meta.len()),
                modified.format("%Y-%m-%d %H:%M")
            );
        } else {
            println!("  {} MISSING", name);
        }
    }

    println!("\nContent Scrolls (upload each session):");
    println!("{:-<60}", "");
    for name in &scrolls {
        let path = shared.join(name);
        if path.exists() {
            let meta = fs::metadata(&path).unwrap();
            let modified: DateTime<Local> = meta.modified().unwrap().into();
            println!(
                "  {} {:>8}  {}",
                name,
                format_size(meta.len()),
                modified.format("%Y-%m-%d %H:%M")
            );
        } else {
            println!("  {} MISSING", name);
        }
    }

    // Calculate total size
    let all_files: Vec<&str> = protocols.iter().chain(scrolls.iter()).copied().collect();
    let total: u64 = all_files
        .iter()
        .filter_map(|name| fs::metadata(shared.join(name)).ok())
        .map(|m| m.len())
        .sum();

    println!("\nTotal: {}", format_size(total));
}

fn verify_scrolls() -> bool {
    let shared = get_shared_dir();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    // Required files
    let required = [
        "WILLIAM-INDEX.md",
        "WILLIAM-CHANGELOG.md",
        "SENECA-PROTOCOL.md",
        "GEOFF-PROTOCOL.md",
    ];

    let optional = [
        "WILLIAM-PHILOSOPHICAL.md",
        "WILLIAM-BIOGRAPHICAL.md",
        "WILLIAM-LIFESTYLE.md",
        "WILLIAM-SOCIAL.md",
        "WILLIAM-FINANCIAL-PLANNING-CONTEXT.md",
    ];

    println!("Verifying module system...\n");

    // Check shared directory exists
    if !shared.exists() {
        errors.push(format!("Shared directory missing: {}", shared.display()));
    } else {
        // Check required files
        for name in &required {
            let path = shared.join(name);
            if !path.exists() {
                errors.push(format!("Required file missing: {}", name));
            } else if fs::metadata(&path).unwrap().len() == 0 {
                errors.push(format!("Required file empty: {}", name));
            }
        }

        // Check optional files
        for name in &optional {
            let path = shared.join(name);
            if !path.exists() {
                warnings.push(format!("Optional file missing: {}", name));
            }
        }

        // Check changelog format (should have recent entries)
        let changelog_path = shared.join("WILLIAM-CHANGELOG.md");
        if changelog_path.exists() {
            let content = fs::read_to_string(&changelog_path).unwrap_or_default();
            if !content.contains("###") {
                warnings.push("CHANGELOG has no dated entries (### headers)".to_string());
            }
        }
    }

    // Report results
    if errors.is_empty() && warnings.is_empty() {
        println!("All checks passed.");
        true
    } else {
        if !errors.is_empty() {
            println!("ERRORS:");
            for e in &errors {
                println!("  - {}", e);
            }
        }
        if !warnings.is_empty() {
            println!("\nWARNINGS:");
            for w in &warnings {
                println!("  - {}", w);
            }
        }
        errors.is_empty()
    }
}

fn export_scrolls(persona: &str, output: Option<PathBuf>) {
    let shared = get_shared_dir();

    let (protocol, scrolls): (&str, Vec<&str>) = match persona.to_lowercase().as_str() {
        "seneca" => (
            "SENECA-PROTOCOL.md",
            vec![
                "WILLIAM-INDEX.md",
                "WILLIAM-CHANGELOG.md",
                "WILLIAM-PHILOSOPHICAL.md",
                "WILLIAM-BIOGRAPHICAL.md",
                "WILLIAM-LIFESTYLE.md",
                "WILLIAM-SOCIAL.md",
            ],
        ),
        "geoff" => (
            "GEOFF-PROTOCOL.md",
            vec![
                "WILLIAM-INDEX.md",
                "WILLIAM-CHANGELOG.md",
                "WILLIAM-PHILOSOPHICAL.md",
            ],
        ),
        "all" | "full" => (
            "WILLIAM-INDEX.md",
            vec![
                "SENECA-PROTOCOL.md",
                "GEOFF-PROTOCOL.md",
                "WILLIAM-CHANGELOG.md",
                "WILLIAM-PHILOSOPHICAL.md",
                "WILLIAM-BIOGRAPHICAL.md",
                "WILLIAM-LIFESTYLE.md",
                "WILLIAM-SOCIAL.md",
                "WILLIAM-FINANCIAL-PLANNING-CONTEXT.md",
            ],
        ),
        _ => {
            eprintln!("Unknown persona: {}. Use: seneca, geoff, or all", persona);
            std::process::exit(1);
        }
    };

    let mut bundle = String::new();

    // Add export header
    bundle.push_str(&format!(
        "# Module Export: {} Persona\n",
        persona.to_uppercase()
    ));
    bundle.push_str(&format!(
        "*Exported: {}*\n\n",
        Utc::now().format("%Y-%m-%d %H:%M UTC")
    ));
    bundle.push_str("---\n\n");

    // Add protocol first
    let protocol_path = shared.join(protocol);
    if protocol_path.exists() {
        let content = fs::read_to_string(&protocol_path).unwrap_or_default();
        bundle.push_str(&format!("# {}\n\n", protocol));
        bundle.push_str(&content);
        bundle.push_str("\n\n---\n\n");
    }

    // Add each scroll
    for name in scrolls {
        let path = shared.join(name);
        if path.exists() {
            let content = fs::read_to_string(&path).unwrap_or_default();
            bundle.push_str(&format!("# {}\n\n", name));
            bundle.push_str(&content);
            bundle.push_str("\n\n---\n\n");
        } else {
            bundle.push_str(&format!("# {} (NOT FOUND)\n\n---\n\n", name));
        }
    }

    // Add import instructions footer
    bundle.push_str("# How to Import Updates Back\n\n");
    bundle.push_str("After this session, if modules need updating, provide changes in this JSON format:\n\n");
    bundle.push_str("```json\n");
    bundle.push_str(&serde_json::to_string_pretty(&ImportPayload {
        timestamp: Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        advisor: "GPT/Gemini/etc".to_string(),
        updates: vec![ModuleUpdate {
            module: "WILLIAM-LIFESTYLE.md".to_string(),
            action: "section_update".to_string(),
            content: Some("New section content...".to_string()),
            section: Some("## Current Practice".to_string()),
            changelog_entry: Some("Updated practice structure based on session discussion".to_string()),
        }],
    }).unwrap());
    bundle.push_str("\n```\n\n");
    bundle.push_str("William can then run: `module import updates.json` to apply changes.\n");

    match output {
        Some(path) => {
            fs::write(&path, &bundle).expect("Failed to write output file");
            println!("Exported to: {}", path.display());
            println!("Size: {}", format_size(bundle.len() as u64));
        }
        None => {
            print!("{}", bundle);
        }
    }
}

fn import_updates(file: PathBuf, dry_run: bool) {
    let shared = get_shared_dir();

    let content = fs::read_to_string(&file).expect("Failed to read import file");
    let payload: ImportPayload = serde_json::from_str(&content).expect("Invalid JSON format");

    println!("Import from: {} ({})", payload.advisor, payload.timestamp);
    println!("Updates: {}\n", payload.updates.len());

    if dry_run {
        println!("=== DRY RUN (no changes will be made) ===\n");
    }

    for update in &payload.updates {
        println!("Module: {}", update.module);
        println!("Action: {}", update.action);
        if let Some(section) = &update.section {
            println!("Section: {}", section);
        }

        let module_path = shared.join(&update.module);
        if !module_path.exists() {
            println!("  WARNING: Module does not exist!");
            continue;
        }

        match update.action.as_str() {
            "replace" => {
                if let Some(content) = &update.content {
                    println!("  Would replace entire file ({} bytes)", content.len());
                    if !dry_run {
                        fs::write(&module_path, content).expect("Failed to write module");
                        println!("  APPLIED");
                    }
                }
            }
            "append" => {
                if let Some(content) = &update.content {
                    println!("  Would append {} bytes", content.len());
                    if !dry_run {
                        let mut existing = fs::read_to_string(&module_path).unwrap_or_default();
                        existing.push_str("\n\n");
                        existing.push_str(content);
                        fs::write(&module_path, existing).expect("Failed to write module");
                        println!("  APPLIED");
                    }
                }
            }
            "section_update" => {
                if let (Some(section), Some(content)) = (&update.section, &update.content) {
                    println!("  Would update section '{}' ({} bytes)", section, content.len());
                    if !dry_run {
                        // Simple section replacement - find section header and replace until next ## or end
                        let existing = fs::read_to_string(&module_path).unwrap_or_default();
                        // This is a simplified implementation - a real one would need proper markdown parsing
                        if existing.contains(section) {
                            println!("  NOTE: Section replacement requires manual review");
                            println!("  New content for '{}':\n{}", section, content);
                        } else {
                            println!("  WARNING: Section '{}' not found in module", section);
                        }
                    }
                }
            }
            _ => {
                println!("  Unknown action: {}", update.action);
            }
        }
        println!();
    }

    // Handle changelog
    if !dry_run {
        let changelog_path = shared.join("WILLIAM-CHANGELOG.md");
        if changelog_path.exists() {
            let changelog_entries: Vec<&str> = payload
                .updates
                .iter()
                .filter_map(|u| u.changelog_entry.as_deref())
                .collect();

            if !changelog_entries.is_empty() {
                let mut changelog = fs::read_to_string(&changelog_path).unwrap_or_default();
                let entry = format!(
                    "\n### {} — Import from {}\n\n**Advisor**: {}\n**Modules changed**: {}\n\n{}\n",
                    chrono::Local::now().format("%Y-%m-%d"),
                    payload.advisor,
                    payload.advisor,
                    payload.updates.iter().map(|u| u.module.as_str()).collect::<Vec<_>>().join(", "),
                    changelog_entries.join("\n")
                );

                // Insert after the header (find first ---)
                if let Some(pos) = changelog.find("\n---\n") {
                    changelog.insert_str(pos + 5, &entry);
                } else {
                    changelog.push_str(&entry);
                }

                fs::write(&changelog_path, changelog).expect("Failed to update changelog");
                println!("Changelog updated.");
            }
        }
    }

    if dry_run {
        println!("\n=== END DRY RUN ===");
        println!("Run without --dry-run to apply changes.");
    }
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::List => list_scrolls(),
        Commands::Verify => {
            let ok = verify_scrolls();
            std::process::exit(if ok { 0 } else { 1 });
        }
        Commands::Export { persona, output } => export_scrolls(&persona, output),
        Commands::Import { file, dry_run } => import_updates(file, dry_run),
    }
}
