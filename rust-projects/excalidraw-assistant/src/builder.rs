use crate::elements::*;
use crate::scene::Scene;
use crate::style::Style;

/// Per-character width ratios for Nunito (font_family 2), measured relative to fontSize.
/// Covers ASCII printable range; unknown chars fall back to 0.55.
fn char_width_ratio(ch: char) -> f64 {
    match ch {
        'i' | 'l' | '!' | '|' | '.' | ',' | ':' | ';' | '\'' => 0.30,
        'f' | 'j' | 't' | 'r' | '(' | ')' | '[' | ']' | '{' | '}' => 0.38,
        'a' | 'c' | 'e' | 'g' | 'k' | 'n' | 'o' | 'p' | 'q' | 's' | 'u' | 'v' | 'x' | 'y' | 'z' => 0.52,
        'b' | 'd' | 'h' => 0.54,
        'w' => 0.72,
        'm' => 0.80,
        'A' | 'B' | 'C' | 'D' | 'E' | 'F' | 'G' | 'H' | 'K' | 'L' | 'N' | 'P' | 'R' | 'S' | 'T' | 'U' | 'V' | 'X' | 'Y' | 'Z' => 0.62,
        'M' | 'W' => 0.82,
        'O' | 'Q' => 0.68,
        'I' | 'J' => 0.42,
        '0'..='9' => 0.55,
        ' ' => 0.30,
        '-' | '–' => 0.40,
        '—' => 0.70,
        '"' | '\u{201C}' | '\u{201D}' => 0.44,
        _ => 0.55,
    }
}

/// Estimate text width using per-character metrics for Nunito.
pub fn estimate_text_width(text: &str, font_size: f64) -> f64 {
    text.chars().map(|c| char_width_ratio(c) * font_size).sum()
}

/// Calculate appropriate box dimensions for a label.
fn size_for_label(text: &str, font_size: f64) -> (f64, f64) {
    let lines: Vec<&str> = text.split('\n').collect();
    let max_line = lines.iter().map(|l| l.len()).max().unwrap_or(0);
    let text_width = max_line as f64 * font_size * 0.6;
    let width = (text_width + 40.0).max(120.0); // padding
    let line_height = font_size * 1.4;
    let height = (lines.len() as f64 * line_height + 20.0).max(50.0);
    (width, height)
}

/// Add a rectangle with auto-sized label (text element bound to shape).
/// If `center_x` is true, `x` is treated as the desired centre, not left edge.
pub fn add_rect(scene: &mut Scene, x: f64, y: f64, label: &str, style: &Style, center_x: bool) -> String {
    let (w, h) = size_for_label(label, style.font_size);
    let x = if center_x { x - w / 2.0 } else { x };
    let shape_id = new_id();
    let text_id = new_id();

    // Shape with boundElements pointing to text
    scene.add(Element {
        id: shape_id.clone(),
        element_type: "rectangle".into(),
        x, y, width: w, height: h,
        stroke_color: style.stroke.clone(),
        background_color: style.fill.clone(),
        fill_style: "solid".into(),
        stroke_width: 1.0,
        stroke_style: String::new(),
        roughness: 0,
        opacity: style.opacity,
        font_family: 2,
        font_size: style.font_size,
        roundness: Some(Roundness { roundness_type: 3 }),
        label: None,
        bound_elements: Some(vec![BoundElement { id: text_id.clone(), bound_type: "text".into() }]),
        text: None, original_text: None, text_align: None,
        vertical_align: None, container_id: None,
        points: None, end_arrowhead: None, start_arrowhead: None,
        start_binding: None, end_binding: None,
        angle: None, is_deleted: false,
        custom_data: None, group_ids: None, simulate_pressure: None,
    });

    // Text element bound to the shape
    let text_width = estimate_text_width(label, style.font_size);
    let text_height = style.font_size * 1.2 * label.split('\n').count() as f64;
    scene.add(Element {
        id: text_id,
        element_type: "text".into(),
        x: x + (w - text_width) / 2.0,
        y: y + (h - text_height) / 2.0,
        width: text_width,
        height: text_height,
        stroke_color: style.stroke.clone(),
        background_color: "transparent".into(),
        fill_style: "solid".into(),
        stroke_width: 0.0,
        stroke_style: String::new(),
        roughness: 0,
        opacity: 100,
        font_family: 2,
        font_size: style.font_size,
        roundness: None,
        label: None,
        bound_elements: None,
        text: Some(label.into()),
        original_text: Some(label.into()),
        text_align: Some("center".into()),
        vertical_align: Some("middle".into()),
        container_id: Some(shape_id.clone()),
        points: None, end_arrowhead: None, start_arrowhead: None,
        start_binding: None, end_binding: None,
        angle: None, is_deleted: false,
        custom_data: None, group_ids: None, simulate_pressure: None,
    });

    shape_id
}

/// Add a diamond with auto-sized label (text element bound to shape).
pub fn add_diamond(scene: &mut Scene, x: f64, y: f64, label: &str, style: &Style, center_x: bool) -> String {
    let (w, h) = size_for_label(label, style.font_size);
    let w = w * 2.0;
    let h = h * 1.5;
    let x = if center_x { x - w / 2.0 } else { x };
    let shape_id = new_id();
    let text_id = new_id();

    scene.add(Element {
        id: shape_id.clone(),
        element_type: "diamond".into(),
        x, y, width: w, height: h,
        stroke_color: style.stroke.clone(),
        background_color: style.fill.clone(),
        fill_style: "solid".into(),
        stroke_width: 1.0,
        stroke_style: String::new(),
        roughness: 0,
        opacity: style.opacity,
        font_family: 2,
        font_size: style.font_size,
        roundness: None,
        label: None,
        bound_elements: Some(vec![BoundElement { id: text_id.clone(), bound_type: "text".into() }]),
        text: None, original_text: None, text_align: None,
        vertical_align: None, container_id: None,
        points: None, end_arrowhead: None, start_arrowhead: None,
        start_binding: None, end_binding: None,
        angle: None, is_deleted: false,
        custom_data: None, group_ids: None, simulate_pressure: None,
    });

    let text_width = estimate_text_width(label, style.font_size);
    let text_height = style.font_size * 1.2 * label.split('\n').count() as f64;
    scene.add(Element {
        id: text_id,
        element_type: "text".into(),
        x: x + (w - text_width) / 2.0,
        y: y + (h - text_height) / 2.0,
        width: text_width,
        height: text_height,
        stroke_color: style.stroke.clone(),
        background_color: "transparent".into(),
        fill_style: "solid".into(),
        stroke_width: 0.0,
        stroke_style: String::new(),
        roughness: 0,
        opacity: 100,
        font_family: 2,
        font_size: style.font_size,
        roundness: None,
        label: None,
        bound_elements: None,
        text: Some(label.into()),
        original_text: Some(label.into()),
        text_align: Some("center".into()),
        vertical_align: Some("middle".into()),
        container_id: Some(shape_id.clone()),
        points: None, end_arrowhead: None, start_arrowhead: None,
        start_binding: None, end_binding: None,
        angle: None, is_deleted: false,
        custom_data: None, group_ids: None, simulate_pressure: None,
    });

    shape_id
}

/// Add an ellipse with auto-sized label.
pub fn add_ellipse(scene: &mut Scene, x: f64, y: f64, label: &str, style: &Style, center_x: bool) -> String {
    let (w, h) = size_for_label(label, style.font_size);
    let w = w * 1.4; // ellipse needs more horizontal space
    let h = h * 1.3;
    let x = if center_x { x - w / 2.0 } else { x };
    let shape_id = new_id();
    let text_id = new_id();

    scene.add(Element {
        id: shape_id.clone(),
        element_type: "ellipse".into(),
        x, y, width: w, height: h,
        stroke_color: style.stroke.clone(),
        background_color: style.fill.clone(),
        fill_style: "solid".into(),
        stroke_width: 1.0,
        stroke_style: String::new(),
        roughness: 0,
        opacity: style.opacity,
        font_family: 2,
        font_size: style.font_size,
        roundness: None,
        label: None,
        bound_elements: Some(vec![BoundElement { id: text_id.clone(), bound_type: "text".into() }]),
        text: None, original_text: None, text_align: None,
        vertical_align: None, container_id: None,
        points: None, end_arrowhead: None, start_arrowhead: None,
        start_binding: None, end_binding: None,
        angle: None, is_deleted: false,
        custom_data: None, group_ids: None, simulate_pressure: None,
    });

    let text_width = estimate_text_width(label, style.font_size);
    let text_height = style.font_size * 1.2 * label.split('\n').count() as f64;
    scene.add(Element {
        id: text_id,
        element_type: "text".into(),
        x: x + (w - text_width) / 2.0,
        y: y + (h - text_height) / 2.0,
        width: text_width, height: text_height,
        stroke_color: style.stroke.clone(),
        background_color: "transparent".into(),
        fill_style: "solid".into(),
        stroke_width: 0.0, stroke_style: String::new(),
        roughness: 0, opacity: 100, font_family: 2, font_size: style.font_size,
        roundness: None, label: None, bound_elements: None,
        text: Some(label.into()), original_text: Some(label.into()),
        text_align: Some("center".into()), vertical_align: Some("middle".into()),
        container_id: Some(shape_id.clone()),
        points: None, end_arrowhead: None, start_arrowhead: None,
        start_binding: None, end_binding: None,
        angle: None, is_deleted: false,
        custom_data: None, group_ids: None, simulate_pressure: None,
    });

    shape_id
}

/// Add standalone text (title, annotation — not bound to a shape).
pub fn add_text(scene: &mut Scene, x: f64, y: f64, text: &str, font_size: f64, color: &str) -> String {
    let text_id = new_id();
    let text_width = estimate_text_width(text, font_size);
    let text_height = font_size * 1.2 * text.split('\n').count() as f64;

    scene.add(Element {
        id: text_id.clone(),
        element_type: "text".into(),
        x, y, width: text_width, height: text_height,
        stroke_color: color.into(),
        background_color: "transparent".into(),
        fill_style: "solid".into(),
        stroke_width: 0.0,
        stroke_style: String::new(),
        roughness: 0,
        opacity: 100,
        font_family: 2,
        font_size,
        roundness: None,
        label: None,
        bound_elements: None,
        text: Some(text.into()),
        original_text: Some(text.into()),
        text_align: Some("center".into()),
        vertical_align: Some("middle".into()),
        container_id: None,
        points: None, end_arrowhead: None, start_arrowhead: None,
        start_binding: None, end_binding: None,
        angle: None, is_deleted: false,
        custom_data: None, group_ids: None, simulate_pressure: None,
    });

    text_id
}

/// Add an arrow connecting two elements with proper binding.
pub fn add_arrow(
    scene: &mut Scene,
    from_id: &str,
    to_id: &str,
    from_point: [f64; 2],
    to_point: [f64; 2],
    style: &Style,
    label: Option<&str>,
) -> String {
    let from = scene.get(from_id).expect("from element not found").clone();
    let to = scene.get(to_id).expect("to element not found").clone();

    // Calculate start/end coordinates from fixedPoint
    let start_x = from.x + from_point[0] * from.width;
    let start_y = from.y + from_point[1] * from.height;
    let end_x = to.x + to_point[0] * to.width;
    let end_y = to.y + to_point[1] * to.height;

    let arrow_id = new_id();

    let arrow = Element {
        id: arrow_id.clone(),
        element_type: "arrow".into(),
        x: start_x,
        y: start_y,
        width: end_x - start_x,
        height: end_y - start_y,
        stroke_color: style.stroke.clone(),
        background_color: "transparent".into(),
        fill_style: "solid".into(),
        stroke_width: 2.0,
        stroke_style: String::new(),
        roughness: 0,
        opacity: style.opacity,
        font_family: 2,
        font_size: 0.0,
        roundness: None,
        points: Some(vec![[0.0, 0.0], [end_x - start_x, end_y - start_y]]),
        end_arrowhead: Some("arrow".into()),
        start_arrowhead: None,
        start_binding: Some(Binding {
            element_id: from_id.into(),
            fixed_point: from_point,
            focus: 0.0,
            gap: 0.0,
        }),
        end_binding: Some(Binding {
            element_id: to_id.into(),
            fixed_point: to_point,
            focus: 0.0,
            gap: 0.0,
        }),
        bound_elements: None,
        label: label.map(|l| Label {
            text: l.into(),
            font_size: 14.0,
            font_family: 2,
        }),
        text: None, original_text: None, text_align: None,
        vertical_align: None, container_id: None,
        angle: None, is_deleted: false,
        custom_data: None, group_ids: None, simulate_pressure: None,
    };

    scene.add(arrow);

    // Add boundElements to source and target
    if let Some(el) = scene.get_mut(from_id) {
        let bound = el.bound_elements.get_or_insert_with(Vec::new);
        bound.push(BoundElement { id: arrow_id.clone(), bound_type: "arrow".into() });
    }
    if let Some(el) = scene.get_mut(to_id) {
        let bound = el.bound_elements.get_or_insert_with(Vec::new);
        bound.push(BoundElement { id: arrow_id.clone(), bound_type: "arrow".into() });
    }

    arrow_id
}

/// Connect two elements with an arrow from bottom of source to top of target.
pub fn connect_down(scene: &mut Scene, from_id: &str, to_id: &str, style: &Style) -> String {
    add_arrow(scene, from_id, to_id, [0.5, 1.0], [0.5, 0.0], style, None)
}

/// Connect two elements with an arrow from right of source to left of target.
pub fn connect_right(scene: &mut Scene, from_id: &str, to_id: &str, style: &Style) -> String {
    add_arrow(scene, from_id, to_id, [1.0, 0.5], [0.0, 0.5], style, None)
}

/// Check if a line segment intersects any element (other than from/to).
fn segment_crosses_element(
    x1: f64, y1: f64, x2: f64, y2: f64,
    scene: &Scene, skip_ids: &[&str],
) -> bool {
    let sl = x1.min(x2);
    let sr = x1.max(x2);
    let st = y1.min(y2);
    let sb = y1.max(y2);
    let margin = 3.0;

    for el in &scene.elements {
        if el.element_type == "text" || el.element_type == "arrow" {
            continue;
        }
        if skip_ids.contains(&el.id.as_str()) {
            continue;
        }
        if sr >= el.x + margin && sl <= el.right() - margin
            && sb >= el.y + margin && st <= el.bottom() - margin
        {
            return true;
        }
    }
    false
}

/// Check if a multi-segment path crosses any element.
fn path_crosses(points: &[(f64, f64)], scene: &Scene, skip_ids: &[&str]) -> bool {
    for i in 0..points.len().saturating_sub(1) {
        if segment_crosses_element(points[i].0, points[i].1, points[i+1].0, points[i+1].1, scene, skip_ids) {
            return true;
        }
    }
    false
}

/// Create a multi-point routed arrow between two elements.
fn create_routed_arrow(
    scene: &mut Scene,
    from_id: &str,
    to_id: &str,
    from_point: [f64; 2],
    to_point: [f64; 2],
    points: &[(f64, f64)],
    style: &Style,
    label: Option<&str>,
) -> String {
    let sx = points[0].0;
    let sy = points[0].1;
    let ex = points.last().unwrap().0;
    let ey = points.last().unwrap().1;

    let arrow_id = new_id();
    let rel_points: Vec<[f64; 2]> = points.iter()
        .map(|(px, py)| [px - sx, py - sy])
        .collect();

    let arrow = Element {
        id: arrow_id.clone(),
        element_type: "arrow".into(),
        x: sx, y: sy,
        width: ex - sx, height: ey - sy,
        stroke_color: style.stroke.clone(),
        background_color: "transparent".into(),
        fill_style: "solid".into(),
        stroke_width: 2.0,
        stroke_style: String::new(),
        roughness: 0,
        opacity: style.opacity,
        font_family: 2, font_size: 0.0,
        roundness: None,
        points: Some(rel_points),
        end_arrowhead: Some("arrow".into()),
        start_arrowhead: None,
        start_binding: Some(crate::elements::Binding {
            element_id: from_id.into(),
            fixed_point: from_point,
            focus: 0.0, gap: 0.0,
        }),
        end_binding: Some(crate::elements::Binding {
            element_id: to_id.into(),
            fixed_point: to_point,
            focus: 0.0, gap: 0.0,
        }),
        bound_elements: None,
        label: label.map(|l| crate::elements::Label {
            text: l.into(), font_size: 14.0, font_family: 2,
        }),
        text: None, original_text: None, text_align: None,
        vertical_align: None, container_id: None,
        angle: None, is_deleted: false,
        custom_data: None, group_ids: None, simulate_pressure: None,
    };

    scene.add(arrow);

    if let Some(el) = scene.get_mut(from_id) {
        let bound = el.bound_elements.get_or_insert_with(Vec::new);
        bound.push(crate::elements::BoundElement { id: arrow_id.clone(), bound_type: "arrow".into() });
    }
    if let Some(el) = scene.get_mut(to_id) {
        let bound = el.bound_elements.get_or_insert_with(Vec::new);
        bound.push(crate::elements::BoundElement { id: arrow_id.clone(), bound_type: "arrow".into() });
    }

    arrow_id
}

/// Smart connect: if direct path crosses an obstacle, propose routes.
/// Returns the arrow ID and any routing notes.
pub fn smart_connect(
    scene: &mut Scene,
    from_id: &str,
    to_id: &str,
    from_point: [f64; 2],
    to_point: [f64; 2],
    style: &Style,
    label: Option<&str>,
) -> (String, Option<String>) {
    let from = scene.get(from_id).expect("from not found").clone();
    let to = scene.get(to_id).expect("to not found").clone();

    let sx = from.x + from_point[0] * from.width;
    let sy = from.y + from_point[1] * from.height;
    let ex = to.x + to_point[0] * to.width;
    let ey = to.y + to_point[1] * to.height;

    let skip = [from_id, to_id];

    // For perpendicular connections (side exit → top entry), force L-routing
    let is_side_exit = from_point[1] == 0.5 && (from_point[0] == 0.0 || from_point[0] == 1.0);
    let is_top_entry = to_point[1] == 0.0 && to_point[0] == 0.5;
    if is_side_exit && is_top_entry && (ey - sy).abs() > 10.0 {
        // L-route: horizontal from source side, then vertical down to target top
        let points = vec![(sx, sy), (ex, sy), (ex, ey)];
        if !path_crosses(&points, scene, &skip) {
            let arrow_id = create_routed_arrow(scene, from_id, to_id, from_point, to_point, &points, style, label);
            return (arrow_id, Some("L-routed".into()));
        }
        // Try alternative L: vertical first, then horizontal
        let points2 = vec![(sx, sy), (sx, ey), (ex, ey)];
        if !path_crosses(&points2, scene, &skip) {
            let arrow_id = create_routed_arrow(scene, from_id, to_id, from_point, to_point, &points2, style, label);
            return (arrow_id, Some("L-routed alt".into()));
        }
    }

    // Try direct path first
    if !segment_crosses_element(sx, sy, ex, ey, scene, &skip) {
        let id = add_arrow(scene, from_id, to_id, from_point, to_point, style, label);
        return (id, None);
    }

    // Find ALL obstacles in the bounding box between start and end
    let bbox_left = sx.min(ex) - 5.0;
    let bbox_right = sx.max(ex) + 5.0;
    let bbox_top = sy.min(ey);
    let bbox_bot = sy.max(ey);

    let obstacles: Vec<&crate::elements::Element> = scene.elements.iter()
        .filter(|e| e.element_type != "text" && e.element_type != "arrow" && !skip.contains(&e.id.as_str()))
        .filter(|e| e.right() >= bbox_left && e.x <= bbox_right && e.bottom() >= bbox_top && e.y <= bbox_bot)
        .collect();

    // Find the widest extent of ALL obstacles
    let obs_left = obstacles.iter().map(|e| e.x).fold(f64::MAX, f64::min);
    let obs_right = obstacles.iter().map(|e| e.right()).fold(f64::MIN, f64::max);

    let pad = 40.0;

    // Departure: drop below source before going horizontal
    let depart_y = from.bottom() + 20.0;
    // Approach: midpoint between lowest obstacle bottom and target top
    let obs_bottom = obstacles.iter().map(|e| e.bottom()).fold(f64::MIN, f64::max);
    let approach_y = (obs_bottom + to.y) / 2.0;

    // Route options — go OUTSIDE all obstacles, approach target vertically
    let route_left = vec![(sx, sy), (sx, depart_y), (obs_left - pad, depart_y), (obs_left - pad, approach_y), (ex, approach_y), (ex, ey)];
    let route_right = vec![(sx, sy), (sx, depart_y), (obs_right + pad, depart_y), (obs_right + pad, approach_y), (ex, approach_y), (ex, ey)];

    // Pick the tightest clear route
    let (points, note) = if !path_crosses(&route_left, scene, &skip) {
        (route_left, "routed left")
    } else if !path_crosses(&route_right, scene, &skip) {
        (route_right, "routed right")
    } else {
        // Fallback: direct (accept the crossing)
        let id = add_arrow(scene, from_id, to_id, from_point, to_point, style, label);
        return (id, Some("WARNING: no clear route found, direct path used".into()));
    };

    let arrow_id = create_routed_arrow(scene, from_id, to_id, from_point, to_point, &points, style, label);
    (arrow_id, Some(format!("Arrow {}", note)))
}

/// Add a polyline (no arrowheads) from a sequence of absolute points.
/// Returns the element ID. Points are stored relative to the first point.
pub fn add_line(scene: &mut Scene, points: &[[f64; 2]], style: &Style) -> String {
    let line_id = new_id();
    let origin = points.first().copied().unwrap_or([0.0, 0.0]);
    let rel_points: Vec<[f64; 2]> = points.iter()
        .map(|p| [p[0] - origin[0], p[1] - origin[1]])
        .collect();
    let last = points.last().copied().unwrap_or([0.0, 0.0]);

    scene.add(Element {
        id: line_id.clone(),
        element_type: "line".into(),
        x: origin[0],
        y: origin[1],
        width: last[0] - origin[0],
        height: last[1] - origin[1],
        stroke_color: style.stroke.clone(),
        background_color: "transparent".into(),
        fill_style: "solid".into(),
        stroke_width: style.font_size.max(2.0).min(4.0), // sensible default
        stroke_style: String::new(),
        roughness: 0,
        opacity: style.opacity,
        font_family: 1,
        font_size: 0.0,
        roundness: Some(Roundness { roundness_type: 2 }),
        label: None,
        bound_elements: None,
        text: None, original_text: None, text_align: None,
        vertical_align: None, container_id: None,
        points: Some(rel_points),
        end_arrowhead: None, start_arrowhead: None,
        start_binding: None, end_binding: None,
        angle: None, is_deleted: false,
        custom_data: None, group_ids: None, simulate_pressure: None,
    });

    line_id
}

/// Assign a set of elements to a shared Excalidraw group.
/// Returns the generated groupId.
pub fn add_to_group(scene: &mut Scene, element_ids: &[&str]) -> String {
    let group_id = new_id();
    for eid in element_ids {
        if let Some(el) = scene.get_mut(eid) {
            let groups = el.group_ids.get_or_insert_with(Vec::new);
            groups.push(group_id.clone());
        }
    }
    group_id
}

/// Named connection sides for `connect_objects`.
pub enum ConnectionSide {
    Top,
    Bottom,
    Left,
    Right,
}

impl ConnectionSide {
    /// Parse from string (CLI-friendly).
    pub fn from_str(s: &str) -> Self {
        match s {
            "top" => Self::Top,
            "bottom" => Self::Bottom,
            "left" => Self::Left,
            "right" => Self::Right,
            _ => Self::Bottom,
        }
    }

    /// Convert to Excalidraw fixedPoint [x_ratio, y_ratio].
    pub fn to_fixed_point(&self) -> [f64; 2] {
        match self {
            Self::Top => [0.5, 0.0],
            Self::Bottom => [0.5, 1.0],
            Self::Left => [0.0, 0.5],
            Self::Right => [1.0, 0.5],
        }
    }
}

/// High-level connector: connect two elements by named sides.
/// Uses smart_connect internally for obstacle avoidance.
pub fn connect_objects(
    scene: &mut Scene,
    from_id: &str,
    from_side: ConnectionSide,
    to_id: &str,
    to_side: ConnectionSide,
    style: &Style,
    label: Option<&str>,
) -> (String, Option<String>) {
    smart_connect(
        scene, from_id, to_id,
        from_side.to_fixed_point(),
        to_side.to_fixed_point(),
        style, label,
    )
}

/// Compute bounding box of a set of elements.
/// Returns (min_x, min_y, width, height).
pub fn bounding_box(scene: &Scene, element_ids: &[&str]) -> (f64, f64, f64, f64) {
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;

    for eid in element_ids {
        if let Some(el) = scene.get(eid) {
            min_x = min_x.min(el.x);
            min_y = min_y.min(el.y);
            max_x = max_x.max(el.right());
            max_y = max_y.max(el.bottom());
            // Include points for lines/arrows
            if let Some(ref pts) = el.points {
                for p in pts {
                    min_x = min_x.min(el.x + p[0]);
                    min_y = min_y.min(el.y + p[1]);
                    max_x = max_x.max(el.x + p[0]);
                    max_y = max_y.max(el.y + p[1]);
                }
            }
        }
    }

    (min_x, min_y, max_x - min_x, max_y - min_y)
}
