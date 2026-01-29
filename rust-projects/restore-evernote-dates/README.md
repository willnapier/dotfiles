# restore-evernote-dates

Restores file creation dates from Evernote `.enex` exports using exact filename matching.

## What It Does

1. **Parses** an Evernote `.enex` XML export to extract note titles and creation dates
2. **Scans** a target directory for markdown files
3. **Matches** Evernote note titles to local filenames exactly
4. **Restores** file modification timestamps from Evernote creation dates

## Installation

```bash
cd ~/dotfiles/rust-projects/restore-evernote-dates
cargo build --release
```

## Usage

```bash
# Preview matches
restore-evernote-dates ~/exports/notes.enex ~/notes --dry-run

# Restore dates
restore-evernote-dates ~/exports/notes.enex ~/notes

# Verbose output
restore-evernote-dates ~/exports/notes.enex ~/notes --verbose
```

## How It Fits

The first pass in a three-tool timestamp restoration suite. Run this first for exact matches (handles most files), then use `restore-content-dates` for fuzzy matches, and `restore-special-char-dates` for files with escaped special characters.

See `restore-content-dates` README for the full sequence.

## Dependencies

- `quick-xml` -- Evernote `.enex` XML parsing
- `filetime` -- Cross-platform timestamp manipulation
- `walkdir` -- Recursive directory traversal
- `chrono` -- Timestamp handling
- `indicatif` -- Progress bars
- `clap` -- CLI argument parsing
