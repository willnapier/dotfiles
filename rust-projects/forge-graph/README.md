# forge-graph

Graph analysis for a markdown knowledge base. Finds orphans, identifies hubs, computes statistics, and generates interactive HTML visualizations.

## What It Does

1. **Parses** all markdown files in a vault directory
2. **Extracts** `[[wikilinks]]` and builds a directed graph (via `petgraph`)
3. **Analyzes** connectivity: orphan detection, hub identification, link statistics
4. **Visualizes** the graph as an interactive HTML page with pre-computed force-directed layout

## Installation

```bash
cd ~/dotfiles/rust-projects/forge-graph
cargo build --release
```

## Usage

```bash
# Show vault statistics
forge-graph analyze ~/notes

# List orphaned notes (no incoming links)
forge-graph orphans ~/notes --count 20

# Find hub notes (most outgoing links)
forge-graph hubs ~/notes --count 20

# Generate interactive HTML visualization
forge-graph viz ~/notes --output graph.html
forge-graph viz ~/notes --output connected.html --filter connected

# Random orphans for daily connection work
forge-graph daily ~/notes --count 10
```

## Visualization

The `viz` subcommand generates a self-contained HTML file using vis.js with:

- Pre-computed force-directed layout (computed in Rust, not in the browser)
- Color coding: green for connected notes, red for orphans
- Click highlighting of connected neighbors
- Pan, zoom, and hover interactions

## How It Fits

This is a diagnostic tool for knowledge base health. It identifies structural issues (orphaned notes, over-connected hubs) that guide daily maintenance work. The `daily` subcommand picks random orphans to connect, turning graph maintenance into a lightweight daily habit.

For an interactive GUI version, see `forge-graph-viewer`.

## Dependencies

- `petgraph` -- Graph data structure and algorithms
- `walkdir` -- Recursive directory traversal
- `regex` -- Wikilink extraction
- `serde_json` -- JSON generation for HTML visualization
- `clap` -- CLI argument parsing
