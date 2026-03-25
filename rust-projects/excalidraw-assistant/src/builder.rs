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

/// Add a rectangle with auto-sized label.
pub fn add_rect(scene: &mut Scene, x: f64, y: f64, label: &str, style: &Style) -> String {
    let (w, h) = size_for_label(label, style.font_size);
    let id = new_id();

    scene.add(Element {
        id: id.clone(),
        element_type: "rectangle".into(),
        x, y, width: w, height: h,
        stroke_color: style.stroke.clone(),
        background_color: style.fill.clone(),
        fill_style: "solid".into(),
        stroke_width: 1.0,
        stroke_style: String::new(),
        roughness: 0,
        opacity: style.opacity,
        font_family: 2, // Nunito
        font_size: style.font_size,
        roundness: Some(Roundness { roundness_type: 3 }),
        label: Some(Label {
            text: label.into(),
            font_size: style.font_size,
            font_family: 2,
        }),
        // defaults
        text: None, original_text: None, text_align: None,
        vertical_align: None, container_id: None,
        points: None, end_arrowhead: None, start_arrowhead: None,
        start_binding: None, end_binding: None, bound_elements: None,
        angle: None, is_deleted: false,
    });

    id
}

/// Add a diamond with auto-sized label.
pub fn add_diamond(scene: &mut Scene, x: f64, y: f64, label: &str, style: &Style) -> String {
    let (w, h) = size_for_label(label, style.font_size);
    // Diamond inscribes text at ~50% width, so double it
    let w = w * 2.0;
    let h = h * 1.5;
    let id = new_id();

    scene.add(Element {
        id: id.clone(),
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
        label: Some(Label {
            text: label.into(),
            font_size: style.font_size,
            font_family: 2,
        }),
        text: None, original_text: None, text_align: None,
        vertical_align: None, container_id: None,
        points: None, end_arrowhead: None, start_arrowhead: None,
        start_binding: None, end_binding: None, bound_elements: None,
        angle: None, is_deleted: false,
    });

    id
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
