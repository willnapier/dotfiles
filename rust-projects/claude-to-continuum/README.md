# claude-to-continuum

Converts Claude.ai conversation exports to continuum JSONL format.

## What It Does

1. **Reads** a `conversations.json` file from Claude.ai's data export
2. **Parses** each conversation with its UUID, title, and message history
3. **Converts** messages to continuum's `messages.jsonl` format
4. **Writes** session metadata (`session.json`) with timestamps and message counts
5. **Organizes** output into `date/uuid/` directory structure

## Installation

```bash
cd ~/dotfiles/rust-projects/claude-to-continuum
cargo build --release
```

## Usage

```bash
# Convert Claude.ai export
claude-to-continuum ~/Downloads/conversations.json

# Custom output directory
claude-to-continuum --output ~/my-logs/claude ~/Downloads/conversations.json
```

To get the export: Claude.ai > Settings > Account > Export Data. You'll receive an email with a download link containing `conversations.json`.

## How It Fits

Part of the [continuum](https://github.com/willnapier/continuum) import pipeline alongside `chatgpt-to-continuum` and `grok-to-continuum`. Each converter handles one vendor's export format and produces the same standardized JSONL output.

## Dependencies

- `serde` / `serde_json` -- JSON parsing and serialization
- `chrono` -- Timestamp handling
- `clap` -- CLI argument parsing
