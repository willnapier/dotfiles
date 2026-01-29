# restore-content-dates

Restores file creation dates from Evernote `.enex` exports using multi-strategy matching including fuzzy title comparison.

## What It Does

1. **Parses** an Evernote `.enex` XML export to extract note titles and creation dates
2. **Scans** a target directory for markdown files
3. **Matches** Evernote notes to local files using multiple strategies:
   - Exact filename match
   - Unicode-normalized comparison
   - Fuzzy matching via Jaro-Winkler similarity
4. **Restores** file timestamps from the Evernote creation dates

## Installation

```bash
cd ~/dotfiles/rust-projects/restore-content-dates
cargo build --release
```

## Usage

```bash
# Preview matches without changing anything
restore-content-dates ~/exports/notes.enex ~/notes --dry-run

# Restore dates with verbose output
restore-content-dates ~/exports/notes.enex ~/notes --verbose

# Restore dates
restore-content-dates ~/exports/notes.enex ~/notes
```

## How It Fits

Part of a three-tool suite for restoring timestamps after migrating from Evernote:

- **restore-evernote-dates** -- Exact filename matching (fast, handles most files)
- **restore-content-dates** -- Fuzzy matching (catches renamed or normalized files)
- **restore-special-char-dates** -- Special character handling (files where `?`, `!`, `:` were replaced with `_`)

Use them in sequence: exact first, then fuzzy, then special characters, to maximize coverage.

## Dependencies

- `quick-xml` -- Evernote `.enex` XML parsing
- `strsim` -- Jaro-Winkler fuzzy string matching
- `unicode-normalization` -- Unicode normalization for comparison
- `walkdir` -- Recursive directory traversal
- `chrono` -- Timestamp handling
- `indicatif` -- Progress bars
- `clap` -- CLI argument parsing
