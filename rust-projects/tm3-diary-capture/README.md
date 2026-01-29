# tm3-diary-capture

Parses TM3 clinical diary HTML snapshots into DayPage checklist entries.

## What It Does

1. **Reads** an HTML snapshot of a TM3 clinical diary page (saved via SingleFile or similar)
2. **Extracts** appointment data: dates, times, client identifiers, and status
3. **Maps** client identifiers to anonymized codes using a configurable mapping file
4. **Generates** checklist entries for each appointment
5. **Appends** entries to the appropriate DayPage markdown files

## Installation

```bash
cd ~/dotfiles/rust-projects/tm3-diary-capture
cargo build --release
```

## Usage

```bash
# Process a specific HTML file
tm3-diary-capture ~/Downloads/diary.html

# Find and process the latest TM3 HTML in Downloads
tm3-diary-capture --latest

# Preview without modifying files
tm3-diary-capture --dry-run ~/Downloads/diary.html

# Process only a specific date
tm3-diary-capture --date 2025-01-15 ~/Downloads/diary.html

# Use a custom client mapping file
tm3-diary-capture --map-file ~/config/clients.toml ~/Downloads/diary.html
```

## Client Mapping

A TOML file maps client names to short codes for privacy. The tool looks for a mapping file at a configurable location.

## How It Fits

Similar to `concert-capture`, this follows the "HTML snapshot to DayPage" pattern: save a web page with a browser extension, run the capture tool, and structured data appears in the daily journal. The client mapping layer ensures no real names appear in the knowledge base.

## Dependencies

- `scraper` -- HTML parsing
- `clap` -- CLI argument parsing
- `chrono` -- Date handling
- `regex` -- Pattern extraction
- `serde` / `toml` -- Client mapping configuration
- `dirs` -- Home directory resolution
