# readwise-sync

Syncs Readwise highlights and Reader articles to local markdown files. Provides a complete local backup of reading annotations with incremental updates.

## What It Does

1. **Syncs highlights** from Readwise API v2 (books, articles, podcasts, tweets)
2. **Syncs documents** from Reader API v3 (saved articles, newsletters, PDFs)
3. **Saves full HTML snapshots** of Reader documents for data sovereignty
4. **Writes markdown** with YAML frontmatter for each source
5. **Tracks sync state** for incremental updates (only fetches what's new)

## Installation

```bash
cd ~/dotfiles/rust-projects/readwise-sync
cargo build --release
```

## Setup

Provide your Readwise API token via either:

```bash
# Environment variable
export READWISE_TOKEN=your_token_here

# Or config file
mkdir -p ~/.config/readwise
echo "your_token_here" > ~/.config/readwise/token
```

Get your token from [readwise.io/access_token](https://readwise.io/access_token).

## Usage

```bash
# Run a full sync
readwise-sync
```

Typically run nightly via launchd (macOS) or systemd (Linux).

## Output Structure

```
~/Captures/readwise/
├── highlights/           # Readwise highlights as markdown
│   ├── books-*.md
│   ├── articles-*.md
│   └── tweets-*.md
├── reader/               # Reader articles as markdown
│   ├── 2024-01-15-article-title.md
│   └── html/             # Full HTML snapshots
│       └── 2024-01-15-article-title.html
└── sync-state.json       # Tracks last sync time
```

## How It Fits

This is the data sovereignty layer for reading annotations. All highlights and saved articles are captured as local plain-text markdown, searchable with standard tools (`rg`, `sk`) and indexable by the knowledge base. The full HTML snapshots ensure content survives even if the original URLs go offline.

## Dependencies

- `reqwest` -- HTTP client for Readwise API
- `serde` / `serde_json` -- JSON handling
- `chrono` -- Timestamp handling
- `slug` -- Safe filename generation
- `dirs` -- Home directory resolution
