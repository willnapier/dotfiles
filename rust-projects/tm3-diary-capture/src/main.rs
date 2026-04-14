use anyhow::{bail, Context, Result};
use chrono::{Local, NaiveDate};
use clap::Parser;
use std::path::PathBuf;

use tm3_diary_capture::archive;
use tm3_diary_capture::client_map::ClientMap;
use tm3_diary_capture::daypage;
use tm3_diary_capture::html::{self, Status};
use tm3_diary_capture::live;

#[derive(Parser)]
#[command(about = "Parse TM3 clinical diary HTML snapshots into DayPage checklists")]
struct Cli {
    /// Path to a TM3 diary HTML file
    #[arg(value_name = "FILE")]
    file: Option<PathBuf>,

    /// Find latest TM3 diary HTML in Downloads
    #[arg(long)]
    latest: bool,

    /// Scrape TM3 diary live via headless Chrome (requires tm3-upload login)
    #[arg(long)]
    live: bool,

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

    /// Skip JSON archive of captured data
    #[arg(long)]
    no_archive: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // --- Source selection: --live, --latest, or FILE ---
    let (schedules, file_path) = if cli.live {
        if cli.file.is_some() || cli.latest {
            bail!("Cannot use --live with FILE or --latest");
        }
        let schedules = live::scrape_diary()?;
        (schedules, None)
    } else {
        let path = match (&cli.file, cli.latest) {
            (Some(path), false) => path.clone(),
            (None, true) => find_latest_tm3_html()?,
            (Some(_), true) => bail!("Cannot specify both FILE and --latest"),
            (None, false) => bail!("Provide a FILE, use --latest, or use --live"),
        };

        eprintln!("Processing: {}", path.display());

        let html_content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read: {}", path.display()))?;

        let s = html::parse_diary(&html_content)?;
        (s, Some(path))
    };

    // Archive captured data (JSON, 7-day retention)
    if !cli.no_archive && !schedules.is_empty() {
        let source = if cli.live { "live" } else { "html" };
        match archive::save(&schedules, source) {
            Ok(path) => eprintln!("Archived: {}", path.display()),
            Err(e) => eprintln!("Warning: archive failed: {e}"),
        }
        match archive::cleanup() {
            Ok(n) if n > 0 => eprintln!("Cleaned up {n} archive(s) older than 7 days"),
            _ => {}
        }
    }

    // Warn if today isn't covered by this export (skip for --live, it's always current)
    let today = Local::now().date_naive();
    if !cli.live && !schedules.is_empty() {
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

    if !unmapped.is_empty() && !cli.dry_run {
        eprintln!();
        eprintln!("╭─ {} unmapped client(s) ─────────────────────", unmapped.len());
        for (name, date, time) in &unmapped {
            eprintln!("│  \"{}\"  ({} {})", name, date.format("%a %b %d"), time);
        }
        eprintln!("╰────────────────────────────────────────────");

        for (name, _date, _time) in &unmapped {
            eprintln!();
            eprint!("Client ID for \"{}\" (initials + birth year, or 's' to skip): ", name);
            let mut input = String::new();
            std::io::stdin().read_line(&mut input).context("Failed to read input")?;
            let id = input.trim();

            if id.is_empty() || id.eq_ignore_ascii_case("s") {
                eprintln!("Skipped \"{}\"", name);
                continue;
            }

            eprintln!("Adding: \"{}\" = \"{}\"", name, id);
            let status = std::process::Command::new("tm3-client-add")
                .args([name.as_str(), id, "--no-recapture"])
                .status()
                .context("Failed to run tm3-client-add")?;

            if !status.success() {
                eprintln!("Warning: tm3-client-add failed for \"{}\"", name);
            }
        }

        // Check if all unmapped clients were resolved
        let map_path = map_path.clone();
        let resolved = if let Ok(map) = ClientMap::load(&map_path) {
            unmapped.iter().all(|(name, _, _)| map.lookup(name).is_some())
        } else {
            false
        };

        if resolved {
            eprintln!();
            eprintln!("All clients mapped. Re-processing...");
            // Re-run ourselves on the same file to generate complete output
            let rerun_source = if let Some(ref fp) = file_path {
                fp.display().to_string()
            } else {
                "--live".to_string()
            };
            let mut rerun_args = vec!["--include-past".to_string(), rerun_source];
            if let Some(d) = cli.date {
                rerun_args.push("--date".to_string());
                rerun_args.push(d.to_string());
            }
            let status = std::process::Command::new("tm3-diary-capture")
                .args(&rerun_args)
                .status()
                .context("Failed to re-run tm3-diary-capture")?;
            if !status.success() {
                eprintln!("Warning: re-run exited with error");
            }
            return Ok(());
        } else {
            if let Some(ref fp) = file_path {
                eprintln!();
                eprintln!("Some clients still unmapped. HTML retained: {}", fp.display());
            }
        }
    } else if !unmapped.is_empty() {
        // dry-run mode: just list them
        eprintln!();
        eprintln!("╭─ {} unmapped client(s) ─────────────────────", unmapped.len());
        for (name, date, time) in &unmapped {
            eprintln!("│  \"{}\"  ({} {})", name, date.format("%a %b %d"), time);
        }
        eprintln!("╰────────────────────────────────────────────");
    }

    // Delete source file only if no unmapped clients remain (not applicable for --live)
    if let Some(ref fp) = file_path {
        if !cli.dry_run && any_output && unmapped.is_empty() {
            std::fs::remove_file(fp)
                .with_context(|| format!("Failed to delete: {}", fp.display()))?;
            eprintln!("Deleted: {}", fp.display());
        }
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
