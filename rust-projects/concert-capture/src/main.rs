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

    /// Find latest concert HTML in Downloads or Captures/web-archives
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

fn search_roots() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    let downloads = dirs::download_dir().unwrap_or_else(|| home.join("Downloads"));
    vec![downloads, home.join("Captures/web-archives")]
}

const RECENT: std::time::Duration = std::time::Duration::from_secs(3 * 24 * 60 * 60);

fn find_latest_concert_html() -> Result<PathBuf> {
    find_latest_concert_html_in(&search_roots(), std::time::SystemTime::now(), RECENT).context(
        "No recent concert HTML in Downloads or Captures/web-archives (last 3 days)",
    )
}

/// Newest `.html` whose opening bytes look like a known venue page.
/// `ai-export-watcher` may already have moved a SingleFile save out of
/// Downloads into web-archives; Space+D still has to find it.
/// Ignore older clips — web-archives still holds leftover venue pages from
/// months ago, and those must not win over "I just SingleFile'd this".
fn find_latest_concert_html_in(
    dirs: &[PathBuf],
    now: std::time::SystemTime,
    max_age: std::time::Duration,
) -> Option<PathBuf> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let is_html = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("html"))
                .unwrap_or(false);
            if !is_html {
                continue;
            }
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            let too_old = match now.duration_since(mtime) {
                Ok(age) => age > max_age,
                Err(_) => false,
            };
            if too_old {
                continue;
            }
            files.push((mtime, path));
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files
        .into_iter()
        .map(|(_, path)| path)
        .find(|path| html::path_looks_like_concert(path))
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
    let date_str = concert.date.format("%Y-%m-%d").to_string();
    let status = std::process::Command::new("daypage-append")
        .arg("--date")
        .arg(&date_str)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, UNIX_EPOCH};

    fn write_html(dir: &std::path::Path, name: &str, body: &str, unix_secs: u64) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_secs(unix_secs))
            .unwrap();
        path
    }

    #[test]
    fn latest_picks_concert_from_web_archives_when_downloads_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let downloads = tmp.path().join("Downloads");
        let archives = tmp.path().join("web-archives");
        fs::create_dir_all(&downloads).unwrap();
        fs::create_dir_all(&archives).unwrap();

        write_html(&archives, "random.html", "<html>no venue</html>", 2_000);
        let concert = write_html(
            &archives,
            "2026-09-15-Trio Zadig.html",
            "url: https://www.wigmore-hall.org.uk/whats-on/202609131130\n<title>Trio Zadig</title>",
            1_000,
        );

        let found = find_latest_concert_html_in(
            &[downloads, archives],
            UNIX_EPOCH + Duration::from_secs(3_000),
            Duration::from_secs(2_500),
        )
        .unwrap();
        assert_eq!(found, concert);
    }

    #[test]
    fn latest_prefers_newer_concert_across_both_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let downloads = tmp.path().join("Downloads");
        let archives = tmp.path().join("web-archives");
        fs::create_dir_all(&downloads).unwrap();
        fs::create_dir_all(&archives).unwrap();

        write_html(
            &downloads,
            "older.html",
            "url: https://www.wigmore-hall.org.uk/whats-on/202601010000",
            1_000,
        );
        let newer = write_html(
            &archives,
            "newer.html",
            "url: https://ilminsterartscentre.com/whats-on/someone",
            2_000,
        );

        let found = find_latest_concert_html_in(
            &[downloads, archives],
            UNIX_EPOCH + Duration::from_secs(3_000),
            Duration::from_secs(2_500),
        )
        .unwrap();
        assert_eq!(found, newer);
    }

    #[test]
    fn latest_skips_newer_non_concert_html() {
        let tmp = tempfile::tempdir().unwrap();
        let downloads = tmp.path().join("Downloads");
        fs::create_dir_all(&downloads).unwrap();

        let concert = write_html(
            &downloads,
            "concert.html",
            "url: https://www.southbankcentre.co.uk/whats-on/x",
            1_000,
        );
        write_html(&downloads, "later-clip.html", "<html>invoice</html>", 2_000);

        let found = find_latest_concert_html_in(
            &[downloads],
            UNIX_EPOCH + Duration::from_secs(3_000),
            Duration::from_secs(2_500),
        )
        .unwrap();
        assert_eq!(found, concert);
    }

    #[test]
    fn latest_none_when_no_concert_html() {
        let tmp = tempfile::tempdir().unwrap();
        let downloads = tmp.path().join("Downloads");
        fs::create_dir_all(&downloads).unwrap();
        write_html(&downloads, "clip.html", "<html>not a concert</html>", 1_000);
        assert!(find_latest_concert_html_in(
            &[downloads],
            UNIX_EPOCH + Duration::from_secs(3_000),
            Duration::from_secs(2_500),
        )
        .is_none());
    }

    #[test]
    fn latest_ignores_old_venue_html() {
        let tmp = tempfile::tempdir().unwrap();
        let archives = tmp.path().join("web-archives");
        fs::create_dir_all(&archives).unwrap();
        write_html(
            &archives,
            "old-barbican.html",
            "url: https://www.barbican.org.uk/whats-on/x",
            1_000,
        );
        assert!(find_latest_concert_html_in(
            &[archives],
            UNIX_EPOCH + Duration::from_secs(10_000),
            Duration::from_secs(2_500),
        )
        .is_none());
    }
}
