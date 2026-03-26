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

/// Configuration for mind map generation.
pub struct MindMapConfig {
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

/// Height of a single node box.
fn node_height(node: &MmNode, cfg: &MindMapConfig, depth: usize) -> f64 {
    let fs = font_size_at_depth(cfg, depth);
    let lines = node.text.split('\n').count();
    let line_height = fs * 1.4;
    (lines as f64 * line_height + 16.0).max(32.0)
}

/// Width of a single node box.
fn node_width(node: &MmNode, cfg: &MindMapConfig, depth: usize) -> f64 {
    let fs = font_size_at_depth(cfg, depth);
    let max_line = node.text.split('\n').map(|l| builder::estimate_text_width(l, fs)).fold(0.0f64, f64::max);
    (max_line + 28.0).max(60.0)
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

    // Create the node shape
    let elem_id = builder::add_rect(scene, x, y, &node.text, &style, false);

    // Read back actual dimensions (add_rect uses size_for_label which differs from node_width)
    let (actual_w, actual_h) = scene.get(&elem_id)
        .map(|el| (el.width, el.height))
        .unwrap_or((w, h));

    // Layout children using actual element width
    let child_x = x + actual_w + cfg.gap_x;
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
        x, y, width: actual_w, height: actual_h,
        children: child_placed,
    }
}

/// Sample a quadratic Bezier into dense points for freedraw rendering.
fn sample_quadratic_bezier(p0: [f64; 2], p1: [f64; 2], p2: [f64; 2], steps: usize) -> Vec<[f64; 2]> {
    (0..=steps)
        .map(|i| {
            let t = i as f64 / steps as f64;
            let u = 1.0 - t;
            [
                u * u * p0[0] + 2.0 * u * t * p1[0] + t * t * p2[0],
                u * u * p0[1] + 2.0 * u * t * p1[1] + t * t * p2[1],
            ]
        })
        .collect()
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

/// Create curved connectors between parent and children.
fn connect_tree(scene: &mut Scene, placed: &Placed, _cfg: &MindMapConfig) {
    for child in &placed.children {
        // Bezier: parent right edge → control point → child left edge
        let sx = placed.x + placed.width;
        let sy = placed.y + placed.height / 2.0;
        let ex = child.x;
        let ey = child.y + child.height / 2.0;

        // Control point at horizontal midpoint, Y biased toward child
        let cp = [sx + (ex - sx) * 0.5, sy + (ey - sy) * 0.3];

        // Sample the curve densely for freedraw with simulatePressure.
        let abs_points = sample_quadratic_bezier([sx, sy], cp, [ex, ey], 48);
        let rel_points: Vec<[f64; 2]> = abs_points.iter()
            .map(|p| [p[0] - sx, p[1] - sy])
            .collect();

        let conn_id = new_id();
        scene.add(Element {
            id: conn_id.clone(),
            element_type: "freedraw".into(),
            x: sx, y: sy,
            width: ex - sx, height: ey - sy,
            stroke_color: "#999999".into(),
            background_color: "transparent".into(),
            fill_style: "solid".into(),
            stroke_width: 1.0,
            stroke_style: String::new(),
            roughness: 0,
            opacity: 80,
            font_family: 1,
            font_size: 0.0,
            roundness: None,
            label: None,
            bound_elements: None,
            text: None, original_text: None, text_align: None,
            vertical_align: None, container_id: None,
            points: Some(rel_points),
            end_arrowhead: None,
            start_arrowhead: None,
            start_binding: None,
            end_binding: None,
            angle: None, is_deleted: false,
            custom_data: Some(organic_stroke_options()),
            group_ids: None,
            simulate_pressure: Some(true),
        });

        // Recurse
        connect_tree(scene, child, _cfg);
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

    let start_x = 100.0;
    let mut total_height = 0.0;

    // Calculate total height across all roots
    for root in roots {
        total_height += subtree_height(root, cfg, 0);
    }
    total_height += (roots.len() as f64 - 1.0).max(0.0) * cfg.gap_y * 3.0;

    let mut cursor_y = 200.0; // top margin

    for (ri, root) in roots.iter().enumerate() {
        let root_h = subtree_height(root, cfg, 0);
        let root_center = cursor_y + root_h / 2.0;

        let placed = layout_node(root, &mut scene, cfg, 0, start_x, root_center, ri);
        connect_tree(&mut scene, &placed, cfg);

        cursor_y += root_h + cfg.gap_y * 3.0;
    }

    // Z-order: arrows at back, then shapes, then text on top.
    // This ensures shapes paint over arrow origins.
    scene.elements.sort_by_key(|el| match el.element_type.as_str() {
        "arrow" | "freedraw" => 0,
        "line" => 1,
        "rectangle" | "diamond" | "ellipse" => 2,
        "text" => 3,
        _ => 2,
    });

    scene
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
