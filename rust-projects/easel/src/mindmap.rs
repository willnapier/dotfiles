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
    Buzan,  // radial with text ON branches, no boxes
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

/// Clamp a branch angle so it's never steeper than max_tilt from horizontal.
fn clamp_to_readable(angle: f64, max_tilt: f64) -> f64 {
    // Normalize to [-PI, PI]
    let mut a = angle;
    while a > std::f64::consts::PI { a -= std::f64::consts::PI * 2.0; }
    while a < -std::f64::consts::PI { a += std::f64::consts::PI * 2.0; }

    // Compute tilt from horizontal (0° or 180°)
    let tilt = if a.abs() <= std::f64::consts::FRAC_PI_2 {
        a.abs() // right side: tilt from 0°
    } else {
        std::f64::consts::PI - a.abs() // left side: tilt from 180°
    };

    if tilt <= max_tilt {
        return a; // already within limit
    }

    // Clamp: push toward nearest horizontal
    if a >= 0.0 && a <= std::f64::consts::FRAC_PI_2 {
        max_tilt // upper right → clamp to +max_tilt
    } else if a > std::f64::consts::FRAC_PI_2 {
        std::f64::consts::PI - max_tilt // upper left → clamp to PI-max_tilt
    } else if a < -std::f64::consts::FRAC_PI_2 {
        -(std::f64::consts::PI - max_tilt) // lower left → clamp to -(PI-max_tilt)
    } else {
        -max_tilt // lower right → clamp to -max_tilt
    }
}

// ── Radial Layout (Buzan principles) ─────────────────────────────────
//
// Key ratios:
//   L1 distance from center: 2.5× root radius
//   L2 distance from L1: 0.65× L1 distance
//   L3 distance from L2: 0.65× L2 distance
//   Angular allocation: proportional to subtree weight, min 20°, max 120°
//   Sub-branch fan: 55% of parent's angular sector
//   Start angle: 1 o'clock (clockwise, like reading a clock)

const MIN_ANGLE_DEG: f64 = 25.0;
const MAX_ANGLE_DEG: f64 = 120.0;
const DISTANCE_DECAY: f64 = 0.85;
const FAN_RATIO: f64 = 0.85;

/// Count total descendants (for angular allocation weighting).
fn subtree_weight(node: &MmNode) -> f64 {
    if node.children.is_empty() {
        1.0
    } else {
        1.0 + node.children.iter().map(subtree_weight).sum::<f64>()
    }
}

/// Lay out nodes in a radial pattern following Buzan mind map principles.
fn layout_radial(
    root: &MmNode,
    scene: &mut Scene,
    cfg: &MindMapConfig,
    center_x: f64,
    center_y: f64,
    _root_color_idx: usize,
) -> Placed {
    let fs = font_size_at_depth(cfg, 0);
    let (w, h) = node_size(root, cfg, 0);

    // Root at center (ellipse)
    let style = root_style(fs);
    let root_id = builder::add_ellipse(scene, center_x - w / 2.0, center_y - h / 2.0,
                                        &root.text, &style, false);

    if root.children.is_empty() {
        return Placed {
            element_id: root_id, x: center_x - w / 2.0, y: center_y - h / 2.0,
            width: w, height: h, color_idx: 0, children: Vec::new(),
        };
    }

    // L1 radius: 2.5× average root radius
    let root_r = (w + h) / 4.0; // average radius of ellipse
    let l1_distance = root_r * 1.5 + cfg.gap_x;

    // Compute angular spans proportional to subtree weight
    let weights: Vec<f64> = root.children.iter().map(subtree_weight).collect();
    let total_weight: f64 = weights.iter().sum();
    let n = root.children.len();

    let min_angle = MIN_ANGLE_DEG.to_radians();
    let max_angle = MAX_ANGLE_DEG.to_radians();
    let full_circle = std::f64::consts::PI * 2.0;

    // Raw proportional angles, then clamp
    let mut angles: Vec<f64> = weights.iter()
        .map(|w| (full_circle * w / total_weight).clamp(min_angle, max_angle))
        .collect();

    // Normalize so they sum to full circle
    let angle_sum: f64 = angles.iter().sum();
    for a in &mut angles {
        *a *= full_circle / angle_sum;
    }

    // X pattern: center first branch at top-right diagonal (-45°)
    // Offset start so the first branch's CENTER lands at -45°
    let start_angle = -std::f64::consts::FRAC_PI_4 - angles[0] / 2.0;
    let mut angle_cursor = start_angle;
    let mut child_placed = Vec::new();

    for (ci, child) in root.children.iter().enumerate() {
        let raw_angle = angle_cursor + angles[ci] / 2.0;
        // Slight horizontal pull on L1 too (15% toward horizontal)
        let horiz = if raw_angle.cos() >= 0.0 { 0.0 } else { std::f64::consts::PI };
        let mut src = raw_angle;
        let mut tgt = horiz;
        while (tgt - src).abs() > std::f64::consts::PI {
            if tgt > src { src += std::f64::consts::PI * 2.0; } else { tgt += std::f64::consts::PI * 2.0; }
        }
        let child_angle = src + (tgt - src) * 0.15;
        let child_cx = center_x + l1_distance * child_angle.cos();
        let child_cy = center_y + l1_distance * child_angle.sin();

        let placed = layout_radial_subtree(
            child, scene, cfg, 1,
            child_cx, child_cy,
            child_angle, angles[ci],
            l1_distance, ci,
        );
        child_placed.push(placed);

        angle_cursor += angles[ci];
    }

    Placed {
        element_id: root_id,
        x: center_x - w / 2.0, y: center_y - h / 2.0,
        width: w, height: h,
        color_idx: 0,
        children: child_placed,
    }
}

/// Lay out a subtree node, fanning children outward within the allocated angular sector.
fn layout_radial_subtree(
    node: &MmNode,
    scene: &mut Scene,
    cfg: &MindMapConfig,
    depth: usize,
    cx: f64,
    cy: f64,
    my_angle: f64,          // radial angle from map center to this node
    my_angular_sector: f64, // how much angular space this branch owns
    parent_distance: f64,   // distance from parent to this node
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
        // Distance decays per level
        let child_distance = parent_distance * DISTANCE_DECAY;
        // Sub-branches fan within a portion of parent's sector
        let fan_sector = my_angular_sector * FAN_RATIO;

        // Proportional allocation within fan sector
        let weights: Vec<f64> = node.children.iter().map(subtree_weight).collect();
        let total_w: f64 = weights.iter().sum();

        let mut child_angles: Vec<f64> = weights.iter()
            .map(|w| fan_sector * w / total_w)
            .collect();

        // Start fan centered on parent angle
        let fan_start = my_angle - fan_sector / 2.0;
        let mut cursor = fan_start;

        for (ci, child) in node.children.iter().enumerate() {
            let raw_angle = cursor + child_angles[ci] / 2.0;

            // Bend toward horizontal at deeper levels:
            // Right side → angle trends toward 0, Left side → trends toward PI
            let horizontal = if raw_angle.cos() >= 0.0 { 0.0 } else { std::f64::consts::PI };
            // Normalize angles for interpolation
            let mut target = horizontal;
            let mut source = raw_angle;
            // Handle wrap-around: ensure we interpolate the short way
            while (target - source).abs() > std::f64::consts::PI {
                if target > source { source += std::f64::consts::PI * 2.0; }
                else { target += std::f64::consts::PI * 2.0; }
            }
            let blend = match depth {
                1 => 0.35, // L2: moderate horizontal pull
                2 => 0.55, // L3: strong horizontal pull
                _ => 0.65, // deeper: mostly horizontal
            };
            let child_angle = source + (target - source) * blend;

            let (cw, ch) = node_size(child, cfg, depth + 1);

            // Minimum distance: branch must be visible beyond both node labels
            // Use width (not height) since branches trend horizontal
            let min_visible_branch = 80.0; // px of visible branch
            let min_distance = (w + cw) / 2.0 + min_visible_branch;

            // Push outward if siblings would overlap, but cap at 1.5× base
            let min_arc_gap = ch + 16.0;
            let arc_at_distance = child_distance * child_angles[ci].max(0.1);
            let collision_distance = if arc_at_distance < min_arc_gap && n > 1 {
                (child_distance * (min_arc_gap / arc_at_distance)).min(child_distance * 1.5)
            } else {
                child_distance
            };

            let actual_distance = collision_distance.max(min_distance);

            let child_cx = cx + actual_distance * child_angle.cos();
            let child_cy = cy + actual_distance * child_angle.sin();

            let placed = layout_radial_subtree(
                child, scene, cfg, depth + 1,
                child_cx, child_cy,
                child_angle, child_angles[ci],
                actual_distance, color_idx,
            );
            child_placed.push(placed);

            cursor += child_angles[ci];
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

        // Cubic Bezier: depart in radial direction, arrive horizontally.
        // CP1: follows the radial direction (diagonal departure)
        // CP2: approaches the child horizontally (from the root side)
        let gap = ((end_x - start_x).powi(2) + (end_y - start_y).powi(2)).sqrt();
        let cp1_dist = gap * 0.4;
        let cp2_dist = gap * 0.4;
        // CP1: radial direction from parent
        let cp1_x = nx * cp1_dist;
        let cp1_y = ny * cp1_dist;
        // CP2: horizontal approach to child (from the side closest to parent)
        let horiz_dir = if ex > sx { -1.0 } else { 1.0 }; // approach from root's side
        let cp2_x = (end_x - start_x) + horiz_dir * cp2_dist;
        let cp2_y = end_y - start_y; // same Y as endpoint (horizontal)
        let rel_points = vec![
            [0.0, 0.0],
            [cp1_x, cp1_y],
            [cp2_x, cp2_y],
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

// ── Buzan Layout (text on branches, no boxes) ───────────────────────

/// Buzan layout: radial positions but text placed directly on branches.
fn generate_buzan(scene: &mut Scene, roots: &[MmNode], cfg: &MindMapConfig) {
    let center_x = 600.0;
    let center_y = 600.0;

    for (ri, root) in roots.iter().enumerate() {
        let cy = center_y + ri as f64 * 800.0;
        let placed = layout_buzan_root(root, scene, cfg, center_x, cy, ri);
        // Connectors are created during layout (branch = connector + text)
        // No separate connect pass needed
        let _ = placed;
    }
}

/// Buzan root: ellipse at center, then radial branches with text on them.
fn layout_buzan_root(
    root: &MmNode,
    scene: &mut Scene,
    cfg: &MindMapConfig,
    center_x: f64,
    center_y: f64,
    _root_color_idx: usize,
) -> Placed {
    let fs = font_size_at_depth(cfg, 0);
    let (w, h) = node_size(root, cfg, 0);

    // Root still gets an ellipse (Buzan allows a central image/shape)
    let style = root_style(fs);
    let root_id = builder::add_ellipse(scene, center_x - w / 2.0, center_y - h / 2.0,
                                        &root.text, &style, false);

    if root.children.is_empty() {
        return Placed {
            element_id: root_id, x: center_x - w / 2.0, y: center_y - h / 2.0,
            width: w, height: h, color_idx: 0, children: Vec::new(),
        };
    }

    // Split branches between right and left sides, distribute within ±45° range.
    let root_r = (w + h) / 4.0;
    let n = root.children.len();
    let max_tilt = std::f64::consts::FRAC_PI_4;

    // Split: alternate right/left for balance (odd extras go right)
    let n_right = (n + 1) / 2;
    let n_left = n - n_right;

    // Compute angles: right side = [-max_tilt, +max_tilt], left = [PI-max_tilt, PI+max_tilt]
    let mut child_angles: Vec<f64> = Vec::with_capacity(n);
    // Right side: indices 0, 2, 4, ... → evenly within [-max_tilt, +max_tilt]
    // Left side: indices 1, 3, 5, ... → evenly within [PI-max_tilt, PI+max_tilt]
    let right_angles: Vec<f64> = if n_right == 1 {
        vec![0.0] // single right branch → horizontal
    } else {
        (0..n_right).map(|i| -max_tilt + (2.0 * max_tilt) * i as f64 / (n_right - 1) as f64).collect()
    };
    let left_angles: Vec<f64> = if n_left == 0 {
        vec![]
    } else if n_left == 1 {
        vec![std::f64::consts::PI]
    } else {
        (0..n_left).map(|i| {
            (std::f64::consts::PI - max_tilt) + (2.0 * max_tilt) * i as f64 / (n_left - 1) as f64
        }).collect()
    };

    // Interleave: child 0→right[0], child 1→left[0], child 2→right[1], etc.
    let mut ri = 0;
    let mut li = 0;
    for ci in 0..n {
        if ci % 2 == 0 && ri < n_right {
            child_angles.push(right_angles[ri]);
            ri += 1;
        } else if li < n_left {
            child_angles.push(left_angles[li]);
            li += 1;
        } else {
            child_angles.push(right_angles[ri]);
            ri += 1;
        }
    }

    let mut child_placed = Vec::new();

    for (ci, child) in root.children.iter().enumerate() {
        let child_angle = child_angles[ci];

        // Branch length based on text width + visible margins
        let child_fs = font_size_at_depth(cfg, 1);
        let text_width = builder::estimate_text_width(&child.text, child_fs);
        // Symmetric gap on both sides of text
        let text_margin = (child_fs * 2.0).min(40.0);
        let branch_distance = text_margin + text_width + text_margin;

        // Start from ellipse edge, end along child_angle direction FROM start
        let rx = w / 2.0;
        let ry = h / 2.0;
        let overlap = 8.0;
        let start_x = center_x + (rx - overlap) * child_angle.cos();
        let start_y = center_y + (ry - overlap) * child_angle.sin();
        let end_x = start_x + branch_distance * child_angle.cos();
        let end_y = start_y + branch_distance * child_angle.sin();

        // Text midpoint along the branch (after root edge + gap)
        let text_mid_dist = root_r + 40.0 + text_width / 2.0;
        let text_cx = center_x + text_mid_dist * child_angle.cos();
        let text_cy = center_y + text_mid_dist * child_angle.sin();

        // Text angle: follow the branch but keep readable (not upside down)
        let mut text_angle = child_angle;
        if text_angle.cos() < 0.0 {
            text_angle += std::f64::consts::PI; // flip so text reads left-to-right
        }

        // Color
        let color = if cfg.multicolor {
            let palettes = ["#1a73e8", "#d93025", "#188038", "#e37400", "#8430ce", "#00838f", "#c2185b", "#e65100"];
            palettes[ci % palettes.len()]
        } else { "#2c3e50" };

        // Create the branch — start slightly inside the ellipse so the organic
        // stroke overlaps the edge and blends seamlessly
        // Arrow geometry
        let dx = end_x - start_x;
        let dy = end_y - start_y;
        let nx = child_angle.cos();
        let ny = child_angle.sin();
        let gap = (dx * dx + dy * dy).sqrt();
        let cp_dist = gap * 0.4;
        let horiz_dir = if end_x > start_x { -1.0 } else { 1.0 };

        let (start_size, end_size) = branch_sizes(0);
        let conn_id = new_id();
        scene.add(Element {
            id: conn_id.clone(),
            element_type: "arrow".into(),
            x: start_x, y: start_y,
            width: dx, height: dy,
            stroke_color: color.into(),
            background_color: "transparent".into(),
            fill_style: "solid".into(),
            stroke_width: 2.0,
            stroke_style: String::new(),
            roughness: 0,
            opacity: 80,
            font_family: 1, font_size: 0.0,
            roundness: Some(Roundness { roundness_type: 2 }),
            label: None, bound_elements: None,
            text: None, original_text: None, text_align: None,
            vertical_align: None, container_id: None,
            points: Some(vec![
                [0.0, 0.0],
                [nx * cp_dist, ny * cp_dist],
                [dx + horiz_dir * cp_dist, dy],
                [dx, dy],
            ]),
            end_arrowhead: None, start_arrowhead: None,
            start_binding: Some(Binding {
                element_id: root_id.clone(),
                fixed_point: [0.5, 0.5],
                focus: 0.0, gap: 1.0,
            }),
            end_binding: None,
            angle: None, is_deleted: false,
            custom_data: Some(serde_json::json!({
                "strokeOptions": { "organic": true, "startSize": start_size, "endSize": end_size, "depth": 0 }
            })),
            group_ids: None, simulate_pressure: None,
        });

        // Place text ON the branch (rotated, no box)
        let text_id = new_id();
        let text_h = child_fs * 1.2;
        scene.add(Element {
            id: text_id.clone(),
            element_type: "text".into(),
            x: text_cx - text_width / 2.0,
            y: text_cy - text_h / 2.0,
            width: text_width, height: text_h,
            stroke_color: color.into(),
            background_color: "transparent".into(),
            fill_style: "solid".into(),
            stroke_width: 0.0, stroke_style: String::new(),
            roughness: 0, opacity: 100,
            font_family: 2, font_size: child_fs,
            roundness: None, label: None, bound_elements: None,
            text: Some(child.text.clone()),
            original_text: Some(child.text.clone()),
            text_align: Some("center".into()),
            vertical_align: Some("middle".into()),
            container_id: None,
            points: None, end_arrowhead: None, start_arrowhead: None,
            start_binding: None, end_binding: None,
            angle: Some(text_angle),
            is_deleted: false,
            custom_data: Some(serde_json::json!({ "onBranch": conn_id })),
            group_ids: None, simulate_pressure: None,
        });

        // Now layout L2 children branching from the endpoint
        let mut l2_placed = Vec::new();
        if !child.children.is_empty() {
            let n2 = child.children.len();
            let l2_fs = font_size_at_depth(cfg, 2);

            // L2 fan: ±35° from horizontal (leaves 10° buffer between adjacent L1 fans)
            let l2_max_tilt = 35.0f64.to_radians();
            let (fan_lo, fan_hi) = if child_angle.cos() >= 0.0 {
                (-l2_max_tilt, l2_max_tilt)
            } else {
                (std::f64::consts::PI - l2_max_tilt, std::f64::consts::PI + l2_max_tilt)
            };
            let available_fan = fan_hi - fan_lo;

            // Distribute children evenly across available range
            // Minimum angular gap: at the text offset distance, siblings must clear
            let l2_fs_calc = font_size_at_depth(cfg, 2);
            let text_offset_dist = (l2_fs_calc * 2.5).min(45.0); // where text begins
            let min_clearance = l2_fs_calc * 1.8 + 12.0; // text height + branch + gap
            let min_angle_gap = (min_clearance / text_offset_dist.max(1.0)).min(available_fan / n2 as f64);

            let l2_weights: Vec<f64> = child.children.iter().map(subtree_weight).collect();
            let l2_total: f64 = l2_weights.iter().sum();
            let l2_angles: Vec<f64> = l2_weights.iter()
                .map(|w| (available_fan * w / l2_total).max(min_angle_gap))
                .collect();
            // Re-center fan if angles expanded beyond available
            let actual_fan: f64 = l2_angles.iter().sum();
            let fan_center = (fan_lo + fan_hi) / 2.0;
            let mut l2_cursor = fan_center - actual_fan / 2.0;

            for (ci2, child2) in child.children.iter().enumerate() {
                let l2_angle = l2_cursor + l2_angles[ci2] / 2.0;

                let l2_fs = font_size_at_depth(cfg, 2);
                let l2_tw = builder::estimate_text_width(&child2.text, l2_fs);
                let l2_margin = 70.0; // matches SVG startOffset for L2
                let l2_branch_len = l2_margin + l2_tw + 15.0;

                // All L2 branches start from L1 endpoint (organic continuity)
                let l2_start_x = end_x;
                let l2_start_y = end_y;
                let l2_end_x = end_x + l2_branch_len * l2_angle.cos();
                let l2_end_y = end_y + l2_branch_len * l2_angle.sin();

                let l2_text_dist = l2_tw / 2.0 + 20.0;
                let l2_text_cx = end_x + l2_text_dist * l2_angle.cos();
                let l2_text_cy = end_y + l2_text_dist * l2_angle.sin();

                let mut l2_text_angle = l2_angle;
                if l2_text_angle.cos() < 0.0 {
                    l2_text_angle += std::f64::consts::PI;
                }

                let l2_dx = l2_end_x - l2_start_x;
                let l2_dy = l2_end_y - l2_start_y;
                let l2_gap = (l2_dx * l2_dx + l2_dy * l2_dy).sqrt();
                // CP1 points in fan direction (immediate spread), CP2 arrives horizontal
                let l2_cp1_dist = l2_gap * 0.35;
                let l2_cp2_dist = l2_gap * 0.35;
                let l2_nx = l2_angle.cos();
                let l2_ny = l2_angle.sin();
                let l2_hdir = if l2_end_x > l2_start_x { -1.0 } else { 1.0 };

                let (l2_ss, l2_es) = branch_sizes(1);
                let l2_conn_id = new_id();
                scene.add(Element {
                    id: l2_conn_id.clone(),
                    element_type: "arrow".into(),
                    x: l2_start_x, y: l2_start_y,
                    width: l2_dx, height: l2_dy,
                    stroke_color: color.into(),
                    background_color: "transparent".into(),
                    fill_style: "solid".into(),
                    stroke_width: 2.0, stroke_style: String::new(),
                    roughness: 0, opacity: 80,
                    font_family: 1, font_size: 0.0,
                    roundness: Some(Roundness { roundness_type: 2 }),
                    label: None, bound_elements: None,
                    text: None, original_text: None, text_align: None,
                    vertical_align: None, container_id: None,
                    points: Some(vec![
                        [0.0, 0.0],
                        [l2_nx * l2_cp1_dist, l2_ny * l2_cp1_dist], // spread outward at junction
                        [l2_dx + l2_hdir * l2_cp2_dist, l2_dy],     // arrive horizontal at tip
                        [l2_dx, l2_dy],
                    ]),
                    end_arrowhead: None, start_arrowhead: None,
                    start_binding: None, end_binding: None,
                    angle: None, is_deleted: false,
                    custom_data: Some(serde_json::json!({
                        "strokeOptions": { "organic": true, "startSize": l2_ss, "endSize": l2_es, "depth": 1 }
                    })),
                    group_ids: None, simulate_pressure: None,
                });

                // L2 text on branch
                let l2_text_id = new_id();
                let l2_text_h = l2_fs * 1.2;
                scene.add(Element {
                    id: l2_text_id,
                    element_type: "text".into(),
                    x: l2_text_cx - l2_tw / 2.0,
                    y: l2_text_cy - l2_text_h / 2.0,
                    width: l2_tw, height: l2_text_h,
                    stroke_color: color.into(),
                    background_color: "transparent".into(),
                    fill_style: "solid".into(),
                    stroke_width: 0.0, stroke_style: String::new(),
                    roughness: 0, opacity: 100,
                    font_family: 2, font_size: l2_fs,
                    roundness: None, label: None, bound_elements: None,
                    text: Some(child2.text.clone()),
                    original_text: Some(child2.text.clone()),
                    text_align: Some("center".into()),
                    vertical_align: Some("middle".into()),
                    container_id: None,
                    points: None, end_arrowhead: None, start_arrowhead: None,
                    start_binding: None, end_binding: None,
                    angle: Some(l2_text_angle),
                    is_deleted: false,
                    custom_data: Some(serde_json::json!({ "onBranch": l2_conn_id })),
                    group_ids: None, simulate_pressure: None,
                });

                l2_cursor += l2_angles[ci2];
            }
        }

        child_placed.push(Placed {
            element_id: text_id,
            x: text_cx - text_width / 2.0, y: text_cy - 10.0,
            width: text_width, height: 20.0,
            color_idx: ci,
            children: l2_placed,
        });
    }

    Placed {
        element_id: root_id,
        x: center_x - w / 2.0, y: center_y - h / 2.0,
        width: w, height: h,
        color_idx: 0,
        children: child_placed,
    }
}

// ── Public API ───────────────────────────────────────────────────────

/// Generate a mind map from parsed nodes.
pub fn generate(roots: &[MmNode], cfg: &MindMapConfig) -> Scene {
    let mut scene = Scene::new();

    match cfg.layout {
        Layout::Right => generate_right(&mut scene, roots, cfg),
        Layout::Radial => generate_radial(&mut scene, roots, cfg),
        Layout::Buzan => generate_buzan(&mut scene, roots, cfg),
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
