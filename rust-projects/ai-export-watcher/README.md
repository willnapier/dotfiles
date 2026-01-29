# ai-export-watcher

Watches `~/Downloads` for AI conversation exports and automatically triggers the appropriate converter.

## What It Does

1. **Monitors** `~/Downloads/` for new JSON files matching AI export patterns
2. **Detects** ChatGPT, Grok, and Gemini exports by filename (case-insensitive)
3. **Triggers** `chatgpt-to-continuum` to convert them to continuum JSONL format
4. **Renames** processed files to `.json.imported` to prevent re-processing
5. **Notifies** via messageboard on failure

## Installation

```bash
cd ~/dotfiles/rust-projects/ai-export-watcher
cargo build --release
```

The binary is symlinked to `~/.local/bin/ai-export-watcher` via Dotter.

## Usage

```bash
# Start watching (runs as long-lived process)
ai-export-watcher
```

Typically run as a background service via launchd (macOS) or systemd (Linux).

## Detected Patterns

| Pattern | Source |
|---------|--------|
| `ChatGPT-*.json` | ChatGPT browser exporter |
| `Grok-*.json` | Grok browser exporter |
| `Gemini-*.json` | Gemini browser exporter |

## How It Fits

This is the file watcher component of the [continuum](https://github.com/willnapier/continuum) import pipeline. When you export a conversation from a browser extension, this watcher detects the file and feeds it to the appropriate converter (`chatgpt-to-continuum`), which writes continuum-format JSONL to the conversation archive.

## Dependencies

- `notify` -- Cross-platform filesystem watcher
- `anyhow` -- Error handling
- `regex` -- Filename pattern matching
