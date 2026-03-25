use crate::scene::Scene;

/// Arrange all shapes in a vertical or horizontal flow.
/// Centres all shapes on a common axis with consistent gaps.
/// Text elements follow their containers automatically.
pub fn flow(scene: &mut Scene, direction: &str, gap: f64) {
    // Collect shape indices (skip text elements — they follow containers)
    let shape_indices: Vec<usize> = scene.elements.iter().enumerate()
        .filter(|(_, e)| e.element_type != "text" && e.element_type != "arrow")
        .map(|(i, _)| i)
        .collect();

    if shape_indices.is_empty() {
        return;
    }

    match direction {
        "down" | "vertical" => layout_vertical(scene, &shape_indices, gap),
        "right" | "horizontal" => layout_horizontal(scene, &shape_indices, gap),
        _ => layout_vertical(scene, &shape_indices, gap),
    }

    // Reposition text elements to match their containers
    reposition_bound_text(scene);
}

fn layout_vertical(scene: &mut Scene, shape_indices: &[usize], gap: f64) {
    // Find the widest shape to determine centre axis
    let max_width = shape_indices.iter()
        .map(|&i| scene.elements[i].width)
        .fold(0.0f64, f64::max);

    let start_x = 100.0; // left margin
    let centre_x = start_x + max_width / 2.0;
    let mut current_y = 100.0; // top margin

    for &idx in shape_indices {
        let w = scene.elements[idx].width;
        scene.elements[idx].x = centre_x - w / 2.0;
        scene.elements[idx].y = current_y;
        current_y += scene.elements[idx].height + gap;
    }
}

fn layout_horizontal(scene: &mut Scene, shape_indices: &[usize], gap: f64) {
    let max_height = shape_indices.iter()
        .map(|&i| scene.elements[i].height)
        .fold(0.0f64, f64::max);

    let start_y = 100.0;
    let centre_y = start_y + max_height / 2.0;
    let mut current_x = 100.0;

    for &idx in shape_indices {
        let h = scene.elements[idx].height;
        scene.elements[idx].x = current_x;
        scene.elements[idx].y = centre_y - h / 2.0;
        current_x += scene.elements[idx].width + gap;
    }
}

/// Reposition all bound text elements to the centre of their container.
pub fn reposition_bound_text(scene: &mut Scene) {
    // Collect container positions first (avoid borrow issues)
    let container_positions: Vec<(String, f64, f64, f64, f64)> = scene.elements.iter()
        .filter(|e| e.element_type != "text")
        .map(|e| (e.id.clone(), e.x, e.y, e.width, e.height))
        .collect();

    for el in &mut scene.elements {
        if el.element_type != "text" {
            continue;
        }
        if let Some(ref cid) = el.container_id {
            if let Some((_, cx, cy, cw, ch)) = container_positions.iter().find(|(id, _, _, _, _)| id == cid) {
                // Centre text in container
                el.x = cx + (cw - el.width) / 2.0;
                el.y = cy + (ch - el.height) / 2.0;
            }
        }
    }
}

/// Recalculate all arrow positions based on current shape positions.
/// Arrows with bindings get their start/end recalculated from the bound shapes.
pub fn recalculate_arrows(scene: &mut Scene) {
    // Collect shape positions
    let positions: std::collections::HashMap<String, (f64, f64, f64, f64)> = scene.elements.iter()
        .filter(|e| e.element_type != "text" && e.element_type != "arrow")
        .map(|e| (e.id.clone(), (e.x, e.y, e.width, e.height)))
        .collect();

    for el in &mut scene.elements {
        if el.element_type != "arrow" {
            continue;
        }

        let start = el.start_binding.as_ref().and_then(|b| {
            positions.get(&b.element_id).map(|&(x, y, w, h)| {
                (x + b.fixed_point[0] * w, y + b.fixed_point[1] * h)
            })
        });

        let end = el.end_binding.as_ref().and_then(|b| {
            positions.get(&b.element_id).map(|&(x, y, w, h)| {
                (x + b.fixed_point[0] * w, y + b.fixed_point[1] * h)
            })
        });

        if let (Some((sx, sy)), Some((ex, ey))) = (start, end) {
            el.x = sx;
            el.y = sy;
            el.width = ex - sx;
            el.height = ey - sy;
            el.points = Some(vec![[0.0, 0.0], [ex - sx, ey - sy]]);
        }
    }
}
