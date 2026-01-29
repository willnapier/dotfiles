# forge-metadata-backup

Export and restore file creation/modification timestamps to CSV. Protects against timestamp loss during migrations, sync operations, or filesystem changes.

## What It Does

1. **Export**: Walks a directory tree and records each file's creation and modification timestamps to a CSV file
2. **Restore**: Reads the CSV and restores modification timestamps to the original files
3. **Progress**: Shows a progress bar for large directories

## Installation

```bash
cd ~/dotfiles/rust-projects/forge-metadata-backup
cargo build --release
```

## Usage

```bash
# Export timestamps to CSV
forge-metadata-backup export ~/notes
forge-metadata-backup export ~/notes --output ~/backups/timestamps.csv

# Preview what would be restored
forge-metadata-backup restore ~/notes --dry-run

# Restore timestamps from CSV
forge-metadata-backup restore ~/notes
forge-metadata-backup restore ~/notes --input ~/backups/timestamps.csv
```

The default CSV location is `<directory>/.metadata-backup.csv`.

## CSV Format

```csv
path,created,modified
notes/2024-01-15.md,1705276800,1705363200
projects/readme.md,1700000000,1705400000
```

Timestamps are Unix epoch seconds. Paths are relative to the backed-up directory.

## How It Fits

File timestamps carry meaningful information in a knowledge base -- they record when notes were originally created, which matters for journal entries and historical context. This tool preserves that metadata through operations that might otherwise destroy it (cross-platform sync, filesystem migrations, backup restores).

## Dependencies

- `walkdir` -- Recursive directory traversal
- `csv` -- CSV reading and writing
- `filetime` -- Cross-platform timestamp manipulation
- `indicatif` -- Progress bars
- `clap` -- CLI argument parsing
