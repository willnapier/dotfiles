use anyhow::{bail, Context, Result};
use chrono::{Local, NaiveDate};
use clap::Parser;
use std::path::PathBuf;

use tm3_diary_capture::client_map::ClientMap;
use tm3_diary_capture::daypage;
use tm3_diary_capture::html::{self, Status};

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

    /// Include past dates (by default, only today and future dates are processed)
    #[arg(long)]
    include_past: bool,
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

    // Warn if today isn't covered by this export
    let today = Local::now().date_naive();
    if !schedules.is_empty() {
        let first = schedules.first().unwrap().date;
        let last = schedules.last().unwrap().date;
        if today < first || today > last {
            eprintln!();
            eprintln!("⚠️  WARNING: This export covers {} to {} — today ({}) is not included.",
                first.format("%a %b %-d"), last.format("%a %b %-d"), today.format("%a %b %-d"));
            eprintln!("   You may need to export a fresh diary page from TM3 in the browser.");
            eprintln!();
        }
    }

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

    let filter_date = cli.date;

    let mut any_output = false;
    let mut unmapped: Vec<(String, NaiveDate, String)> = Vec::new(); // (name, date, time)

    for schedule in &schedules {
        if let Some(filter_date) = filter_date {
            if schedule.date != filter_date {
                continue;
            }
        } else if !cli.include_past && schedule.date < today {
            continue;
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

        let mut lines = vec!["clinic::".to_string()];
        for appt in &sorted {
            let client_id = match &client_map {
                Some(map) => match map.lookup(&appt.client_name) {
                    Some(id) => id.to_string(),
                    None => {
                        unmapped.push((
                            appt.client_name.clone(),
                            schedule.date,
                            appt.start_time.clone(),
                        ));
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

        lines.push("- [ ] `clinic-attendance-report`".to_string());
        let block = lines.join("\n");

        println!("{} ({}):", schedule.date, schedule.date.format("%A"));
        println!("{}", block);
        println!();

        if !cli.dry_run {
            daypage::append_entry(&schedule.date, &block)?;
        }

        any_output = true;
    }

    if !any_output {
        eprintln!("No booked appointments found");
    }

    // Deduplicate unmapped clients (same person may appear on multiple days)
    unmapped.sort_by(|a, b| a.0.cmp(&b.0));
    unmapped.dedup_by(|a, b| a.0 == b.0);

    if !unmapped.is_empty() {
        eprintln!();
        eprintln!("╭─ {} unmapped client(s) ─────────────────────", unmapped.len());
        for (name, date, time) in &unmapped {
            eprintln!("│  \"{}\"  ({} {})", name, date.format("%a %b %d"), time);
        }
        eprintln!("│");
        eprintln!("│  Fix with (check TM3 for DOB → initials + birth year):");
        for (name, _, _) in &unmapped {
            eprintln!("│    tm3-client-add \"{}\" <ID>", name);
        }
        eprintln!("╰────────────────────────────────────────────");
        eprintln!();
        eprintln!("HTML retained for re-processing: {}", file_path.display());
    }

    // Delete source file only if no unmapped clients remain
    if !cli.dry_run && any_output && unmapped.is_empty() {
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
