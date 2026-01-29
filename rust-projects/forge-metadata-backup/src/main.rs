use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use csv::Writer;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(name = "forge-metadata-backup")]
#[command(about = "Backup and restore file creation/modification timestamps")]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Export file metadata to CSV
    Export {
        /// Directory to backup (e.g., ~/Forge)
        #[arg(value_name = "DIRECTORY")]
        directory: PathBuf,

        /// Custom output file (default: DIR/.metadata-backup.csv)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Restore file metadata from CSV
    Restore {
        /// Directory to restore  (e.g., ~/Forge)
        #[arg(value_name = "DIRECTORY")]
        directory: PathBuf,

        /// Custom input file (default: DIR/.metadata-backup.csv)
        #[arg(short, long)]
        input: Option<PathBuf>,

        /// Show what would be restored without making changes
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct FileMetadata {
    path: String,
    created: u64,
    modified: u64,
}

fn main() -> Result<()> {
    let args = Args::parse();

    match args.command {
        Commands::Export { directory, output } => export_metadata(&directory, output.as_deref())?,
        Commands::Restore {
            directory,
            input,
            dry_run,
        } => restore_metadata(&directory, input.as_deref(), dry_run)?,
    }

    Ok(())
}

fn export_metadata(dir: &Path, output_file: Option<&Path>) -> Result<()> {
    let dir = fs::canonicalize(dir)
        .with_context(|| format!("Failed to resolve directory: {}", dir.display()))?;

    let backup_file = output_file
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.join(".metadata-backup.csv"));

    println!("Exporting metadata from: {}", dir.display());
    println!("Output file: {}\n", backup_file.display());

    // Collect all files
    println!("Scanning files...");
    let entries: Vec<_> = WalkDir::new(&dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();

    println!("Found {} files\n", entries.len());

    // Create progress bar
    let pb = ProgressBar::new(entries.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("##-"),
    );

    // Extract metadata
    let mut metadata_records = Vec::new();
    for entry in entries {
        let path = entry.path();
        if let Ok(meta) = fs::metadata(path) {
            let created = meta
                .created()
                .unwrap_or(UNIX_EPOCH)
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            let modified = meta
                .modified()
                .unwrap_or(UNIX_EPOCH)
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();

            let relative_path = path
                .strip_prefix(&dir)
                .unwrap()
                .to_string_lossy()
                .to_string();

            metadata_records.push(FileMetadata {
                path: relative_path,
                created,
                modified,
            });
        }
        pb.inc(1);
    }

    pb.finish_with_message("Scan complete");

    // Write to CSV
    println!("\nWriting to CSV...");
    let mut wtr = Writer::from_path(&backup_file)
        .with_context(|| format!("Failed to create CSV file: {}", backup_file.display()))?;

    for record in &metadata_records {
        wtr.serialize(record)?;
    }
    wtr.flush()?;

    let file_size = fs::metadata(&backup_file)?.len();
    println!("\n✅ Exported {} files", metadata_records.len());
    println!(
        "📁 Backup file: {} ({} bytes)",
        backup_file.display(),
        file_size
    );
    println!("\n💡 Tip: Commit this file to git for ultimate protection:");
    println!("   cd {} && git add .metadata-backup.csv && git commit -m 'Update metadata backup'", dir.display());

    Ok(())
}

fn restore_metadata(dir: &Path, input_file: Option<&Path>, dry_run: bool) -> Result<()> {
    let dir = fs::canonicalize(dir)
        .with_context(|| format!("Failed to resolve directory: {}", dir.display()))?;

    let backup_file = input_file
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.join(".metadata-backup.csv"));

    if !backup_file.exists() {
        anyhow::bail!(
            "Backup file not found: {}\n\nRun 'forge-metadata-backup export' first to create a backup.",
            backup_file.display()
        );
    }

    println!("Restoring metadata to: {}", dir.display());
    println!("From backup file: {}", backup_file.display());
    if dry_run {
        println!("🔍 DRY RUN MODE - No changes will be made\n");
    } else {
        println!();
    }

    // Read CSV
    let mut rdr = csv::Reader::from_path(&backup_file)
        .with_context(|| format!("Failed to read CSV file: {}", backup_file.display()))?;

    let records: Vec<FileMetadata> = rdr
        .deserialize()
        .collect::<Result<_, _>>()
        .context("Failed to parse CSV")?;

    println!("Found {} files in backup\n", records.len());

    // Create progress bar
    let pb = ProgressBar::new(records.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("##-"),
    );

    // Restore timestamps
    let mut restored = 0;
    let mut missing = 0;
    let mut errors = 0;

    for record in &records {
        let full_path = dir.join(&record.path);

        if !full_path.exists() {
            missing += 1;
        } else if dry_run {
            restored += 1;
        } else {
            // Set modification time
            let mtime = filetime::FileTime::from_unix_time(record.modified as i64, 0);
            if let Err(_e) = filetime::set_file_mtime(&full_path, mtime) {
                errors += 1;
            } else {
                restored += 1;
            }
        }
        pb.inc(1);
    }

    pb.finish_with_message("Complete");

    // Print summary
    println!("\n=== SUMMARY ===");
    println!("Total files in backup: {}", records.len());

    if dry_run {
        println!("\nFiles that would be restored: {}", restored);
    } else {
        println!("\nFiles restored: {}", restored);
    }
    println!("Files missing - not in directory: {}", missing);
    if errors > 0 {
        println!("Errors: {}", errors);
    }

    if dry_run {
        println!("\n💡 Run without --dry-run to apply changes");
    }

    Ok(())
}
