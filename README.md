# Dotfiles

Personal dotfiles and automation tools managed with [Dotter](https://github.com/SuperCuber/dotter). Cross-platform (macOS + Linux) configuration for a terminal-native workflow built around Nushell, Helix, and a markdown knowledge base.

## Repository Map

| Directory | Contents |
|-----------|----------|
| `nushell/` | Shell configuration, universal knowledge functions, file watchers |
| `helix/` | Modal editor config (Colemak-DH), language servers, keybindings |
| `scripts/` | Standalone shell scripts deployed to `~/.local/bin/` |
| `rust-projects/` | 17 Rust CLI tools (see table below) |
| `docs/` | Architecture documentation for key systems |
| `wezterm/` | GPU-accelerated terminal emulator config |
| `yazi/` | Terminal file manager config and keybindings |
| `zellij/` | Terminal multiplexer layouts and config |
| `git/` | Git configuration |
| `shell/` | Zsh configuration (fallback shell) |
| `launchd/` | macOS LaunchAgent service definitions |
| `hammerspoon/` | macOS automation (window management) |
| `.dotter/` | Dotter deployment configuration |

## Rust Projects

All projects live in `rust-projects/` and build with `cargo build --release`. Binaries are symlinked to `~/.local/bin/` via Dotter.

| Project | Purpose |
|---------|---------|
| [ai-export-watcher](rust-projects/ai-export-watcher/) | Watches ~/Downloads for AI conversation exports, triggers converters |
| [backlinks-init](rust-projects/backlinks-init/) | One-time bulk backlink population for markdown knowledge base |
| [chatgpt-to-continuum](rust-projects/chatgpt-to-continuum/) | Converts ChatGPT/Grok/Gemini JSON exports to continuum JSONL |
| [claude-to-continuum](rust-projects/claude-to-continuum/) | Converts Claude.ai conversation exports to continuum JSONL |
| [concert-capture](rust-projects/concert-capture/) | Extracts concert data from Wigmore Hall HTML snapshots into DayPages |
| [forge-graph](rust-projects/forge-graph/) | Graph analysis for markdown knowledge base (orphans, hubs, viz) |
| [forge-graph-viewer](rust-projects/forge-graph-viewer/) | Interactive graph visualization using egui |
| [forge-metadata-backup](rust-projects/forge-metadata-backup/) | Export and restore file timestamps to CSV |
| [grok-to-continuum](rust-projects/grok-to-continuum/) | Converts Grok data exports to continuum JSONL |
| [module](rust-projects/module/) | Knowledge base module import/export for AI advisor sessions |
| [readwise-sync](rust-projects/readwise-sync/) | Syncs Readwise highlights and Reader articles to local markdown |
| [restore-content-dates](rust-projects/restore-content-dates/) | Restores file dates from Evernote exports (fuzzy matching) |
| [restore-evernote-dates](rust-projects/restore-evernote-dates/) | Restores file dates from Evernote exports (exact matching) |
| [restore-special-char-dates](rust-projects/restore-special-char-dates/) | Restores dates for files with escaped special characters |
| [tm3-diary-capture](rust-projects/tm3-diary-capture/) | Parses clinical diary HTML snapshots into DayPage checklists |
| [wiki-resolve-batch](rust-projects/wiki-resolve-batch/) | Batch-resolves broken wiki links by removing `?[[` markers |

## Key Design Patterns

### Universal Tools

CLI functions that work identically across platforms, editors, and SSH sessions. Built with Nushell + Rust tooling (`rg`, `sd`, `fd`, `sk`):

- `fcit` -- Citation picker
- `fcitz` -- PDF finder with Zotero integration
- `fwl` -- Wiki link picker
- `fsem` -- AI semantic search
- `fsh` -- File search and open
- `fsearch` -- Content search across knowledge base

### File Watchers

Native Nushell file watchers (zero external dependencies) for real-time automation:

- Configuration drift detection and auto-sync
- Activity duration processing on file save
- AI conversation export auto-import
- Bidirectional cross-platform sync via git

### Activity Tracking

A `key:: value` notation system embedded in daily markdown notes, processed by Nushell and Rust tools for quantified tracking of time, activities, and events.

### Cross-Platform Sync

Three-layer synchronization between macOS and Linux:

1. **Dotfiles** -- Git-based (Dotter + auto-push/pull watchers)
2. **Knowledge base** -- Syncthing with 30-day versioning
3. **State coordination** -- Messageboard system for cross-machine signals

## Architecture Documentation

See the `docs/` directory for detailed system design:

- [Entry Notation System](docs/ENTRY-NOTATION-SYSTEM.md) -- The `key:: value` notation and collection architecture
- [File Watcher Architecture](docs/FILE-WATCHER-ARCHITECTURE.md) -- Native Nushell watcher design
- [Literature System Architecture](docs/LITERATURE-SYSTEM-ARCHITECTURE.md) -- Three-layer Zettelkasten design
- [Universal Tool Architecture](docs/UNIVERSAL-TOOL-ARCHITECTURE.md) -- Editor-neutral function design
- [Dotter Orphan Prevention](docs/DOTTER-ORPHAN-PREVENTION.md) -- Configuration management safety
- [Cross-Platform Sync](docs/CROSS-PLATFORM-SYNC.md) -- Bidirectional sync architecture
- [Activity Classification System](docs/ACTIVITY-CLASSIFICATION-SYSTEM.md) -- AI-powered semantic tagging
- [Quantified Tracking Notation](docs/QUANTIFIED-TRACKING-NOTATION.md) -- Duration and activity notation spec

## Related Repositories

| Repository | Description |
|------------|-------------|
| [nushell-knowledge-tools](https://github.com/willnapier/nushell-knowledge-tools) | Universal CLI functions for knowledge base navigation and citation management |
| [helix-knowledge-integration](https://github.com/willnapier/helix-knowledge-integration) | Helix editor integration for the knowledge tools |
| [continuum](https://github.com/willnapier/continuum) | Cross-platform, vendor-neutral AI conversation logging system |

## Usage

### Prerequisites

```bash
# Install Dotter
cargo install dotter
```

### Deploy

```bash
git clone https://github.com/willnapier/dotfiles.git ~/dotfiles
cd ~/dotfiles
dotter deploy
```

### Update

Edit files in `~/dotfiles/`, then commit and push. Changes are reflected immediately via symlinks.

## Automation Philosophy

This repository demonstrates two complementary approaches to workflow automation:

**Universal Tools** prioritize portability -- they work on any platform, in any terminal, over SSH, with zero setup. These are the foundation for anywhere-access to the knowledge base.

**Stack Integrations** prioritize depth -- they leverage specific tool combinations (Helix + Nushell + WezTerm) for context-aware automation within a chosen workflow. File watchers detect what's running and trigger processing automatically.

Both approaches are complementary. Universal functions provide the foundation; stack integrations provide the depth for daily productivity.
