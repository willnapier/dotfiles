use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct VideoMetadata {
    pub title: String,
    pub channel: Option<String>,
    pub uploader: Option<String>,
    pub upload_date: Option<String>,
    pub webpage_url: String,
    pub duration: Option<f64>,
    pub duration_string: Option<String>,
    pub id: String,
}

impl VideoMetadata {
    pub fn channel_name(&self) -> &str {
        self.channel
            .as_deref()
            .or(self.uploader.as_deref())
            .unwrap_or("Unknown")
    }

    /// Format upload_date from "YYYYMMDD" to "YYYY-MM-DD"
    pub fn formatted_date(&self) -> Option<String> {
        let d = self.upload_date.as_ref()?;
        if d.len() == 8 {
            Some(format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..8]))
        } else {
            Some(d.clone())
        }
    }
}

/// Fetch video metadata via yt-dlp --dump-json
pub fn fetch_metadata(url: &str) -> Result<VideoMetadata> {
    eprintln!("Fetching metadata...");
    let output = Command::new("yt-dlp")
        .args(["--dump-json", "--no-download", url])
        .output()
        .context("Failed to run yt-dlp — is it installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("yt-dlp metadata failed: {}", stderr.trim());
    }

    serde_json::from_slice(&output.stdout).context("Failed to parse yt-dlp JSON metadata")
}

/// Download subtitles to a temp directory, returning the path to the json3 file.
/// Prefers manual captions; falls back to auto-generated.
pub fn download_subtitles(url: &str, lang: &str, tmp_dir: &Path) -> Result<std::path::PathBuf> {
    eprintln!("Downloading subtitles...");

    // Try manual captions first
    let manual_result = try_download_subs(url, lang, tmp_dir, false);
    if let Ok(path) = manual_result {
        eprintln!("Using manual captions");
        return Ok(path);
    }

    // Fall back to auto-generated
    let auto_result = try_download_subs(url, lang, tmp_dir, true);
    if let Ok(path) = auto_result {
        eprintln!("Using auto-generated captions");
        return Ok(path);
    }

    bail!("No subtitles available for this video (tried manual and auto-generated, language: {lang})")
}

fn try_download_subs(
    url: &str,
    lang: &str,
    tmp_dir: &Path,
    auto_subs: bool,
) -> Result<std::path::PathBuf> {
    let mut args = vec![
        "--skip-download".to_string(),
        "--sub-format".to_string(),
        "json3".to_string(),
        "--sub-langs".to_string(),
        lang.to_string(),
        "-o".to_string(),
        tmp_dir.join("subs.%(ext)s").to_string_lossy().to_string(),
    ];

    if auto_subs {
        args.push("--write-auto-subs".to_string());
    } else {
        args.push("--write-subs".to_string());
    }

    args.push(url.to_string());

    let output = Command::new("yt-dlp")
        .args(&args)
        .output()
        .context("Failed to run yt-dlp for subtitles")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("yt-dlp subtitle download failed: {}", stderr.trim());
    }

    // Find the json3 file in tmp_dir
    find_json3_file(tmp_dir)
}

fn find_json3_file(dir: &Path) -> Result<std::path::PathBuf> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json3") {
            return Ok(path);
        }
    }
    bail!("No json3 subtitle file found in {}", dir.display())
}

/// List video URLs from a channel, up to `limit`.
pub fn list_channel_videos(channel_url: &str, limit: usize) -> Result<Vec<String>> {
    eprintln!("Listing channel videos (limit {limit})...");
    let output = Command::new("yt-dlp")
        .args([
            "--flat-playlist",
            "--print",
            "url",
            "--playlist-end",
            &limit.to_string(),
            channel_url,
        ])
        .output()
        .context("Failed to run yt-dlp for channel listing")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("yt-dlp channel listing failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let urls: Vec<String> = stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    if urls.is_empty() {
        bail!("No videos found for channel: {channel_url}");
    }

    eprintln!("Found {} videos", urls.len());
    Ok(urls)
}
