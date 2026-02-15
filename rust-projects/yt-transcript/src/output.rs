use crate::ytdlp::VideoMetadata;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Generate markdown with YAML frontmatter.
pub fn format_markdown(meta: &VideoMetadata, transcript: &str, is_auto: bool) -> String {
    let date = meta.formatted_date().unwrap_or_else(|| "unknown".into());
    let duration = meta
        .duration_string
        .clone()
        .unwrap_or_else(|| "unknown".into());
    let transcript_type = if is_auto { "auto-generated" } else { "manual" };

    // Escape YAML special chars in title
    let title_escaped = meta.title.replace('"', r#"\""#);

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("title: \"{title_escaped}\"\n"));
    out.push_str(&format!("channel: \"{}\"\n", meta.channel_name()));
    out.push_str(&format!("date: {date}\n"));
    out.push_str(&format!("url: {}\n", meta.webpage_url));
    out.push_str(&format!("duration: {duration}\n"));
    out.push_str(&format!("transcript_type: {transcript_type}\n"));
    out.push_str("---\n\n");
    out.push_str(&format!("# {}\n\n", meta.title));
    out.push_str(transcript);
    out.push('\n');

    out
}

/// Determine the output file path: ~/Media/transcripts/YYYY-MM-DD-slugified-title.md
pub fn output_path(meta: &VideoMetadata, output_dir: Option<&Path>) -> Result<PathBuf> {
    let dir = match output_dir {
        Some(d) => d.to_path_buf(),
        None => {
            let home = dirs::home_dir().context("Could not determine home directory")?;
            home.join("Media").join("transcripts")
        }
    };

    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create output directory: {}", dir.display()))?;

    let date_prefix = meta.formatted_date().unwrap_or_else(|| "unknown".into());
    let title_slug = slug::slugify(&meta.title);

    // Truncate slug to keep filename reasonable
    let title_slug = if title_slug.len() > 80 {
        title_slug[..80].trim_end_matches('-').to_string()
    } else {
        title_slug
    };

    let filename = format!("{date_prefix}-{title_slug}.md");
    Ok(dir.join(filename))
}
