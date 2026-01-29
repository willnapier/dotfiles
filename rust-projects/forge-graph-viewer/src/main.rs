use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use regex::Regex;
use anyhow::{Context, Result};

#[derive(Debug, Clone)]
struct Note {
    name: String,
    links: Vec<String>,
}

#[derive(Clone)]
struct GraphData {
    nodes: Vec<NodeData>,
    edges: Vec<EdgeData>,
    node_map: HashMap<String, usize>,
}

#[derive(Clone)]
struct NodeData {
    name: String,
    x: f32,
    y: f32,
    is_orphan: bool,
}

#[derive(Clone)]
struct EdgeData {
    from: usize,
    to: usize,
}

struct ForgeGraphViewer {
    graph: GraphData,
    full_graph: GraphData, // Keep original graph for reset
    camera_pos: egui::Vec2,
    zoom: f32,
    dragging: bool,
    drag_start: egui::Pos2,
    selected_node: Option<usize>,
    filter_orphans: bool,
    velocities: Vec<(f32, f32)>,
    simulation_running: bool,
    ego_mode: EgoMode,
}

#[derive(Clone, Copy, PartialEq)]
enum EgoMode {
    Full,      // Show entire graph
    OneHop,    // Show selected node + immediate neighbors
    TwoHop,    // Show selected node + neighbors + neighbors of neighbors
}

impl ForgeGraphViewer {
    fn new(vault_path: &Path, filter_orphans: bool) -> Result<Self> {
        println!("📖 Parsing vault at {:?}...", vault_path);
        let graph = parse_vault(vault_path, filter_orphans)?;
        println!("✅ Loaded {} nodes, {} edges", graph.nodes.len(), graph.edges.len());

        let velocities = vec![(0.0, 0.0); graph.nodes.len()];
        let full_graph = graph.clone();

        Ok(Self {
            graph: full_graph.clone(),
            full_graph,
            camera_pos: egui::Vec2::ZERO,
            zoom: 0.5, // Start zoomed out to see the whole circle
            dragging: false,
            drag_start: egui::Pos2::ZERO,
            selected_node: None,
            filter_orphans,
            velocities,
            simulation_running: false, // Disable physics for now - too dense!
            ego_mode: EgoMode::Full,
        })
    }

    fn apply_forces(&mut self) {
        if !self.simulation_running {
            return;
        }

        let k = 30.0; // Ideal spring length (much shorter for dense graphs)
        let damping = 0.9; // Higher damping for faster settling
        let repulsion_strength = 5000.0; // Much weaker repulsion
        let attraction_strength = 0.05; // Stronger attraction to pull clusters together

        let mut forces = vec![(0.0, 0.0); self.graph.nodes.len()];

        // Repulsive forces between all nodes (Barnes-Hut would be better but this works for now)
        for i in 0..self.graph.nodes.len() {
            for j in (i + 1)..self.graph.nodes.len() {
                let node1 = &self.graph.nodes[i];
                let node2 = &self.graph.nodes[j];

                let dx = node2.x - node1.x;
                let dy = node2.y - node1.y;
                let distance = (dx * dx + dy * dy).sqrt().max(1.0);

                // Coulomb's law (repulsion)
                let force = repulsion_strength / (distance * distance);
                let fx = (dx / distance) * force;
                let fy = (dy / distance) * force;

                forces[i].0 -= fx;
                forces[i].1 -= fy;
                forces[j].0 += fx;
                forces[j].1 += fy;
            }
        }

        // Attractive forces along edges (Hooke's law)
        for edge in &self.graph.edges {
            let from = &self.graph.nodes[edge.from];
            let to = &self.graph.nodes[edge.to];

            let dx = to.x - from.x;
            let dy = to.y - from.y;
            let distance = (dx * dx + dy * dy).sqrt().max(1.0);

            // Spring force
            let force = (distance - k) * attraction_strength;
            let fx = (dx / distance) * force;
            let fy = (dy / distance) * force;

            forces[edge.from].0 += fx;
            forces[edge.from].1 += fy;
            forces[edge.to].0 -= fx;
            forces[edge.to].1 -= fy;
        }

        // Update positions based on forces
        let mut total_kinetic_energy = 0.0;
        for i in 0..self.graph.nodes.len() {
            let (fx, fy) = forces[i];

            self.velocities[i].0 = (self.velocities[i].0 + fx) * damping;
            self.velocities[i].1 = (self.velocities[i].1 + fy) * damping;

            self.graph.nodes[i].x += self.velocities[i].0;
            self.graph.nodes[i].y += self.velocities[i].1;

            total_kinetic_energy += self.velocities[i].0 * self.velocities[i].0
                                  + self.velocities[i].1 * self.velocities[i].1;
        }

        // Stop simulation when energy is low (graph has stabilized)
        if total_kinetic_energy < 1.0 {
            self.simulation_running = false;
            println!("⚡ Simulation stabilized!");
        }
    }

    fn recenter_view(&mut self, viewport_size: egui::Vec2) {
        // Calculate bounding box of all nodes
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;

        for node in &self.graph.nodes {
            min_x = min_x.min(node.x);
            max_x = max_x.max(node.x);
            min_y = min_y.min(node.y);
            max_y = max_y.max(node.y);
        }

        // Calculate center of bounding box
        let center_x = (min_x + max_x) / 2.0;
        let center_y = (min_y + max_y) / 2.0;

        // Calculate zoom to fit
        let width = max_x - min_x;
        let height = max_y - min_y;
        let zoom_x = viewport_size.x / width * 0.8; // 80% to add padding
        let zoom_y = viewport_size.y / height * 0.8;
        self.zoom = zoom_x.min(zoom_y).max(0.1).min(10.0);

        // Center camera on graph center
        self.camera_pos = egui::vec2(-center_x, -center_y);
    }

    fn screen_to_world(&self, screen_pos: egui::Pos2, center: egui::Pos2) -> egui::Pos2 {
        let offset = screen_pos - center;
        egui::pos2(
            (offset.x / self.zoom) - self.camera_pos.x,
            (offset.y / self.zoom) - self.camera_pos.y,
        )
    }

    fn world_to_screen(&self, world_pos: egui::Pos2, center: egui::Pos2) -> egui::Pos2 {
        let offset = egui::vec2(
            (world_pos.x + self.camera_pos.x) * self.zoom,
            (world_pos.y + self.camera_pos.y) * self.zoom,
        );
        center + offset
    }

    fn extract_ego_network(&mut self, center_node: usize, hops: usize) {
        // Build adjacency list from full graph
        let mut adj_list: HashMap<usize, Vec<usize>> = HashMap::new();
        for edge in &self.full_graph.edges {
            adj_list.entry(edge.from).or_insert_with(Vec::new).push(edge.to);
            adj_list.entry(edge.to).or_insert_with(Vec::new).push(edge.from);
        }

        // BFS to find nodes within N hops
        let mut nodes_to_include = HashSet::new();
        let mut current_frontier = vec![center_node];
        nodes_to_include.insert(center_node);

        for _ in 0..hops {
            let mut next_frontier = Vec::new();
            for node in current_frontier {
                if let Some(neighbors) = adj_list.get(&node) {
                    for &neighbor in neighbors {
                        if nodes_to_include.insert(neighbor) {
                            next_frontier.push(neighbor);
                        }
                    }
                }
            }
            current_frontier = next_frontier;
        }

        // Build new node map
        let mut new_node_map = HashMap::new();
        let mut new_nodes = Vec::new();

        for (new_idx, &old_idx) in nodes_to_include.iter().enumerate() {
            new_node_map.insert(old_idx, new_idx);
            new_nodes.push(self.full_graph.nodes[old_idx].clone());
        }

        // Build new edges (only between included nodes)
        let mut new_edges = Vec::new();
        for edge in &self.full_graph.edges {
            if let (Some(&new_from), Some(&new_to)) = (
                new_node_map.get(&edge.from),
                new_node_map.get(&edge.to),
            ) {
                new_edges.push(EdgeData {
                    from: new_from,
                    to: new_to,
                });
            }
        }

        // Update selected node index to match new graph
        self.selected_node = new_node_map.get(&center_node).copied();

        // Create filtered graph
        self.graph = GraphData {
            nodes: new_nodes,
            edges: new_edges,
            node_map: HashMap::new(), // Not needed for rendering
        };

        // Reset velocities
        self.velocities = vec![(0.0, 0.0); self.graph.nodes.len()];

        println!("🎯 Ego network: {} nodes, {} edges", self.graph.nodes.len(), self.graph.edges.len());
    }

    fn reset_to_full_graph(&mut self) {
        self.graph = self.full_graph.clone();
        self.velocities = vec![(0.0, 0.0); self.graph.nodes.len()];
        self.ego_mode = EgoMode::Full;
        println!("🌐 Restored full graph: {} nodes, {} edges", self.graph.nodes.len(), self.graph.edges.len());
    }
}

impl eframe::App for ForgeGraphViewer {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Apply physics simulation
        self.apply_forces();

        egui::CentralPanel::default().show(ctx, |ui| {
            let (response, painter) = ui.allocate_painter(
                ui.available_size(),
                egui::Sense::click_and_drag(),
            );

            let rect = response.rect;
            let center = rect.center();

            // Handle mouse wheel zoom
            let scroll_delta = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll_delta.abs() > 0.1 {
                let zoom_factor = 1.0 + scroll_delta * 0.001;
                self.zoom = (self.zoom * zoom_factor).clamp(0.1, 10.0);
            }

            // Handle dragging
            if response.dragged() {
                if !self.dragging {
                    self.dragging = true;
                    self.drag_start = response.interact_pointer_pos().unwrap_or(center);
                }
                if let Some(current_pos) = response.interact_pointer_pos() {
                    let delta = current_pos - self.drag_start;
                    self.camera_pos += delta / self.zoom;
                    self.drag_start = current_pos;
                }
            } else {
                self.dragging = false;
            }

            // Draw edges first (so they appear behind nodes)
            let edge_color = egui::Color32::from_rgba_unmultiplied(132, 132, 132, 50);
            for edge in &self.graph.edges {
                let from = &self.graph.nodes[edge.from];
                let to = &self.graph.nodes[edge.to];

                let from_pos = self.world_to_screen(egui::pos2(from.x, from.y), center);
                let to_pos = self.world_to_screen(egui::pos2(to.x, to.y), center);

                // Only draw edges that are at least partially visible
                if rect.intersects(egui::Rect::from_two_pos(from_pos, to_pos)) {
                    painter.line_segment(
                        [from_pos, to_pos],
                        egui::Stroke::new(0.5, edge_color),
                    );
                }
            }

            // Draw nodes
            let mut visible_count = 0;
            for (idx, node) in self.graph.nodes.iter().enumerate() {
                let pos = self.world_to_screen(egui::pos2(node.x, node.y), center);

                // Draw ALL nodes (remove visibility culling for debugging)
                let node_radius = 5.0 * self.zoom.sqrt().max(3.0); // Ensure minimum size

                let color = if Some(idx) == self.selected_node {
                    egui::Color32::YELLOW
                } else if node.is_orphan {
                    egui::Color32::from_rgb(255, 107, 107)
                } else {
                    egui::Color32::from_rgb(78, 205, 196)
                };

                painter.circle_filled(pos, node_radius, color);
                visible_count += 1;

                // Draw label for selected or hovered node
                if Some(idx) == self.selected_node {
                    painter.text(
                        pos + egui::vec2(node_radius + 5.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        &node.name,
                        egui::FontId::proportional(12.0),
                        egui::Color32::WHITE,
                    );
                }
            }

            // Handle node selection
            if response.clicked() {
                if let Some(click_pos) = response.interact_pointer_pos() {
                    let world_pos = self.screen_to_world(click_pos, center);
                    let click_radius = 10.0 / self.zoom;

                    let clicked_node = self.graph.nodes.iter().enumerate()
                        .find(|(_, node)| {
                            let dx = node.x - world_pos.x;
                            let dy = node.y - world_pos.y;
                            (dx * dx + dy * dy).sqrt() < click_radius
                        })
                        .map(|(idx, _)| idx);

                    if let Some(idx) = clicked_node {
                        self.selected_node = Some(idx);

                        // Apply ego network filter based on current mode
                        if self.ego_mode != EgoMode::Full {
                            let hops = match self.ego_mode {
                                EgoMode::OneHop => 1,
                                EgoMode::TwoHop => 2,
                                EgoMode::Full => 0,
                            };

                            // Find original node index if we're in filtered view
                            let original_idx = if self.ego_mode == EgoMode::Full {
                                idx
                            } else {
                                // Need to map back to original graph
                                // For now, just use the clicked node in current graph
                                idx
                            };

                            self.extract_ego_network(original_idx, hops);
                            self.recenter_view(rect.size());
                        }
                    } else {
                        self.selected_node = clicked_node;
                    }
                }
            }

            // Draw UI overlay
            let mut reset_view = false;
            let mut mode_changed = false;
            let mut new_mode = self.ego_mode;

            egui::Window::new("🔗 Forge Graph Viewer")
                .default_pos(egui::pos2(10.0, 10.0))
                .show(ctx, |ui| {
                    ui.label(format!("📄 Nodes: {}", self.graph.nodes.len()));
                    ui.label(format!("🔗 Edges: {}", self.graph.edges.len()));
                    ui.label(format!("🔍 Zoom: {:.1}x", self.zoom));

                    if self.simulation_running {
                        ui.label("⚡ Organizing graph...");
                    } else {
                        ui.label("✅ Layout stabilized");
                    }

                    ui.separator();
                    ui.label("🔬 Filter Mode:");
                    ui.horizontal(|ui| {
                        if ui.selectable_label(self.ego_mode == EgoMode::Full, "🌐 Full Graph").clicked() {
                            new_mode = EgoMode::Full;
                            mode_changed = true;
                        }
                        if ui.selectable_label(self.ego_mode == EgoMode::OneHop, "🎯 1-Hop").clicked() {
                            new_mode = EgoMode::OneHop;
                            mode_changed = true;
                        }
                        if ui.selectable_label(self.ego_mode == EgoMode::TwoHop, "🎯🎯 2-Hop").clicked() {
                            new_mode = EgoMode::TwoHop;
                            mode_changed = true;
                        }
                    });

                    ui.separator();
                    if ui.button("🎯 Fit to View").clicked() {
                        reset_view = true;
                    }

                    ui.separator();
                    ui.label("🖱️ Drag to pan");
                    ui.label("🎡 Scroll to zoom");
                    ui.label("🎯 Click node to filter");

                    if let Some(idx) = self.selected_node {
                        ui.separator();
                        ui.label(format!("Selected: {}", self.graph.nodes[idx].name));
                    }

                    if self.ego_mode != EgoMode::Full {
                        ui.separator();
                        ui.colored_label(egui::Color32::LIGHT_BLUE, "🔬 Ego Network Active");
                        ui.label("Click another node to re-filter");
                    }
                });

            if reset_view {
                self.recenter_view(rect.size());
            }

            // Handle mode changes
            if mode_changed {
                self.ego_mode = new_mode;
                if new_mode == EgoMode::Full {
                    self.reset_to_full_graph();
                    self.recenter_view(rect.size());
                } else {
                    // If we have a selected node, apply ego filter
                    if let Some(idx) = self.selected_node {
                        let hops = match new_mode {
                            EgoMode::OneHop => 1,
                            EgoMode::TwoHop => 2,
                            EgoMode::Full => 0,
                        };
                        self.extract_ego_network(idx, hops);
                        self.recenter_view(rect.size());
                    }
                }
            }
        });

        ctx.request_repaint();
    }
}

fn parse_vault(vault_path: &Path, filter_orphans: bool) -> Result<GraphData> {
    let link_regex = Regex::new(r"!?\[\[([^\]]+)\]\]")?;
    let mut notes = HashMap::new();

    // Parse all markdown files
    for entry in WalkDir::new(vault_path)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

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

        let content = fs::read_to_string(path)
            .context(format!("Failed to read: {:?}", path))?;

        let mut links_set = HashSet::new();
        for cap in link_regex.captures_iter(&content) {
            if let Some(link) = cap.get(1) {
                let mut link_str = link.as_str().to_string();

                if link_str.starts_with("linked_media/") {
                    continue;
                }

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

        notes.insert(name.clone(), Note {
            name,
            links: links_set.into_iter().collect(),
        });
    }

    // Find orphans
    let mut incoming_links: HashSet<String> = HashSet::new();
    for note in notes.values() {
        for link in &note.links {
            incoming_links.insert(link.clone());
            incoming_links.insert(format!("{}.md", link));
        }
    }

    let orphans: HashSet<String> = notes
        .keys()
        .filter(|name| !incoming_links.contains(*name))
        .cloned()
        .collect();

    // Filter nodes if requested
    let nodes_to_include: Vec<String> = if filter_orphans {
        notes.keys()
            .filter(|name| !orphans.contains(*name))
            .cloned()
            .collect()
    } else {
        notes.keys().cloned().collect()
    };

    // Create node map and compute initial positions
    let mut node_map = HashMap::new();
    let mut nodes = Vec::new();

    // Much smaller initial radius for dense graphs
    let radius = (nodes_to_include.len() as f32).sqrt() * 5.0;
    for (i, name) in nodes_to_include.iter().enumerate() {
        let angle = (i as f32 / nodes_to_include.len() as f32) * 2.0 * std::f32::consts::PI;
        let x = radius * angle.cos();
        let y = radius * angle.sin();

        node_map.insert(name.clone(), i);
        nodes.push(NodeData {
            name: name.clone(),
            x,
            y,
            is_orphan: orphans.contains(name),
        });
    }

    // Build edges
    let mut edges = Vec::new();
    for note in notes.values() {
        if let Some(&from_idx) = node_map.get(&note.name) {
            for link in &note.links {
                let target_name = if notes.contains_key(link) {
                    link.clone()
                } else if notes.contains_key(&format!("{}.md", link)) {
                    format!("{}.md", link)
                } else {
                    continue;
                };

                if let Some(&to_idx) = node_map.get(&target_name) {
                    edges.push(EdgeData {
                        from: from_idx,
                        to: to_idx,
                    });
                }
            }
        }
    }

    Ok(GraphData {
        nodes,
        edges,
        node_map,
    })
}

fn main() -> eframe::Result {
    let vault_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| {
            eprintln!("Usage: forge-graph-viewer <vault-path> [--filter-orphans]");
            std::process::exit(1);
        });

    let filter_orphans = std::env::args().any(|arg| arg == "--filter-orphans");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_title("Forge Graph Viewer"),
        ..Default::default()
    };

    eframe::run_native(
        "Forge Graph Viewer",
        options,
        Box::new(|_cc| {
            match ForgeGraphViewer::new(Path::new(&vault_path), filter_orphans) {
                Ok(app) => Ok(Box::new(app) as Box<dyn eframe::App>),
                Err(e) => {
                    eprintln!("Error loading vault: {}", e);
                    std::process::exit(1);
                }
            }
        }),
    )
}
