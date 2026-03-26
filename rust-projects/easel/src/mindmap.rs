use crate::builder;
use crate::elements::*;
use crate::scene::Scene;
use crate::style::Style;

/// A node in the mind map tree.
#[derive(Debug)]
pub struct MmNode {
    pub text: String,
    pub depth: usize,
    pub children: Vec<MmNode>,
}

/// Layout direction for the mind map.
#[derive(Debug, Clone, PartialEq)]
pub enum Layout {
    Right,  // standard left-to-right tree
    Radial, // nodes radiate outward from central root
}

/// Configuration for mind map generation.
pub struct MindMapConfig {
    pub layout: Layout,
    pub gap_x: f64,
    pub gap_y: f64,
    pub root_font_size: f64,
    pub font_scale: f64,     // multiply font size by this per depth level
    pub min_font_size: f64,
    pub multicolor: bool,
}

impl Default for MindMapConfig {
    fn default() -> Self {
        MindMapConfig {
            layout: Layout::Right,
            gap_x: 80.0,
            gap_y: 16.0,
            root_font_size: 24.0,
            font_scale: 0.85,
            min_font_size: 11.0,
            multicolor: false,
        }
    }
}

// ── Parsing ──────────────────────────────────────────────────────────

/// Parse indented Markdown (bullets or plain) into a tree.
/// Supports:
///   - `- Root` / `  - Child` (bullet lists, any indent)
///   - `Root` / `  Child` (plain indented text)
///   - Mixed tabs/spaces (1 tab = 2 spaces)
pub fn parse_markdown(input: &str) -> Vec<MmNode> {
    let mut lines: Vec<(usize, String)> = Vec::new();

    for raw in input.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        // Normalise tabs to 2 spaces for indent counting
        let expanded = raw.replace('\t', "  ");
        let stripped = expanded.trim_start();
        let indent_chars = expanded.len() - stripped.len();

        // Strip leading bullet marker (-, *, +)
        let text = stripped
            .strip_prefix("- ")
            .or_else(|| stripped.strip_prefix("* "))
            .or_else(|| stripped.strip_prefix("+ "))
            .unwrap_or(stripped)
            .trim()
            .to_string();

        if text.is_empty() {
            continue;
        }

        lines.push((indent_chars, text));
    }

    if lines.is_empty() {
        return Vec::new();
    }

    // Determine indent unit from first child (smallest non-zero indent)
    let indent_unit = lines.iter()
        .map(|(indent, _)| *indent)
        .filter(|&i| i > 0)
        .min()
        .unwrap_or(2);

    // Convert raw indent to depth
    let entries: Vec<(usize, String)> = lines.into_iter()
        .map(|(indent, text)| (indent / indent_unit.max(1), text))
        .collect();

    build_tree(&entries, 0, 0).0
}

/// Recursively build tree from flat (depth, text) list.
/// Returns (nodes, next_index).
fn build_tree(entries: &[(usize, String)], start: usize, parent_depth: usize) -> (Vec<MmNode>, usize) {
    let mut nodes = Vec::new();
    let mut i = start;

    while i < entries.len() {
        let (depth, ref text) = entries[i];

        if depth < parent_depth {
            break; // back to parent level
        }
        if depth == parent_depth || (nodes.is_empty() && depth >= parent_depth) {
            let mut node = MmNode {
                text: text.clone(),
                depth,
                children: Vec::new(),
            };
            i += 1;

            // Collect children (deeper indent)
            if i < entries.len() && entries[i].0 > depth {
                let (children, next) = build_tree(entries, i, entries[i].0);
                node.children = children;
                i = next;
            }

            nodes.push(node);
        } else {
            break;
        }
    }

    (nodes, i)
}

// ── Layout ───────────────────────────────────────────────────────────

/// Computed layout position for a node.
struct Placed {
    element_id: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    color_idx: usize,
    children: Vec<Placed>,
}

/// Compute subtree height (total vertical space needed).
fn subtree_height(node: &MmNode, cfg: &MindMapConfig, depth: usize) -> f64 {
    if node.children.is_empty() {
        node_height(node, cfg, depth)
    } else {
        let child_heights: f64 = node.children.iter()
            .map(|c| subtree_height(c, cfg, depth + 1))
            .sum();
        let gaps = (node.children.len() as f64 - 1.0).max(0.0) * cfg.gap_y;
        let children_total = child_heights + gaps;
        children_total.max(node_height(node, cfg, depth))
    }
}

/// Dimensions of a single node box — delegates to builder::size_for_label
/// so layout matches exactly what add_rect/add_ellipse creates.
fn node_size(node: &MmNode, cfg: &MindMapConfig, depth: usize) -> (f64, f64) {
    let fs = font_size_at_depth(cfg, depth);
    let (w, h) = builder::size_for_label(&node.text, fs);
    if depth == 0 {
        (w * 1.4, h * 1.3) // ellipse scaling matches builder::add_ellipse
    } else {
        (w, h)
    }
}

fn node_height(node: &MmNode, cfg: &MindMapConfig, depth: usize) -> f64 {
    node_size(node, cfg, depth).1
}

fn node_width(node: &MmNode, cfg: &MindMapConfig, depth: usize) -> f64 {
    node_size(node, cfg, depth).0
}

/// Font size at a given depth.
fn font_size_at_depth(cfg: &MindMapConfig, depth: usize) -> f64 {
    (cfg.root_font_size * cfg.font_scale.powi(depth as i32)).max(cfg.min_font_size)
}

/// Recursively lay out a right-facing tree.
/// `x` is the left edge of this node, `y_center` is the vertical center.
fn layout_node(
    node: &MmNode,
    scene: &mut Scene,
    cfg: &MindMapConfig,
    depth: usize,
    x: f64,
    y_center: f64,
    color_idx: usize,
) -> Placed {
    let fs = font_size_at_depth(cfg, depth);
    let w = node_width(node, cfg, depth);
    let h = node_height(node, cfg, depth);
    let y = y_center - h / 2.0;

    // Pick style
    let style = if cfg.multicolor && depth <= 1 {
        branch_color(color_idx, fs)
    } else if depth == 0 {
        root_style(fs)
    } else {
        node_style(depth, fs)
    };

    // Root node is an ellipse, others are rectangles
    let elem_id = if depth == 0 {
        builder::add_ellipse(scene, x, y, &node.text, &style, false)
    } else {
        builder::add_rect(scene, x, y, &node.text, &style, false)
    };

    // Layout children
    let child_x = x + w + cfg.gap_x;
    let mut child_placed = Vec::new();

    if !node.children.is_empty() {
        let total_h = subtree_height(node, cfg, depth);
        let mut cursor_y = y_center - total_h / 2.0;

        for (ci, child) in node.children.iter().enumerate() {
            let child_h = subtree_height(child, cfg, depth + 1);
            let child_center = cursor_y + child_h / 2.0;

            let branch_color = if depth == 0 { ci } else { color_idx };
            let placed = layout_node(child, scene, cfg, depth + 1, child_x, child_center, branch_color);
            child_placed.push(placed);

            cursor_y += child_h + cfg.gap_y;
        }
    }

    Placed {
        element_id: elem_id,
        x, y, width: w, height: h,
        color_idx,
        children: child_placed,
    }
}

// ── Radial Layout ────────────────────────────────────────────────────

/// Lay out nodes in a radial pattern: root at center, L1 around it, L2 fanning outward.
fn layout_radial(
    root: &MmNode,
    scene: &mut Scene,
    cfg: &MindMapConfig,
    center_x: f64,
    center_y: f64,
    root_color_idx: usize,
) -> Placed {
    let fs = font_size_at_depth(cfg, 0);
    let (w, h) = node_size(root, cfg, 0);

    // Root at center
    let root_style = root_style(fs);
    let root_id = builder::add_ellipse(scene, center_x - w / 2.0, center_y - h / 2.0,
                                        &root.text, &root_style, false);

    if root.children.is_empty() {
        return Placed {
            element_id: root_id, x: center_x - w / 2.0, y: center_y - h / 2.0,
            width: w, height: h, color_idx: root_color_idx, children: Vec::new(),
        };
    }

    let n = root.children.len();
    // Radius from center to L1 nodes — proportional to root size + gap
    let l1_radius = (w + h) / 2.0 + cfg.gap_x * 1.5;

    // Compute angular span for each child proportional to its subtree size
    let subtree_sizes: Vec<f64> = root.children.iter()
        .map(|c| subtree_height(c, cfg, 1))
        .collect();
    let total_size: f64 = subtree_sizes.iter().sum();

    // Start angle: top-right, sweep clockwise
    let start_angle = -std::f64::consts::FRAC_PI_2; // -90 degrees (top)
    let sweep = std::f64::consts::PI * 2.0;

    let mut child_placed = Vec::new();
    let mut angle_cursor = start_angle;

    for (ci, child) in root.children.iter().enumerate() {
        let angular_span = sweep * (subtree_sizes[ci] / total_size);
        let child_angle = angle_cursor + angular_span / 2.0;
        angle_cursor += angular_span;

        let child_cx = center_x + l1_radius * child_angle.cos();
        let child_cy = center_y + l1_radius * child_angle.sin();

        let placed = layout_radial_subtree(child, scene, cfg, 1, child_cx, child_cy,
                                            child_angle, l1_radius, ci);
        child_placed.push(placed);
    }

    Placed {
        element_id: root_id,
        x: center_x - w / 2.0, y: center_y - h / 2.0,
        width: w, height: h,
        color_idx: root_color_idx,
        children: child_placed,
    }
}

/// Lay out a subtree node in the radial layout, fanning children outward.
fn layout_radial_subtree(
    node: &MmNode,
    scene: &mut Scene,
    cfg: &MindMapConfig,
    depth: usize,
    cx: f64,
    cy: f64,
    parent_angle: f64, // angle from center to this node
    parent_radius: f64,
    color_idx: usize,
) -> Placed {
    let fs = font_size_at_depth(cfg, depth);
    let (w, h) = node_size(node, cfg, depth);

    let style = if cfg.multicolor && depth <= 1 {
        branch_color(color_idx, fs)
    } else {
        node_style(depth, fs)
    };

    let elem_id = builder::add_rect(scene, cx - w / 2.0, cy - h / 2.0,
                                     &node.text, &style, false);

    let mut child_placed = Vec::new();

    if !node.children.is_empty() {
        let n = node.children.len();
        let child_radius = cfg.gap_x * 1.2;

        // Fan children in an arc centered on the parent angle
        let fan_spread = (n as f64 * 0.4).min(std::f64::consts::PI * 0.8);
        let fan_start = parent_angle - fan_spread / 2.0;
        let angle_step = if n > 1 { fan_spread / (n - 1) as f64 } else { 0.0 };

        for (ci, child) in node.children.iter().enumerate() {
            let child_angle = if n == 1 { parent_angle } else { fan_start + ci as f64 * angle_step };
            let child_cx = cx + child_radius * child_angle.cos();
            let child_cy = cy + child_radius * child_angle.sin();

            let placed = layout_radial_subtree(child, scene, cfg, depth + 1,
                                                child_cx, child_cy,
                                                child_angle, child_radius, color_idx);
            child_placed.push(placed);
        }
    }

    Placed {
        element_id: elem_id,
        x: cx - w / 2.0, y: cy - h / 2.0,
        width: w, height: h,
        color_idx,
        children: child_placed,
    }
}

/// Connect radial tree: straight lines from center of parent to center of child.
fn connect_radial(scene: &mut Scene, placed: &Placed, cfg: &MindMapConfig, depth: usize) {
    for child in &placed.children {
        let sx = placed.x + placed.width / 2.0;
        let sy = placed.y + placed.height / 2.0;
        let ex = child.x + child.width / 2.0;
        let ey = child.y + child.height / 2.0;

        // Direction from parent center to child center
        let dx = ex - sx;
        let dy = ey - sy;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist < 1.0 { continue; }

        // Start from parent edge, end at child edge
        let nx = dx / dist;
        let ny = dy / dist;
        let start_x = sx + nx * (placed.width / 2.0).min(placed.height / 2.0);
        let start_y = sy + ny * (placed.width / 2.0).min(placed.height / 2.0);
        let end_x = ex - nx * (child.width / 2.0).min(child.height / 2.0);
        let end_y = ey - ny * (child.width / 2.0).min(child.height / 2.0);

        // Cubic Bezier for smooth curve
        let gap = ((end_x - start_x).powi(2) + (end_y - start_y).powi(2)).sqrt();
        let cp_dist = gap * 0.35;
        let rel_points = vec![
            [0.0, 0.0],
            [nx * cp_dist, ny * cp_dist],
            [end_x - start_x - nx * cp_dist, end_y - start_y - ny * cp_dist],
            [end_x - start_x, end_y - start_y],
        ];

        let color = connector_color(cfg, child.color_idx);
        let (start_size, end_size) = branch_sizes(depth);

        let conn_id = new_id();
        scene.add(Element {
            id: conn_id.clone(),
            element_type: "arrow".into(),
            x: start_x, y: start_y,
            width: end_x - start_x, height: end_y - start_y,
            stroke_color: color,
            background_color: "transparent".into(),
            fill_style: "solid".into(),
            stroke_width: 2.0,
            stroke_style: String::new(),
            roughness: 0,
            opacity: 80,
            font_family: 1,
            font_size: 0.0,
            roundness: Some(Roundness { roundness_type: 2 }),
            label: None,
            bound_elements: None,
            text: None, original_text: None, text_align: None,
            vertical_align: None, container_id: None,
            points: Some(rel_points),
            end_arrowhead: None,
            start_arrowhead: None,
            start_binding: Some(Binding {
                element_id: placed.element_id.clone(),
                fixed_point: [0.5, 0.5],
                focus: 0.0, gap: 1.0,
            }),
            end_binding: Some(Binding {
                element_id: child.element_id.clone(),
                fixed_point: [0.5, 0.5],
                focus: 0.0, gap: 1.0,
            }),
            angle: None, is_deleted: false,
            custom_data: Some(serde_json::json!({
                "strokeOptions": {
                    "organic": true,
                    "startSize": start_size,
                    "endSize": end_size,
                    "depth": depth
                }
            })),
            group_ids: None,
            simulate_pressure: None,
        });

        if let Some(el) = scene.get_mut(&placed.element_id) {
            el.bound_elements.get_or_insert_with(Vec::new)
                .push(BoundElement { id: conn_id.clone(), bound_type: "arrow".into() });
        }
        if let Some(el) = scene.get_mut(&child.element_id) {
            el.bound_elements.get_or_insert_with(Vec::new)
                .push(BoundElement { id: conn_id.clone(), bound_type: "arrow".into() });
        }

        connect_radial(scene, child, cfg, depth + 1);
    }
}

// ── Bezier helpers ───────────────────────────────────────────────────

/// Sample a cubic Bezier (4 control points) into dense points.
fn sample_cubic_bezier(p0: [f64; 2], p1: [f64; 2], p2: [f64; 2], p3: [f64; 2], steps: usize) -> Vec<[f64; 2]> {
    (0..=steps)
        .map(|i| {
            let t = i as f64 / steps as f64;
            let u = 1.0 - t;
            let u2 = u * u;
            let t2 = t * t;
            [
                u2 * u * p0[0] + 3.0 * u2 * t * p1[0] + 3.0 * u * t2 * p2[0] + t2 * t * p3[0],
                u2 * u * p0[1] + 3.0 * u2 * t * p1[1] + 3.0 * u * t2 * p2[1] + t2 * t * p3[1],
            ]
        })
        .collect()
}

/// Branch width at a given depth. Returns (start_size, end_size) for the freehand stroke.
/// End size of depth N roughly matches start size of depth N+1 for visual continuity.
fn branch_sizes(depth: usize) -> (f64, f64) {
    match depth {
        0 => (14.0, 9.0),   // root→L1: thick, tapers to ~L2 start
        1 => (9.0, 6.0),    // L1→L2: medium, tapers to ~L3 start
        2 => (6.0, 3.5),    // L2→L3: thin
        _ => (3.5, 2.0),    // deeper: fine
    }
}

/// Organic stroke options for perfect-freehand (thick at start, tapers at end).
fn organic_stroke_options() -> serde_json::Value {
    serde_json::json!({
        "strokeOptions": {
            "thinning": 0.6,
            "smoothing": 0.5,
            "streamline": 0.5,
            "start": { "taper": false },
            "end": { "taper": true },
            "easing": "linear",
            "simulatePressure": false
        }
    })
}

/// Connector colour: uses branch colour in multicolor mode, else gray.
fn connector_color(cfg: &MindMapConfig, color_idx: usize) -> String {
    if cfg.multicolor {
        let palettes = [
            "#1a73e8", "#d93025", "#188038", "#e37400",
            "#8430ce", "#00838f", "#c2185b", "#e65100",
        ];
        palettes[color_idx % palettes.len()].into()
    } else {
        "#aaaaaa".into()
    }
}

/// Create curved arrow connectors with proper Excalidraw bindings.
/// `depth` is the parent's depth (0=root, 1=L1, etc.).
fn connect_tree(scene: &mut Scene, placed: &Placed, cfg: &MindMapConfig, depth: usize) {
    for child in &placed.children {
        // Arrow: parent right edge → child left edge
        let sx = placed.x + placed.width;
        let sy = placed.y + placed.height / 2.0;
        let ex = child.x;
        let ey = child.y + child.height / 2.0;
        let gap = ex - sx;

        // Cubic Bezier S-curve: departs horizontally, arrives horizontally
        let cp1 = [sx + gap * 0.4, sy];
        let cp2 = [ex - gap * 0.4, ey];
        let sampled = sample_cubic_bezier([sx, sy], cp1, cp2, [ex, ey], 48);

        // Store as 4-point arrow for Excalidraw (start, cp1, cp2, end — relative to start)
        let rel_points = vec![
            [0.0, 0.0],
            [gap * 0.4, 0.0],
            [gap * 0.6, ey - sy],
            [ex - sx, ey - sy],
        ];

        // Connector inherits the child's branch colour
        let color = connector_color(cfg, child.color_idx);

        // Depth-based sizing stored in customData for SVG renderer
        let (start_size, end_size) = branch_sizes(depth);

        let conn_id = new_id();
        scene.add(Element {
            id: conn_id.clone(),
            element_type: "arrow".into(),
            x: sx, y: sy,
            width: ex - sx, height: ey - sy,
            stroke_color: color,
            background_color: "transparent".into(),
            fill_style: "solid".into(),
            stroke_width: 2.0,
            stroke_style: String::new(),
            roughness: 0,
            opacity: 80,
            font_family: 1,
            font_size: 0.0,
            roundness: Some(Roundness { roundness_type: 2 }),
            label: None,
            bound_elements: None,
            text: None, original_text: None, text_align: None,
            vertical_align: None, container_id: None,
            points: Some(rel_points),
            end_arrowhead: None,
            start_arrowhead: None,
            start_binding: Some(Binding {
                element_id: placed.element_id.clone(),
                fixed_point: [1.0, 0.5],
                focus: 0.0, gap: 1.0,
            }),
            end_binding: Some(Binding {
                element_id: child.element_id.clone(),
                fixed_point: [0.0, 0.5],
                focus: 0.0, gap: 1.0,
            }),
            angle: None, is_deleted: false,
            custom_data: Some(serde_json::json!({
                "strokeOptions": {
                    "organic": true,
                    "startSize": start_size,
                    "endSize": end_size,
                    "depth": depth
                }
            })),
            group_ids: None,
            simulate_pressure: None,
        });

        // Register bindings on parent and child shapes
        if let Some(el) = scene.get_mut(&placed.element_id) {
            el.bound_elements.get_or_insert_with(Vec::new)
                .push(BoundElement { id: conn_id.clone(), bound_type: "arrow".into() });
        }
        if let Some(el) = scene.get_mut(&child.element_id) {
            el.bound_elements.get_or_insert_with(Vec::new)
                .push(BoundElement { id: conn_id.clone(), bound_type: "arrow".into() });
        }

        // Recurse
        connect_tree(scene, child, cfg, depth + 1);
    }
}

// ── Styles ───────────────────────────────────────────────────────────

fn root_style(font_size: f64) -> Style {
    Style {
        fill: "#e8f4f8".into(),
        stroke: "#2c3e50".into(),
        text_color: "#2c3e50".into(),
        opacity: 100,
        font_size,
    }
}

fn node_style(_depth: usize, font_size: f64) -> Style {
    Style {
        fill: "#f7f9fc".into(),
        stroke: "#7f8c8d".into(),
        text_color: "#2c3e50".into(),
        opacity: 100,
        font_size,
    }
}

/// Branch colours for multicolor mode (Zsolt-inspired distinct hues).
fn branch_color(index: usize, font_size: f64) -> Style {
    let palettes = [
        ("#e8f0fe", "#1a73e8"), // blue
        ("#fce8e6", "#d93025"), // red
        ("#e6f4ea", "#188038"), // green
        ("#fef7e0", "#e37400"), // amber
        ("#f3e8fd", "#8430ce"), // purple
        ("#e0f7fa", "#00838f"), // teal
        ("#fce4ec", "#c2185b"), // pink
        ("#fff3e0", "#e65100"), // deep orange
    ];
    let (fill, stroke) = palettes[index % palettes.len()];
    Style {
        fill: fill.into(),
        stroke: stroke.into(),
        text_color: stroke.into(),
        opacity: 100,
        font_size,
    }
}

// ── Public API ───────────────────────────────────────────────────────

/// Generate a mind map from parsed nodes.
pub fn generate(roots: &[MmNode], cfg: &MindMapConfig) -> Scene {
    let mut scene = Scene::new();

    match cfg.layout {
        Layout::Right => generate_right(&mut scene, roots, cfg),
        Layout::Radial => generate_radial(&mut scene, roots, cfg),
    }

    // Z-order: arrows at back, then shapes, then text on top.
    scene.elements.sort_by_key(|el| match el.element_type.as_str() {
        "arrow" | "freedraw" => 0,
        "line" => 1,
        "rectangle" | "diamond" | "ellipse" => 2,
        "text" => 3,
        _ => 2,
    });

    scene
}

fn generate_right(scene: &mut Scene, roots: &[MmNode], cfg: &MindMapConfig) {
    let start_x = 100.0;
    let mut cursor_y = 200.0;

    for (ri, root) in roots.iter().enumerate() {
        let root_h = subtree_height(root, cfg, 0);
        let root_center = cursor_y + root_h / 2.0;

        let placed = layout_node(root, scene, cfg, 0, start_x, root_center, ri);
        connect_tree(scene, &placed, cfg, 0);

        cursor_y += root_h + cfg.gap_y * 3.0;
    }
}

fn generate_radial(scene: &mut Scene, roots: &[MmNode], cfg: &MindMapConfig) {
    let center_x = 600.0;
    let center_y = 600.0;

    for (ri, root) in roots.iter().enumerate() {
        let cy = center_y + ri as f64 * 800.0;
        let placed = layout_radial(root, scene, cfg, center_x, cy, ri);
        connect_radial(scene, &placed, cfg, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bullets() {
        let input = "- Root\n  - Child 1\n    - Grandchild\n  - Child 2\n";
        let nodes = parse_markdown(input);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].text, "Root");
        assert_eq!(nodes[0].children.len(), 2);
        assert_eq!(nodes[0].children[0].text, "Child 1");
        assert_eq!(nodes[0].children[0].children.len(), 1);
        assert_eq!(nodes[0].children[1].text, "Child 2");
    }

    #[test]
    fn parse_plain_indent() {
        let input = "Root\n  Child 1\n  Child 2\n    Grandchild\n";
        let nodes = parse_markdown(input);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].children.len(), 2);
        assert_eq!(nodes[0].children[1].children.len(), 1);
    }

    #[test]
    fn parse_tabs() {
        let input = "- Root\n\t- Child\n\t\t- Grand\n";
        let nodes = parse_markdown(input);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].children.len(), 1);
        assert_eq!(nodes[0].children[0].children.len(), 1);
    }

    #[test]
    fn generate_produces_elements() {
        let nodes = parse_markdown("- Root\n  - A\n  - B\n");
        let cfg = MindMapConfig::default();
        let scene = generate(&nodes, &cfg);
        // Root (rect+text) + A (rect+text) + B (rect+text) + 2 freedraw connectors = 8
        assert_eq!(scene.elements.len(), 8);
    }
}
