# wiki-resolve-batch

Batch cleanup of `?[[` markers for wiki links that now resolve. Removes the unresolved-link prefix from wikilinks whose target files have been created since the marker was added.

## What It Does

1. **Scans** all markdown files across configured directories
2. **Finds** `?[[target]]` patterns (and `??[[`, `???[[` from accumulated marking)
3. **Checks** whether the target file now exists in the vault
4. **Removes** the `?` prefix from resolved links, converting `?[[target]]` back to `[[target]]`
5. **Reports** changes with colored output

## Installation

```bash
cd ~/dotfiles/rust-projects/wiki-resolve-batch
cargo build --release
```

## Usage

```bash
# Dry run -- see what would change
wiki-resolve-batch --dry-run

# Resolve all broken link markers
wiki-resolve-batch

# Scan specific directories
wiki-resolve-batch --dirs ~/notes ~/projects

# Verbose output
wiki-resolve-batch --verbose
```

By default scans `~/Forge`, `~/Admin`, and `~/Assistants`.

## How It Fits

Part of the wiki link management system in [nushell-knowledge-tools](https://github.com/willnapier/nushell-knowledge-tools). When a wikilink's target doesn't exist, the link management system marks it with a `?` prefix (`?[[missing-note]]`). This tool cleans up those markers in batch once the target notes have been created. Uses parallel processing via `rayon`.

## Dependencies

- `walkdir` -- Recursive directory traversal
- `regex` -- Pattern matching for `?[[` markers
- `rayon` -- Parallel file processing
- `colored` -- Terminal color output
- `clap` -- CLI argument parsing
