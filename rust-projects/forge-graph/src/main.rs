use clap::{Parser, Subcommand};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use regex::Regex;
use petgraph::graph::{Graph, NodeIndex};
use anyhow::{Context, Result};

#[derive(Parser)]
#[command(name = "forge-graph")]
#[command(about = "Blazing-fast graph analysis for your Zettelkasten", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Analyze vault and show statistics
    Analyze {
        /// Path to vault directory
        vault_path: PathBuf,
    },
    /// Find orphaned notes (no incoming links)
    Orphans {
        /// Path to vault directory
        vault_path: PathBuf,
        /// Number of orphans to display
        #[arg(short, long, default_value_t = 10)]
        count: usize,
    },
    /// Generate interactive HTML visualization
    Viz {
        /// Path to vault directory
        vault_path: PathBuf,
        /// Output HTML file path
        #[arg(short, long, default_value = "graph.html")]
        output: PathBuf,
        /// Filter to apply: "all" (default) or "connected" (only nodes with links)
        #[arg(short, long, default_value = "all")]
        filter: String,
    },
    /// Show random orphans for daily connection work
    Daily {
        /// Path to vault directory
        vault_path: PathBuf,
        /// Number of notes to show
        #[arg(short, long, default_value_t = 10)]
        count: usize,
    },
    /// Find hub notes (notes with most outgoing links)
    Hubs {
        /// Path to vault directory
        vault_path: PathBuf,
        /// Number of hubs to display
        #[arg(short, long, default_value_t = 20)]
        count: usize,
    },
}

#[derive(Debug, Clone)]
struct Note {
    path: PathBuf,
    name: String,
    links: Vec<String>,
}

struct VaultGraph {
    notes: HashMap<String, Note>,
    graph: Graph<String, ()>,
    node_indices: HashMap<String, NodeIndex>,
}

impl VaultGraph {
    fn new() -> Self {
        VaultGraph {
            notes: HashMap::new(),
            graph: Graph::new(),
            node_indices: HashMap::new(),
        }
    }

    fn parse_vault<P: AsRef<Path>>(vault_path: P) -> Result<Self> {
        let mut vault = VaultGraph::new();
        let link_regex = Regex::new(r"!?\[\[([^\]]+)\]\]")?;

        println!("📖 Parsing vault...");

        // First pass: collect all notes
        for entry in WalkDir::new(vault_path.as_ref())
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();

            // Skip non-markdown files and certain directories
            if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }

            if path.to_string_lossy().contains(".git")
                || path.to_string_lossy().contains(".obsidian") {
                continue;
            }

            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            // Read file content
            let content = fs::read_to_string(path)
                .context(format!("Failed to read: {:?}", path))?;

            // Extract wiki links (using HashSet to deduplicate)
            let mut links_set = HashSet::new();
            for cap in link_regex.captures_iter(&content) {
                if let Some(link) = cap.get(1) {
                    let mut link_str = link.as_str().to_string();

                    // Skip media links
                    if link_str.starts_with("linked_media/") {
                        continue;
                    }

                    // Remove alias (after |) and heading (after #)
                    if let Some(pos) = link_str.find('|') {
                        link_str = link_str[..pos].to_string();
                    }
                    if let Some(pos) = link_str.find('#') {
                        link_str = link_str[..pos].to_string();
                    }

                    link_str = link_str.trim().to_string();
                    if !link_str.is_empty() {
                        links_set.insert(link_str);
                    }
                }
            }
            let links: Vec<String> = links_set.into_iter().collect();

            vault.notes.insert(name.clone(), Note {
                path: path.to_path_buf(),
                name: name.clone(),
                links,
            });
        }

        println!("✅ Found {} notes", vault.notes.len());

        // Second pass: build graph
        println!("🔗 Building graph...");

        // Create nodes for all notes
        for name in vault.notes.keys() {
            let idx = vault.graph.add_node(name.clone());
            vault.node_indices.insert(name.clone(), idx);
        }

        // Add edges for links
        for note in vault.notes.values() {
            let source_idx = vault.node_indices.get(&note.name);

            for link in &note.links {
                // Try to find target note (with or without .md extension)
                let target_name = if vault.notes.contains_key(link) {
                    link.clone()
                } else if vault.notes.contains_key(&format!("{}.md", link)) {
                    format!("{}.md", link)
                } else {
                    // Link target doesn't exist as a note
                    continue;
                };

                if let (Some(&src), Some(&tgt)) = (source_idx, vault.node_indices.get(&target_name)) {
                    vault.graph.add_edge(src, tgt, ());
                }
            }
        }

        println!("✅ Graph built with {} edges", vault.graph.edge_count());

        Ok(vault)
    }

    fn find_orphans(&self) -> Vec<String> {
        let mut incoming_links: HashSet<String> = HashSet::new();

        // Collect all link targets
        for note in self.notes.values() {
            for link in &note.links {
                incoming_links.insert(link.clone());
                incoming_links.insert(format!("{}.md", link));
            }
        }

        // Find notes with no incoming links
        self.notes
            .keys()
            .filter(|name| !incoming_links.contains(*name))
            .cloned()
            .collect()
    }

    fn compute_layout(&self) -> HashMap<String, (f64, f64)> {
        use std::collections::HashMap;
        use std::f64::consts::PI;

        let mut positions: HashMap<String, (f64, f64)> = HashMap::new();
        let mut velocities: HashMap<String, (f64, f64)> = HashMap::new();

        // Initialize positions in a circle (better than random for large graphs)
        let node_count = self.notes.len();
        let radius = (node_count as f64).sqrt() * 50.0;

        for (i, name) in self.notes.keys().enumerate() {
            let angle = (i as f64 / node_count as f64) * 2.0 * PI;
            let x = radius * angle.cos();
            let y = radius * angle.sin();
            positions.insert(name.clone(), (x, y));
            velocities.insert(name.clone(), (0.0, 0.0));
        }

        // Force-directed layout parameters
        let k = 100.0; // Ideal spring length
        let iterations = 50; // Fewer iterations since we start with good initial positions
        let damping = 0.9;

        println!("   Running {} iterations on {} nodes...", iterations, node_count);

        for iteration in 0..iterations {
            let mut forces: HashMap<String, (f64, f64)> = HashMap::new();

            // Initialize forces to zero
            for name in self.notes.keys() {
                forces.insert(name.clone(), (0.0, 0.0));
            }

            // Repulsive forces between all nodes
            let names: Vec<_> = self.notes.keys().cloned().collect();
            for i in 0..names.len() {
                for j in (i+1)..names.len() {
                    let name1 = &names[i];
                    let name2 = &names[j];
                    let (x1, y1) = positions[name1];
                    let (x2, y2) = positions[name2];

                    let dx = x2 - x1;
                    let dy = y2 - y1;
                    let distance = (dx * dx + dy * dy).sqrt().max(1.0);

                    // Coulomb's law (repulsion)
                    let force = k * k / distance;
                    let fx = (dx / distance) * force;
                    let fy = (dy / distance) * force;

                    let f1 = forces.get_mut(name1).unwrap();
                    f1.0 -= fx;
                    f1.1 -= fy;

                    let f2 = forces.get_mut(name2).unwrap();
                    f2.0 += fx;
                    f2.1 += fy;
                }
            }

            // Attractive forces along edges (Hooke's law)
            for edge in self.graph.raw_edges() {
                let source = &self.graph[edge.source()];
                let target = &self.graph[edge.target()];

                if let (Some(&(x1, y1)), Some(&(x2, y2))) =
                    (positions.get(source), positions.get(target)) {

                    let dx = x2 - x1;
                    let dy = y2 - y1;
                    let distance = (dx * dx + dy * dy).sqrt().max(1.0);

                    // Spring force
                    let force = (distance - k) * 0.1;
                    let fx = (dx / distance) * force;
                    let fy = (dy / distance) * force;

                    let f1 = forces.get_mut(source).unwrap();
                    f1.0 += fx;
                    f1.1 += fy;

                    let f2 = forces.get_mut(target).unwrap();
                    f2.0 -= fx;
                    f2.1 -= fy;
                }
            }

            // Update positions based on forces
            for name in self.notes.keys() {
                let (fx, fy) = forces[name];
                let (vx, vy) = velocities.get_mut(name).unwrap();

                *vx = (*vx + fx) * damping;
                *vy = (*vy + fy) * damping;

                let (x, y) = positions.get_mut(name).unwrap();
                *x += *vx;
                *y += *vy;
            }

            if iteration % 10 == 0 {
                println!("   Iteration {}/{}...", iteration, iterations);
            }
        }

        positions
    }

    fn analyze(&self) {
        let orphans = self.find_orphans();
        let total = self.notes.len();
        let connected = total - orphans.len();
        let orphan_pct = (orphans.len() as f64 / total as f64) * 100.0;

        println!("\n📊 VAULT ANALYSIS");
        println!("═══════════════════════════════════════════");
        println!("Total notes:        {}", total);
        println!("Connected notes:    {} ({:.1}%)", connected, 100.0 - orphan_pct);
        println!("Orphaned notes:     {} ({:.1}%)", orphans.len(), orphan_pct);
        println!("Total links:        {}", self.graph.edge_count());
        println!("═══════════════════════════════════════════\n");
    }

    fn generate_html_viz<P: AsRef<Path>>(&self, output_path: P, filter: &str) -> Result<()> {
        use serde_json::json;

        println!("🧮 Computing layout positions in Rust (this will be fast!)...");

        let orphans_set: HashSet<String> = self.find_orphans().into_iter().collect();

        // Determine which nodes to include based on filter
        let nodes_to_include: HashSet<String> = if filter == "connected" {
            println!("🔍 Filtering to show only connected notes...");
            self.notes.keys()
                .filter(|name| !orphans_set.contains(*name))
                .cloned()
                .collect()
        } else {
            self.notes.keys().cloned().collect()
        };

        // Compute layout positions using force-directed algorithm
        let positions = self.compute_layout();

        // Build JSON data with pre-computed positions (filtered)
        let mut nodes = Vec::new();
        for name in self.notes.keys() {
            // Skip nodes not in filter
            if !nodes_to_include.contains(name) {
                continue;
            }

            let is_orphan = orphans_set.contains(name);
            let (x, y) = positions.get(name).unwrap_or(&(0.0, 0.0));

            nodes.push(json!({
                "id": name,
                "label": name,
                "x": x,
                "y": y,
                "color": if is_orphan { "#ff6b6b" } else { "#4ecdc4" },
                "title": format!("{}\n{}", name, if is_orphan { "Orphan (no incoming links)" } else { "Connected" })
            }));
        }

        println!("✅ Layout computed! {} nodes included", nodes.len());

        let mut edges = Vec::new();
        for edge in self.graph.raw_edges() {
            let source = &self.graph[edge.source()];
            let target = &self.graph[edge.target()];

            // Only include edges where both source and target are in filtered nodes
            if nodes_to_include.contains(source) && nodes_to_include.contains(target) {
                edges.push(json!({
                    "from": source,
                    "to": target
                }));
            }
        }

        println!("✅ {} edges included", edges.len());

        let graph_data = json!({
            "nodes": nodes,
            "edges": edges
        });

        // Generate HTML with embedded vis.js
        let html = format!(r#"<!DOCTYPE html>
<html>
<head>
    <title>Forge Graph Visualization</title>
    <script type="text/javascript" src="https://unpkg.com/vis-network/standalone/umd/vis-network.min.js"></script>
    <style>
        body {{ margin: 0; padding: 0; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; }}
        #mynetwork {{ width: 100vw; height: 100vh; border: none; }}
        #info {{
            position: absolute;
            top: 10px;
            left: 10px;
            background: rgba(255, 255, 255, 0.95);
            padding: 15px;
            border-radius: 8px;
            box-shadow: 0 2px 10px rgba(0,0,0,0.1);
            max-width: 300px;
        }}
        h2 {{ margin: 0 0 10px 0; font-size: 18px; }}
        .stat {{ margin: 5px 0; font-size: 14px; }}
        .legend {{ margin-top: 10px; font-size: 12px; }}
        .legend-item {{ margin: 5px 0; }}
        .color-box {{ display: inline-block; width: 15px; height: 15px; margin-right: 5px; border-radius: 3px; }}
    </style>
</head>
<body>
    <div id="info">
        <h2>🔗 Forge Graph{}</h2>
        <div class="stat">📄 Showing: {}</div>
        <div class="stat">🔗 Links: {}</div>
        <div class="stat">📊 Total: {}</div>
        <div class="legend">
            <div class="legend-item"><span class="color-box" style="background: #4ecdc4;"></span> Connected</div>
            <div class="legend-item"><span class="color-box" style="background: #ff6b6b;"></span> Orphan</div>
        </div>
    </div>
    <div id="mynetwork"></div>
    <script type="text/javascript">
        const graphData = {};

        const container = document.getElementById('mynetwork');
        const data = graphData;
        const options = {{
            nodes: {{
                shape: 'dot',
                size: 10,
                font: {{
                    size: 12,
                    color: '#333'
                }},
                borderWidth: 2,
                shadow: true
            }},
            edges: {{
                width: 0.5,
                color: {{ color: '#848484', opacity: 0.5 }},
                smooth: {{
                    type: 'continuous'
                }}
            }},
            physics: {{
                enabled: false  // Positions pre-computed in Rust!
            }},
            interaction: {{
                hover: true,
                tooltipDelay: 200
            }}
        }};

        const network = new vis.Network(container, data, options);
        console.log('Graph loaded with pre-computed layout - instant interaction!');

        // Highlight connected nodes on click
        network.on('click', function(params) {{
            if (params.nodes.length > 0) {{
                const nodeId = params.nodes[0];
                const connectedNodes = network.getConnectedNodes(nodeId);
                const highlightNodes = [nodeId, ...connectedNodes];

                // Update all nodes
                const allNodes = data.nodes.get({{ returnType: 'Array' }});
                allNodes.forEach(node => {{
                    if (highlightNodes.includes(node.id)) {{
                        node.borderWidth = 4;
                    }} else {{
                        node.borderWidth = 2;
                        node.color = {{ ...node.color, opacity: 0.3 }};
                    }}
                }});
                data.nodes.update(allNodes);
            }}
        }});

        // Reset on background click
        network.on('deselectNode', function() {{
            const allNodes = data.nodes.get({{ returnType: 'Array' }});
            allNodes.forEach(node => {{
                node.borderWidth = 2;
                delete node.color.opacity;
            }});
            data.nodes.update(allNodes);
        }});
    </script>
</body>
</html>"#,
            if filter == "connected" { " (Connected Only)" } else { "" },
            nodes.len(),
            edges.len(),
            self.notes.len(),
            serde_json::to_string(&graph_data)?
        );

        fs::write(output_path.as_ref(), html)
            .context("Failed to write HTML file")?;

        Ok(())
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Analyze { vault_path } => {
            let vault = VaultGraph::parse_vault(&vault_path)?;
            vault.analyze();
        }

        Commands::Orphans { vault_path, count } => {
            let vault = VaultGraph::parse_vault(&vault_path)?;
            let orphans = vault.find_orphans();

            println!("\n🔍 ORPHANED NOTES (showing {} of {})",
                     count.min(orphans.len()), orphans.len());
            println!("═══════════════════════════════════════════");

            for (i, name) in orphans.iter().take(count).enumerate() {
                println!("{}. {}", i + 1, name);
            }
            println!();
        }

        Commands::Daily { vault_path, count } => {
            let vault = VaultGraph::parse_vault(&vault_path)?;
            let mut orphans = vault.find_orphans();

            // Shuffle for randomness
            use std::collections::hash_map::RandomState;
            use std::hash::{BuildHasher, Hash, Hasher};
            let seed = RandomState::new().build_hasher().finish();
            orphans.sort_by_cached_key(|name| {
                let mut hasher = RandomState::new().build_hasher();
                name.hash(&mut hasher);
                hasher.finish().wrapping_add(seed)
            });

            println!("\n📝 TODAY'S CONNECTION OPPORTUNITIES");
            println!("═══════════════════════════════════════════");
            println!("Work on connecting these {} orphaned notes:\n", count);

            for (i, name) in orphans.iter().take(count).enumerate() {
                if let Some(note) = vault.notes.get(name) {
                    println!("{}. {}", i + 1, name);
                    println!("   Path: {}", note.path.display());
                    println!();
                }
            }
        }

        Commands::Viz { vault_path, output, filter } => {
            let vault = VaultGraph::parse_vault(&vault_path)?;
            println!("\n🎨 Generating HTML visualization...");

            vault.generate_html_viz(&output, &filter)?;

            println!("✅ Interactive graph saved to: {}", output.display());
            println!("\n💡 Open in browser:");
            println!("   open {}", output.display());
        }

        Commands::Hubs { vault_path, count } => {
            let vault = VaultGraph::parse_vault(&vault_path)?;

            // Find notes with most outgoing links
            let mut hubs: Vec<_> = vault.notes.values()
                .map(|note| (note.name.clone(), note.links.len(), note.path.clone()))
                .collect();

            hubs.sort_by(|a, b| b.1.cmp(&a.1)); // Sort by link count descending

            println!("\n🌟 HUB NOTES (notes with most outgoing links)");
            println!("═══════════════════════════════════════════");
            println!("Showing top {} of {} notes:\n", count.min(hubs.len()), vault.notes.len());

            for (i, (name, link_count, path)) in hubs.iter().take(count).enumerate() {
                println!("{}. {} → {} links", i + 1, name, link_count);
                println!("   Path: {}", path.display());
                println!();
            }

            // Show statistics
            if !hubs.is_empty() {
                let total_links: usize = hubs.iter().map(|(_, count, _)| count).sum();
                let avg_links = total_links / hubs.len();
                let top_10_percent = hubs.len() / 10;
                let top_10_links: usize = hubs.iter().take(top_10_percent).map(|(_, count, _)| count).sum();
                let top_10_percentage = (top_10_links as f64 / total_links as f64) * 100.0;

                println!("📊 STATISTICS:");
                println!("═══════════════════════════════════════════");
                println!("Average links per note: {}", avg_links);
                println!("Top 10% of notes contain: {:.1}% of all outgoing links", top_10_percentage);
                println!();
            }
        }
    }

    Ok(())
}
