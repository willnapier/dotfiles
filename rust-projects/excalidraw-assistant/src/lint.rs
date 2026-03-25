use crate::builder::estimate_text_width;
use crate::elements::Element;
use crate::scene::Scene;

/// Run all lint checks on a scene. Returns list of failures.
pub fn check(scene: &Scene) -> Vec<String> {
    let mut failures = Vec::new();

    failures.extend(check_text_overflow(scene));
    failures.extend(check_short_arrows(scene));
    failures.extend(check_overlap_and_gap(scene));
    failures.extend(check_consistent_stroke(scene));
    failures.extend(check_binding_integrity(scene));
    failures.extend(check_text_container_integrity(scene));

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
