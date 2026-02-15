mod cli;
mod output;
mod transcript;
mod ytdlp;

use anyhow::{bail, Result};
use clap::Parser;
fn main() -> Result<()> {
    let cli = cli::Cli::parse();

    match cli.command {
        Some(cli::Command::Channel {
            url,
            limit,
            lang,
            output_dir,
        }) => process_channel(&url, limit, &lang, output_dir.as_deref()),

        None => {
            let url = cli.url.as_deref().unwrap_or_else(|| {
                eprintln!("Error: provide a YouTube URL or use the 'channel' subcommand");
                eprintln!("Usage: yt-transcript <URL> [--stdout] [--lang LANG]");
                std::process::exit(1);
            });
            process_single(url, cli.stdout, &cli.lang, cli.output_dir.as_deref())
        }
    }
}

fn process_single(
    url: &str,
    to_stdout: bool,
    lang: &str,
    output_dir: Option<&std::path::Path>,
) -> Result<()> {
    let meta = ytdlp::fetch_metadata(url)?;
    eprintln!("Title: {}", meta.title);
    eprintln!("Channel: {}", meta.channel_name());

    let tmp = tempfile::tempdir()?;
    let sub_path = ytdlp::download_subtitles(url, lang, tmp.path())?;

    // Detect if auto-generated (yt-dlp puts "auto" in the filename)
    let is_auto = sub_path
        .to_string_lossy()
        .to_lowercase()
        .contains(".auto.");

    let transcript_text = transcript::parse_json3(&sub_path)?;

    if transcript_text.trim().is_empty() {
        bail!("Transcript was empty after processing");
    }

    let markdown = output::format_markdown(&meta, &transcript_text, is_auto);

    if to_stdout {
        print!("{markdown}");
    } else {
        let out_path = output::output_path(&meta, output_dir)?;
        std::fs::write(&out_path, &markdown)?;
        eprintln!("Saved: {}", out_path.display());
    }

    Ok(())
}

fn process_channel(
    channel_url: &str,
    limit: usize,
    lang: &str,
    output_dir: Option<&std::path::Path>,
) -> Result<()> {
    let video_urls = ytdlp::list_channel_videos(channel_url, limit)?;

    let mut successes = 0;
    let mut failures = 0;

    for (i, url) in video_urls.iter().enumerate() {
        eprintln!("\n--- Video {}/{} ---", i + 1, video_urls.len());
        match process_single(url, false, lang, output_dir) {
            Ok(()) => successes += 1,
            Err(e) => {
                eprintln!("Error: {e:#}");
                failures += 1;
            }
        }
    }

    eprintln!("\nDone: {successes} saved, {failures} failed");
    Ok(())
}
