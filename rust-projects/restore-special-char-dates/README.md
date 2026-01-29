# restore-special-char-dates

Restores file dates from Evernote exports for files whose special characters (`?`, `!`, `:`, `/`) were replaced with underscores during export.

## What It Does

1. **Parses** an Evernote `.enex` XML export to extract note titles and creation dates
2. **Normalizes** note titles by replacing special characters with `_` (matching the export behavior)
3. **Matches** normalized titles to local filenames
4. **Restores** file modification timestamps from Evernote creation dates

## Installation

```bash
cd ~/dotfiles/rust-projects/restore-special-char-dates
cargo build --release
```

## Usage

```bash
# Preview matches
restore-special-char-dates ~/exports/notes.enex ~/notes --dry-run

# Restore dates
restore-special-char-dates ~/exports/notes.enex ~/notes

# Verbose output
restore-special-char-dates ~/exports/notes.enex ~/notes --verbose
```

## How It Fits

The third pass in a three-tool timestamp restoration suite. Handles the edge case where Evernote note titles contained special characters that were replaced with underscores when creating filenames. Run after `restore-evernote-dates` (exact) and `restore-content-dates` (fuzzy).

## Dependencies

- `quick-xml` -- Evernote `.enex` XML parsing
- `filetime` -- Cross-platform timestamp manipulation
- `walkdir` -- Recursive directory traversal
- `chrono` -- Timestamp handling
- `clap` -- CLI argument parsing
