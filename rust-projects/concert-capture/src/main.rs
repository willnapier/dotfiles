mod api;
mod archive;
mod daypage;
mod html;
mod notation;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "concert-capture")]
#[command(about = "Extract concert data from Wigmore Hall HTML snapshots")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// HTML file to process
    #[arg(value_name = "FILE")]
    file: Option<PathBuf>,

    /// Find latest Wigmore HTML in Downloads
    #[arg(long)]
    latest: bool,

    /// Preview without making changes
    #[arg(long)]
    dry_run: bool,

    /// Skip Open Opus API queries (offline mode)
    #[arg(long)]
    no_api: bool,

    /// Output wikilink only (for Helix integration)
    #[arg(long)]
    link_only: bool,

    /// Output entry only (archive file but don't append to DayPage)
    #[arg(long)]
    entry_only: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// List recent concert archives
    List,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::List) => {
            list_archives()?;
        }
        None => {
            let file_path = if cli.latest {
                find_latest_concert_html()?
            } else if let Some(f) = cli.file {
                f
            } else {
                anyhow::bail!("Provide a file path or use --latest");
            };

            process_concert(&file_path, cli.dry_run, cli.no_api, cli.link_only, cli.entry_only)?;
        }
    }

    Ok(())
}

fn find_latest_concert_html() -> Result<PathBuf> {
    let downloads = dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .context("Could not find Downloads directory")?;

    let mut concert_files: Vec<_> = std::fs::read_dir(&downloads)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            name.ends_with(".html") && is_concert_file(&e.path())
        })
        .collect();

    concert_files.sort_by_key(|e| {
        std::cmp::Reverse(e.metadata().and_then(|m| m.modified()).ok())
    });

    concert_files
        .first()
        .map(|e| e.path())
        .context("No concert HTML files found in Downloads")
}

const VENUE_MARKERS: &[&str] = &[
    "wigmore-hall.org.uk",
    "southbankcentre.co.uk",
    "kingsplace.co.uk",
    "barbican.org.uk",
    "ilminsterartscentre.com",
];

fn is_concert_file(path: &PathBuf) -> bool {
    if let Ok(content) = std::fs::read_to_string(path) {
        VENUE_MARKERS.iter().any(|marker| content.contains(marker))
    } else {
        false
    }
}

fn process_concert(path: &PathBuf, dry_run: bool, no_api: bool, link_only: bool, entry_only: bool) -> Result<()> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    let concert = html::parse_concert(&content)?;

    let works_notation: Vec<String> = if no_api {
        concert
            .works
            .iter()
            .map(|w| notation::generate_notation(&w.composer, &w.title, None))
            .collect()
    } else {
        concert
            .works
            .iter()
            .map(|w| {
                let canonical = api::lookup_work(&w.composer, &w.title).ok().flatten();
                notation::generate_notation(&w.composer, &w.title, canonical.as_ref())
            })
            .collect()
    };

    let archive_filename = archive::generate_filename(&concert);
    let archive_path = archive::get_archive_path(&archive_filename);
    let wikilink = format!("[[captures/concerts/{}]]", archive_filename);

    let performers_str: String = concert
        .performers
        .iter()
        .map(|p| notation::performer_tag(p))
        .collect::<Vec<_>>()
        .join(" ");

    let works_str = works_notation.join(" ");

    let venue_tag = venue_to_tag(concert.venue);
    let entry = format!("concert.{}:: {} {} {}", venue_tag, performers_str, works_str, wikilink);

    if link_only {
        print!("{}", wikilink);
        return Ok(());
    }

    if dry_run {
        eprintln!("=== DRY RUN ===");
        eprintln!("Date: {}", concert.date);
        eprintln!("Performers: {:?}", concert.performers);
        eprintln!("Works: {:?}", concert.works);
        eprintln!();
        eprintln!("Entry: {}", entry);
        eprintln!();
        eprintln!("Would archive to: {}", archive_path.display());
        eprintln!("Would append to DayPage: {}", concert.date);
        return Ok(());
    }

    // Archive the HTML file
    archive::move_to_archive(path, &archive_path)?;

    // entry_only mode: archive and output entry, but don't append to DayPage
    if entry_only {
        print!("{}", entry);
        return Ok(());
    }

    eprintln!("Archived to: {}", archive_path.display());

    // Queue entry for DayPage via daypage-append (avoids Helix external modification conflict)
    let status = std::process::Command::new("daypage-append")
        .arg(&entry)
        .status()
        .context("Failed to run daypage-append")?;

    if status.success() {
        eprintln!("Queued entry for DayPage — flush with Space+U in Helix");
    } else {
        anyhow::bail!("daypage-append failed with exit code: {}", status);
    }

    println!("{}", entry);

    Ok(())
}

fn list_archives() -> Result<()> {
    let archive_dir = archive::get_archive_dir();

    if !archive_dir.exists() {
        eprintln!("No archives yet ({})", archive_dir.display());
        return Ok(());
    }

    let mut files: Vec<_> = std::fs::read_dir(&archive_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "html").unwrap_or(false))
        .collect();

    files.sort_by_key(|e| std::cmp::Reverse(e.file_name()));

    for entry in files.iter().take(10) {
        println!("{}", entry.file_name().to_string_lossy());
    }

    Ok(())
}

fn venue_to_tag(venue: html::Venue) -> &'static str {
    match venue {
        html::Venue::WigmoreHall => "wigmore",
        html::Venue::SouthbankCentre => "southbank",
        html::Venue::KingsPlace => "kingsplace",
        html::Venue::Barbican => "barbican",
        html::Venue::IlminsterArts => "ilminster",
        html::Venue::Unknown => "unknown",
    }
}
