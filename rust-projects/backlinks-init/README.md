# backlinks-init

One-time bulk backlink population for a markdown knowledge base.

## What It Does

1. **Scans** all markdown files across multiple directories
2. **Extracts** `[[wikilinks]]` from every file
3. **Builds** a reverse index of which files link to which
4. **Appends** a `## Backlinks` section to each target file listing its incoming links
5. **Skips** duplicate filenames for deterministic behavior

## Installation

```bash
cd ~/dotfiles/rust-projects/backlinks-init
cargo build --release
```

## Usage

```bash
# Dry run -- show what would change
backlinks-init --dry-run

# Live run -- modify files
backlinks-init

# Scan specific directories
backlinks-init --dirs ~/notes ~/projects
```

By default scans `~/Forge`, `~/Admin`, `~/Archives`, and `~/Assistants`.

## How It Fits

This is a one-time initialization tool. After running it, incremental backlink maintenance is handled by the Nushell-based link management system in [nushell-knowledge-tools](https://github.com/willnapier/nushell-knowledge-tools). The `## Backlinks` section it creates follows the same format the incremental system uses.

Uses parallel scanning via `rayon` for performance on large vaults.

## Dependencies

- `walkdir` -- Recursive directory traversal
- `regex` -- Wikilink extraction
- `clap` -- CLI argument parsing
- `rayon` -- Parallel file processing
