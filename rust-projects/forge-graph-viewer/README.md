# forge-graph-viewer

Interactive desktop graph visualization for a markdown knowledge base, built with egui.

## What It Does

1. **Parses** all markdown files and extracts wikilinks (same parsing as `forge-graph`)
2. **Renders** the knowledge graph as an interactive desktop application
3. **Supports** ego-network filtering (1-hop and 2-hop views from any selected node)
4. **Computes** force-directed layout with optional physics simulation
5. **Color-codes** orphaned vs. connected notes

## Installation

```bash
cd ~/dotfiles/rust-projects/forge-graph-viewer
cargo build --release
```

Requires a working GPU/display environment for egui rendering.

## Usage

```bash
# Open vault in viewer
forge-graph-viewer ~/notes

# Filter out orphaned notes
forge-graph-viewer ~/notes --filter-orphans
```

### Controls

- **Drag** -- Pan the view
- **Scroll** -- Zoom in/out
- **Click node** -- Select and highlight connections
- **1-Hop / 2-Hop buttons** -- Filter to ego network around selected node
- **Fit to View** -- Reset camera to show all nodes

## How It Fits

The desktop companion to `forge-graph`. While `forge-graph` generates static HTML reports, this provides a native interactive experience for exploring graph structure in real time. Useful for understanding cluster relationships and finding connection opportunities.

## Dependencies

- `eframe` / `egui` -- Immediate-mode GUI framework
- `walkdir` -- Recursive directory traversal
- `regex` -- Wikilink extraction
- `serde_json` -- Data serialization
