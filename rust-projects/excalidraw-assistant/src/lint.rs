use crate::builder::estimate_text_width;
use crate::elements::Element;
use crate::scene::Scene;

/// Auto-fix all fixable issues. Returns list of what was fixed.
pub fn fix(scene: &mut Scene) -> Vec<String> {
    let mut fixed = Vec::new();

    // Fix text overflow by widening containers
    for i in 0..scene.elements.len() {
        if scene.elements[i].element_type != "text" {
            continue;
        }
        let text = match &scene.elements[i].text {
            Some(t) => t.clone(),
            None => continue,
        };
        let fs = scene.elements[i].font_size;
        let cid = match &scene.elements[i].container_id {
            Some(c) => c.clone(),
            None => continue,
        };

        let est_width = estimate_text_width(&text, fs);

        if let Some(container) = scene.elements.iter_mut().find(|e| e.id == cid) {
            let available = if container.element_type == "diamond" {
                container.width * 0.5
            } else {
                container.width - 20.0
            };

            if est_width > available {
                if container.element_type == "diamond" {
                    let new_w = (est_width * 1.15) / 0.5;
                    let delta = new_w - container.width;
                    container.x -= delta / 2.0;
                    container.width = new_w;
                } else {
                    let new_w = est_width + 40.0;
                    let delta = new_w - container.width;
                    container.x -= delta / 2.0;
                    container.width = new_w;
                }
                fixed.push(format!("Widened {} for \"{}\"", container.element_type, &text[..text.len().min(20)]));
            }
        }
    }

    // Fix short arrows by extending
    for el in &mut scene.elements {
        if el.element_type != "arrow" {
            continue;
        }
        if let Some(ref mut points) = el.points {
            if points.len() >= 2 {
                let dx = points[1][0] - points[0][0];
                let dy = points[1][1] - points[0][1];
                let len = (dx * dx + dy * dy).sqrt();
                if len > 0.0 && len < 15.0 {
                    let scale = 18.0 / len;
                    points[1][0] = points[0][0] + dx * scale;
                    points[1][1] = points[0][1] + dy * scale;
                    el.width = points[1][0];
                    el.height = points[1][1];
                    fixed.push(format!("Extended arrow at ({:.0},{:.0})", el.x, el.y));
                }
            }
        }
    }

    // Fix inconsistent stroke — standardise to 2.0
    let arrow_widths: Vec<f64> = scene.elements.iter()
        .filter(|e| e.element_type == "arrow")
        .map(|e| e.stroke_width)
        .collect();
    let has_inconsistency = {
        let mut unique = arrow_widths.clone();
        unique.sort_by(|a, b| a.partial_cmp(b).unwrap());
        unique.dedup();
        unique.len() > 1
    };
    if has_inconsistency {
        for el in &mut scene.elements {
            if el.element_type == "arrow" {
                el.stroke_width = 2.0;
            }
        }
        fixed.push("Standardised arrow strokeWidth to 2.0".into());
    }

    // Reposition text after container changes
    crate::layout::reposition_bound_text(scene);

    fixed
}

/// Run all lint checks on a scene. Returns list of failures.
pub fn check(scene: &Scene) -> Vec<String> {
    let mut failures = Vec::new();

    failures.extend(check_text_overflow(scene));
    failures.extend(check_short_arrows(scene));
    failures.extend(check_overlap_and_gap(scene));
    failures.extend(check_consistent_stroke(scene));
    failures.extend(check_binding_integrity(scene));
    failures.extend(check_text_container_integrity(scene));
    failures.extend(check_arrow_shape_clearance(scene));

    failures
}

/// Check: text fits within its container.
fn check_text_overflow(scene: &Scene) -> Vec<String> {
    let mut failures = Vec::new();

    for el in &scene.elements {
        if el.element_type != "text" {
            continue;
        }
        if let (Some(text), Some(container_id)) = (&el.text, &el.container_id) {
            if let Some(container) = scene.get(container_id) {
                let est_width = estimate_text_width(text, el.font_size);
                let available = if container.element_type == "diamond" {
                    container.width * 0.5
                } else {
                    container.width - 20.0 // padding
                };

                if est_width > available {
                    failures.push(format!(
                        "TEXT OVERFLOW: \"{}\" est {:.0}px in {} w={:.0} (avail {:.0})",
                        &text[..text.len().min(30)],
                        est_width,
                        container.element_type,
                        container.width,
                        available
                    ));
                }
            }
        }
    }

    failures
}

/// Check: arrow shaft length >= 15px.
fn check_short_arrows(scene: &Scene) -> Vec<String> {
    let mut failures = Vec::new();

    for el in &scene.elements {
        if el.element_type != "arrow" {
            continue;
        }
        if let Some(points) = &el.points {
            let mut total = 0.0;
            for i in 0..points.len().saturating_sub(1) {
                let dx = points[i + 1][0] - points[i][0];
                let dy = points[i + 1][1] - points[i][1];
                total += (dx * dx + dy * dy).sqrt();
            }
            if total > 0.0 && total < 15.0 {
                failures.push(format!(
                    "SHORT ARROW: {:.0}px (min 15) at ({:.0},{:.0})",
                    total, el.x, el.y
                ));
            }
        }
    }

    failures
}

/// Check: same-row rects don't overlap and have >= 12px gap.
fn check_overlap_and_gap(scene: &Scene) -> Vec<String> {
    let mut failures = Vec::new();

    let shapes: Vec<&Element> = scene.elements.iter()
        .filter(|e| (e.element_type == "rectangle" || e.element_type == "diamond") && e.width < 400.0)
        .collect();

    for i in 0..shapes.len() {
        for j in (i + 1)..shapes.len() {
            let (a, b) = (shapes[i], shapes[j]);

            // Horizontal (same row)
            if (a.y - b.y).abs() < 10.0 && a.x < b.x {
                let gap = b.x - a.right();
                if gap < 0.0 {
                    failures.push(format!("OVERLAP at y={:.0}", a.y));
                } else if gap < 12.0 {
                    failures.push(format!("GAP TOO SMALL: {:.0}px at y={:.0}", gap, a.y));
                }
            }

            // Vertical (same column)
            if (a.x - b.x).abs() < 10.0 && a.y < b.y {
                let gap = b.y - a.bottom();
                if gap > 0.0 && gap < 12.0 {
                    failures.push(format!("VERTICAL GAP: {:.0}px at x={:.0}", gap, a.x));
                }
            }
        }
    }

    failures
}

/// Check: all arrows have the same strokeWidth.
fn check_consistent_stroke(scene: &Scene) -> Vec<String> {
    let mut widths = std::collections::HashMap::new();

    for el in &scene.elements {
        if el.element_type == "arrow" {
            let key = format!("{:.1}", el.stroke_width);
            *widths.entry(key).or_insert(0u32) += 1;
        }
    }

    if widths.len() > 1 {
        let detail: Vec<String> = widths.iter().map(|(k, v)| format!("{}×sw={}", v, k)).collect();
        vec![format!("INCONSISTENT STROKE: {}", detail.join(", "))]
    } else {
        vec![]
    }
}

/// Check: every arrow binding references an existing element, and vice versa.
fn check_binding_integrity(scene: &Scene) -> Vec<String> {
    let mut failures = Vec::new();

    for el in &scene.elements {
        if el.element_type != "arrow" {
            continue;
        }

        if let Some(ref binding) = el.start_binding {
            if scene.get(&binding.element_id).is_none() {
                failures.push(format!(
                    "BROKEN BINDING: arrow {} startBinding references missing element {}",
                    &el.id[..8], &binding.element_id[..8.min(binding.element_id.len())]
                ));
            }
        }

        if let Some(ref binding) = el.end_binding {
            if scene.get(&binding.element_id).is_none() {
                failures.push(format!(
                    "BROKEN BINDING: arrow {} endBinding references missing element {}",
                    &el.id[..8], &binding.element_id[..8.min(binding.element_id.len())]
                ));
            }
        }
    }

    failures
}

/// Check: arrow segments maintain minimum clearance from non-connected shapes.
/// Catches: arrows grazing shape edges, arrows cutting through shapes.
fn check_arrow_shape_clearance(scene: &Scene) -> Vec<String> {
    let mut failures = Vec::new();
    let min_clearance = 12.0;

    let shapes: Vec<&Element> = scene.elements.iter()
        .filter(|e| e.element_type != "text" && e.element_type != "arrow")
        .collect();

    for arrow in scene.elements.iter().filter(|e| e.element_type == "arrow") {
        // Get IDs of connected shapes (skip clearance check against these)
        let connected: Vec<&str> = [
            arrow.start_binding.as_ref().map(|b| b.element_id.as_str()),
            arrow.end_binding.as_ref().map(|b| b.element_id.as_str()),
        ].iter().filter_map(|x| *x).collect();

        if let Some(ref points) = arrow.points {
            for seg in 0..points.len().saturating_sub(1) {
                let x1 = arrow.x + points[seg][0];
                let y1 = arrow.y + points[seg][1];
                let x2 = arrow.x + points[seg + 1][0];
                let y2 = arrow.y + points[seg + 1][1];

                let seg_left = x1.min(x2);
                let seg_right = x1.max(x2);
                let seg_top = y1.min(y2);
                let seg_bot = y1.max(y2);

                for shape in &shapes {
                    if connected.contains(&shape.id.as_str()) {
                        continue;
                    }

                    // Check if segment passes through or grazes this shape
                    let overlap_x = seg_right > shape.x && seg_left < shape.right();
                    let overlap_y = seg_bot > shape.y && seg_top < shape.bottom();

                    if overlap_x && overlap_y {
                        failures.push(format!(
                            "ARROW CROSSES SHAPE: arrow at ({:.0},{:.0}) segment {}-{} intersects shape at ({:.0},{:.0})",
                            arrow.x, arrow.y, seg, seg + 1, shape.x, shape.y
                        ));
                        break; // one failure per arrow is enough
                    }

                    // Check clearance for segments that pass alongside shapes
                    if overlap_x {
                        let gap_top = (seg_top - shape.bottom()).abs();
                        let gap_bot = (seg_bot - shape.y).abs();
                        let min_gap = gap_top.min(gap_bot);
                        if min_gap < min_clearance && min_gap > 0.0 {
                            failures.push(format!(
                                "ARROW CLEARANCE: {:.0}px (min {:.0}) from shape at ({:.0},{:.0})",
                                min_gap, min_clearance, shape.x, shape.y
                            ));
                        }
                    }
                    if overlap_y {
                        let gap_left = (seg_left - shape.right()).abs();
                        let gap_right = (seg_right - shape.x).abs();
                        let min_gap = gap_left.min(gap_right);
                        if min_gap < min_clearance && min_gap > 0.0 {
                            failures.push(format!(
                                "ARROW CLEARANCE: {:.0}px (min {:.0}) from shape at ({:.0},{:.0})",
                                min_gap, min_clearance, shape.x, shape.y
                            ));
                        }
                    }
                }
            }
        }
    }

    failures
}

/// Check: every text with containerId has a matching boundElements entry on the container.
fn check_text_container_integrity(scene: &Scene) -> Vec<String> {
    let mut failures = Vec::new();

    for el in &scene.elements {
        if el.element_type != "text" {
            continue;
        }

        if let Some(ref cid) = el.container_id {
            match scene.get(cid) {
                None => {
                    failures.push(format!(
                        "ORPHAN TEXT: text {} references missing container {}",
                        &el.id[..8], &cid[..8.min(cid.len())]
                    ));
                }
                Some(container) => {
                    let has_back_ref = container.bound_elements.as_ref()
                        .map(|be| be.iter().any(|b| b.id == el.id))
                        .unwrap_or(false);
                    if !has_back_ref {
                        failures.push(format!(
                            "UNLINKED TEXT: container {} missing boundElements for text {}",
                            &cid[..8.min(cid.len())], &el.id[..8]
                        ));
                    }
                }
            }
        }
    }

    failures
}
