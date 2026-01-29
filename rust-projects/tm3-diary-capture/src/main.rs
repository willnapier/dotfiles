mod client_map;
mod daypage;
mod html;

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use clap::Parser;
use std::path::PathBuf;

use client_map::ClientMap;
use html::Status;

#[derive(Parser)]
#[command(about = "Parse TM3 clinical diary HTML snapshots into DayPage checklists")]
struct Cli {
    /// Path to a TM3 diary HTML file
    #[arg(value_name = "FILE")]
    file: Option<PathBuf>,

    /// Find latest TM3 diary HTML in Downloads
    #[arg(long)]
    latest: bool,

    /// Preview without modifying files
    #[arg(long)]
    dry_run: bool,

    /// Only process one specific date (YYYY-MM-DD)
    #[arg(long)]
    date: Option<NaiveDate>,

    /// Override client mapping file path
    #[arg(long)]
    map_file: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let file_path = match (&cli.file, cli.latest) {
        (Some(path), false) => path.clone(),
        (None, true) => find_latest_tm3_html()?,
        (Some(_), true) => bail!("Cannot specify both FILE and --latest"),
        (None, false) => bail!("Provide a FILE or use --latest"),
    };

    eprintln!("Processing: {}", file_path.display());

    let html_content = std::fs::read_to_string(&file_path)
        .with_context(|| format!("Failed to read: {}", file_path.display()))?;

    let schedules = html::parse_diary(&html_content)?;

    let map_path = cli
        .map_file
        .unwrap_or_else(ClientMap::default_path);
    let client_map = match ClientMap::load(&map_path) {
        Ok(map) => Some(map),
        Err(e) => {
            eprintln!("Warning: Could not load client map: {}", e);
            eprintln!("All client names will show as ???");
            None
        }
    };

    let mut any_output = false;
    for schedule in &schedules {
        if let Some(filter_date) = cli.date {
            if schedule.date != filter_date {
                continue;
            }
        }

        let booked: Vec<_> = schedule
            .appointments
            .iter()
            .filter(|a| a.status == Status::Booked)
            .collect();

        if booked.is_empty() {
            continue;
        }

        // Sort by start time
        let mut sorted = booked;
        sorted.sort_by(|a, b| a.start_time.cmp(&b.start_time));

        let mut lines = vec!["clinical.todo::".to_string()];
        for appt in &sorted {
            let client_id = match &client_map {
                Some(map) => match map.lookup(&appt.client_name) {
                    Some(id) => id.to_string(),
                    None => {
                        eprintln!("Warning: unmapped client: {}", appt.client_name);
                        "???".to_string()
                    }
                },
                None => "???".to_string(),
            };

            let mut line = format!("- [ ] {} {}", client_id, appt.start_time);
            if let Some(tag) = &appt.rate_tag {
                line.push(' ');
                line.push_str(tag);
            }
            lines.push(line);
        }

        let block = lines.join("\n");

        println!("{} ({}):", schedule.date, schedule.date.format("%A"));
        println!("{}", block);
        println!();

        if !cli.dry_run {
            daypage::append_entry(&schedule.date, &block)?;
            eprintln!("  → Appended to {}", daypage::get_daypage_path(&schedule.date).display());
        }

        any_output = true;
    }

    if !any_output {
        eprintln!("No booked appointments found");
    }

    // Delete source file after successful processing
    if !cli.dry_run && any_output {
        std::fs::remove_file(&file_path)
            .with_context(|| format!("Failed to delete: {}", file_path.display()))?;
        eprintln!("Deleted: {}", file_path.display());
    }

    Ok(())
}

/// Find the latest TM3 diary HTML in the Downloads directory.
fn find_latest_tm3_html() -> Result<PathBuf> {
    let downloads = dirs::download_dir().context("Could not find Downloads directory")?;

    let mut tm3_files: Vec<_> = std::fs::read_dir(&downloads)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            name.ends_with(".html") && name.contains("tm3")
        })
        .collect();

    tm3_files.sort_by_key(|e| std::cmp::Reverse(e.metadata().and_then(|m| m.modified()).ok()));

    tm3_files
        .first()
        .map(|e| e.path())
        .context("No TM3 diary HTML files found in Downloads")
}
