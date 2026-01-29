# module

Cross-platform module and scroll management for AI advisor sessions. Exports and imports knowledge base modules (collections of markdown files) as portable bundles.

## What It Does

1. **Export**: Collects scrolls (markdown documents) for a named advisor session and packages them as a directory or zip bundle
2. **Import**: Parses conversation JSON to extract module updates and applies them to local files
3. **Verify**: Checks scroll consistency and completeness
4. **List**: Shows current scroll state with optional full content display

## Installation

```bash
cd ~/dotfiles/rust-projects/module
cargo build --release
```

## Usage

```bash
# Export scrolls for an advisor session
module export seneca
module export seneca --zip
module export seneca --output ~/Downloads/

# Import module updates from a conversation
module import ~/Downloads/conversation.json
module import --dry-run ~/Downloads/conversation.json

# Verify scroll consistency
module verify

# List current scrolls
module list
module list --full
```

## How It Fits

This supports a workflow where AI advisor sessions maintain persistent context through "scrolls" -- curated markdown documents that carry knowledge between conversations. The tool manages the lifecycle of these scrolls: exporting them to seed new sessions, importing updates back, and verifying consistency.

## Dependencies

- `clap` -- CLI argument parsing
- `serde` / `serde_json` -- JSON handling
- `chrono` -- Date handling
- `zip` -- Bundle creation
- `dirs` -- Home directory resolution
- `regex` -- Pattern matching
