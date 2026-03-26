use crate::elements::*;
use crate::scene::Scene;
use crate::style::Style;

/// Estimate text width: chars × fontSize × 0.55 (Nunito average).
pub fn estimate_text_width(text: &str, font_size: f64) -> f64 {
    text.len() as f64 * font_size * 0.55
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

    // Create multi-point arrow
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

    (arrow_id, Some(format!("Arrow {}", note)))
}
