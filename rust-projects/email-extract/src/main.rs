mod extract;
mod output;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "email-extract")]
#[command(about = "Extract and convert MIME email files to plain text, markdown, or JSON")]
#[command(version)]
struct Cli {
    /// Email file(s) or directory to process
    #[arg(value_name = "PATH")]
    paths: Vec<PathBuf>,

    /// Output format
    #[arg(short, long, value_enum, default_value = "text")]
    format: OutputFormat,

    /// Include full headers in output (not just From/To/Date/Subject)
    #[arg(long)]
    full_headers: bool,

    /// Output to a directory instead of stdout (one file per email)
    #[arg(short, long)]
    output_dir: Option<PathBuf>,

    /// Prefer HTML body even when text/plain is available
    #[arg(long)]
    prefer_html: bool,

    /// Strip all HTML tags (default: basic tag stripping for HTML fallback)
    #[arg(long)]
    strip_html: bool,

    /// Only extract metadata (no body content)
    #[arg(long)]
    metadata_only: bool,

    /// Process Maildir directory recursively (cur/, new/, tmp/)
    #[arg(long)]
    maildir: bool,

    /// Limit number of emails to process (0 = unlimited)
    #[arg(short = 'n', long, default_value = "0")]
    limit: usize,
}

#[derive(ValueEnum, Clone, Debug)]
enum OutputFormat {
    /// Plain text output
    Text,
    /// Markdown with YAML frontmatter
    Markdown,
    /// JSON output
    Json,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.paths.is_empty() {
        anyhow::bail!("Provide at least one file or directory path");
    }

    // Collect all email file paths
    let email_paths = collect_email_paths(&cli.paths, cli.maildir)?;

    if email_paths.is_empty() {
        eprintln!("No email files found");
        return Ok(());
    }

    let limit = if cli.limit == 0 {
        email_paths.len()
    } else {
        cli.limit.min(email_paths.len())
    };

    // Create output directory if specified
    if let Some(ref out_dir) = cli.output_dir {
        std::fs::create_dir_all(out_dir)
            .with_context(|| format!("Failed to create output directory: {}", out_dir.display()))?;
    }

    let mut results: Vec<extract::EmailData> = Vec::new();
    let mut errors = 0;

    for path in email_paths.iter().take(limit) {
        match extract::parse_email(path, cli.prefer_html, cli.strip_html) {
            Ok(email) => results.push(email),
            Err(e) => {
                eprintln!("Error processing {}: {}", path.display(), e);
                errors += 1;
            }
        }
    }

    // Output results
    match cli.format {
        OutputFormat::Json => {
            if let Some(ref out_dir) = cli.output_dir {
                for email in &results {
                    let filename = output::safe_filename(&email.subject, &email.date) + ".json";
                    let out_path = out_dir.join(&filename);
                    let json = output::to_json(email, cli.metadata_only)?;
                    std::fs::write(&out_path, json)
                        .with_context(|| format!("Failed to write {}", out_path.display()))?;
                }
            } else if results.len() == 1 {
                let json = output::to_json(&results[0], cli.metadata_only)?;
                println!("{}", json);
            } else {
                let json = output::to_json_array(&results, cli.metadata_only)?;
                println!("{}", json);
            }
        }
        OutputFormat::Markdown => {
            if let Some(ref out_dir) = cli.output_dir {
                for email in &results {
                    let filename = output::safe_filename(&email.subject, &email.date) + ".md";
                    let out_path = out_dir.join(&filename);
                    let md = output::to_markdown(email, cli.metadata_only, cli.full_headers);
                    std::fs::write(&out_path, md)
                        .with_context(|| format!("Failed to write {}", out_path.display()))?;
                }
            } else {
                for (i, email) in results.iter().enumerate() {
                    if i > 0 {
                        println!("\n---\n");
                    }
                    let md = output::to_markdown(email, cli.metadata_only, cli.full_headers);
                    print!("{}", md);
                }
            }
        }
        OutputFormat::Text => {
            if let Some(ref out_dir) = cli.output_dir {
                for email in &results {
                    let filename = output::safe_filename(&email.subject, &email.date) + ".txt";
                    let out_path = out_dir.join(&filename);
                    let txt = output::to_text(email, cli.metadata_only, cli.full_headers);
                    std::fs::write(&out_path, txt)
                        .with_context(|| format!("Failed to write {}", out_path.display()))?;
                }
            } else {
                for (i, email) in results.iter().enumerate() {
                    if i > 0 {
                        println!("\n{}\n", "=".repeat(72));
                    }
                    let txt = output::to_text(email, cli.metadata_only, cli.full_headers);
                    print!("{}", txt);
                }
            }
        }
    }

    // Summary to stderr when processing multiple files
    if results.len() + errors > 1 {
        eprintln!(
            "\nProcessed {} email(s), {} error(s)",
            results.len(),
            errors
        );
    }

    Ok(())
}

/// Collect all email file paths from the given paths.
/// If a path is a directory, scan for email files within it.
/// If --maildir is set, look specifically in cur/, new/, tmp/ subdirectories.
fn collect_email_paths(paths: &[PathBuf], maildir: bool) -> Result<Vec<PathBuf>> {
    let mut email_paths = Vec::new();

    for path in paths {
        if path.is_file() {
            email_paths.push(path.clone());
        } else if path.is_dir() {
            if maildir {
                // Scan Maildir subdirectories
                for subdir in &["cur", "new", "tmp"] {
                    let dir = path.join(subdir);
                    if dir.is_dir() {
                        scan_directory(&dir, &mut email_paths)?;
                    }
                }
            } else {
                scan_directory(path, &mut email_paths)?;
            }
        } else {
            eprintln!("Warning: {} does not exist, skipping", path.display());
        }
    }

    // Sort by modification time, newest first
    email_paths.sort_by(|a, b| {
        let a_time = a
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let b_time = b
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        b_time.cmp(&a_time)
    });

    Ok(email_paths)
}

fn scan_directory(dir: &PathBuf, paths: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("Failed to read directory: {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            // Skip hidden files and common non-email files
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with('.')
                && !name.ends_with(".json")
                && !name.ends_with(".lock")
                && !name.ends_with(".db")
            {
                paths.push(path);
            }
        }
    }
    Ok(())
}
