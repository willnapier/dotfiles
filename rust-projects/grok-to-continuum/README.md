# grok-to-continuum

Converts Grok's official data export to continuum JSONL format with interactive conversation selection.

## What It Does

1. **Reads** Grok's `prod-grok-backend.json` export file
2. **Presents** each conversation with a preview (title, date, first 3 messages)
3. **Lets you select** which conversations to import (or use `--all` for batch mode)
4. **Converts** messages to continuum's `messages.jsonl` format
5. **Handles** Grok's MongoDB-style timestamps (`$date.$numberLong`)

## Installation

```bash
cd ~/dotfiles/rust-projects/grok-to-continuum
cargo build --release
```

This package also builds `grok-continuum-manage` for post-import management.

## Usage

```bash
# Interactive selection mode
grok-to-continuum ~/Downloads/prod-grok-backend.json

# Import all conversations
grok-to-continuum --all ~/Downloads/prod-grok-backend.json

# Custom output directory
grok-to-continuum --output ~/my-logs/grok ~/Downloads/prod-grok-backend.json
```

To get the export: Grok > Settings > Account > Download Your Data.

## How It Fits

Part of the [continuum](https://github.com/willnapier/continuum) import pipeline. While `chatgpt-to-continuum` handles Grok's browser exporter format, this tool handles Grok's official data export which uses a different JSON structure with MongoDB-style nested timestamps.

## Dependencies

- `serde` / `serde_json` -- JSON parsing (including MongoDB date format)
- `chrono` -- Timestamp conversion
- `clap` -- CLI argument parsing
