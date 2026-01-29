# chatgpt-to-continuum

Converts ChatGPT, Grok, and Gemini conversation exports to continuum JSONL format.

## What It Does

1. **Reads** a JSON export file from ChatGPT, Grok, or Gemini
2. **Auto-detects** the export format (browser exporter, browser extension v2.4+, or official OpenAI bulk export)
3. **Converts** messages to continuum's `messages.jsonl` format with normalized roles and timestamps
4. **Writes** session metadata (`session.json`) alongside messages
5. **Organizes** output into date-based directories

## Installation

```bash
cd ~/dotfiles/rust-projects/chatgpt-to-continuum
cargo build --release
```

## Usage

```bash
# Convert a ChatGPT browser export
chatgpt-to-continuum ~/Downloads/ChatGPT-conversation.json

# Convert a Grok export
chatgpt-to-continuum ~/Downloads/Grok-conversation.json

# Force assistant type
chatgpt-to-continuum --assistant grok ~/Downloads/export.json

# Custom output directory
chatgpt-to-continuum --output ~/my-logs/ ~/Downloads/export.json
```

## Supported Formats

| Format | Source | Detection |
|--------|--------|-----------|
| Browser Exporter | ChatGPT Exporter / Grok Exporter extensions | `metadata.dates` field |
| Browser Extension v2.4+ | Grok Exporter newer versions | `exportDate` / `platform` fields |
| Official OpenAI export | Settings > Data Controls > Export | Array of conversations with `mapping` tree |

## How It Fits

This is the core converter in the [continuum](https://github.com/willnapier/continuum) import pipeline. It handles the actual JSON parsing and format conversion. The `ai-export-watcher` calls this tool automatically when exports appear in Downloads.

## Dependencies

- `serde` / `serde_json` -- JSON parsing and serialization
- `chrono` -- Timestamp normalization
- `clap` -- CLI argument parsing
- `regex` -- Content cleanup (trailing timestamps, UI artifacts)
