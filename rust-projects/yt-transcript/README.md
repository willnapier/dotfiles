# yt-transcript

Extract YouTube video transcripts to local markdown files. Shells out to `yt-dlp` for the actual YouTube interaction (metadata, subtitle download, channel listing) and handles the parsing, deduplication, and formatting in Rust.

## Requirements

- [yt-dlp](https://github.com/yt-dlp/yt-dlp) installed and on PATH
- YouTube may require cookie authentication — see [yt-dlp FAQ](https://github.com/yt-dlp/yt-dlp/wiki/FAQ#how-do-i-pass-cookies-to-yt-dlp) if you hit bot detection errors

## Usage

```
# Single video → ~/Media/transcripts/YYYY-MM-DD-slugified-title.md
yt-transcript "https://www.youtube.com/watch?v=VIDEO_ID"

# Print to stdout (for piping to AI tools)
yt-transcript "https://www.youtube.com/watch?v=VIDEO_ID" --stdout

# Non-English subtitles
yt-transcript "https://www.youtube.com/watch?v=VIDEO_ID" --lang es

# Custom output directory
yt-transcript "https://www.youtube.com/watch?v=VIDEO_ID" -o ./transcripts

# Batch from a channel (default limit: 10)
yt-transcript channel "https://www.youtube.com/@ChannelName" --limit 5
```

## Output format

Markdown with YAML frontmatter:

```markdown
---
title: "Video Title"
channel: "Channel Name"
date: 2026-02-15
url: https://www.youtube.com/watch?v=VIDEO_ID
duration: 12:34
transcript_type: auto-generated
---

# Video Title

First paragraph of transcript text, grouped by natural speech pauses.

Second paragraph after a gap of more than 2 seconds.
```

## How it works

1. `yt-dlp --dump-json` fetches video metadata (title, channel, date, duration)
2. `yt-dlp --write-subs` / `--write-auto-subs` downloads subtitles in json3 format — prefers manual captions over auto-generated when available
3. The json3 events are parsed, deduplicated (auto-captions produce overlapping segments), and grouped into paragraphs on >2s speech gaps
4. Output is written as markdown to file or stdout

## Install

```
cargo install --path .
cp ~/.cargo/bin/yt-transcript ~/.local/bin/
```
