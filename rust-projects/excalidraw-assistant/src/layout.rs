use crate::scene::Scene;
use std::collections::{HashMap, HashSet};

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

/// Graph-aware tree layout that handles branching at decision points.
/// Finds connected components, lays each out as a tree, stacks components vertically.
pub fn tree(
    scene: &mut Scene,
    connections: &[(String, String, Option<String>)],
    node_ids: &HashMap<String, String>,
    gap: f64,
) {
    let h_gap = gap * 1.5;

    // Build adjacency
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    let mut has_parent: HashSet<String> = HashSet::new();

    for (from, to, _) in connections {
        let from_key = from.split('.').next().unwrap_or(from).to_string();
        let to_key = to.split('.').next().unwrap_or(to).to_string();
        if node_ids.contains_key(&from_key) && node_ids.contains_key(&to_key) {
            children.entry(from_key.clone()).or_default().push(to_key.clone());
            has_parent.insert(to_key);
        }
    }

    // Find connected components via roots
    let all_names: Vec<String> = node_ids.keys().cloned().collect();
    let mut roots: Vec<String> = all_names.iter()
        .filter(|n| !has_parent.contains(*n))
        .cloned()
        .collect();
    // Sort roots by their first appearance in the connections for stable ordering
    roots.sort_by_key(|r| {
        connections.iter().position(|(f, _, _)| f.split('.').next().unwrap_or(f) == r)
            .unwrap_or(usize::MAX)
    });

    // BFS from each root to find component members and assign levels
    let mut placed: HashSet<String> = HashSet::new();
    let mut current_y = 100.0;

    for root in &roots {
        if placed.contains(root) {
            continue;
        }

        // BFS to assign levels within this component
        let mut levels: Vec<Vec<String>> = Vec::new();
        let mut queue: Vec<(String, usize)> = vec![(root.clone(), 0)];
        let mut node_level: HashMap<String, usize> = HashMap::new();
        let mut component_visited: HashSet<String> = HashSet::new();

        while let Some((node, level)) = queue.first().cloned() {
            queue.remove(0);

            // For convergence nodes (multiple parents), use the deepest level
            if let Some(&existing) = node_level.get(&node) {
                if level > existing {
                    // Remove from old level, re-add at deeper level
                    if existing < levels.len() {
                        levels[existing].retain(|n| n != &node);
                    }
                    node_level.insert(node.clone(), level);
                    while levels.len() <= level {
                        levels.push(Vec::new());
                    }
                    levels[level].push(node.clone());
                }
                continue;
            }

            if component_visited.contains(&node) {
                continue;
            }
            component_visited.insert(node.clone());
            node_level.insert(node.clone(), level);

            while levels.len() <= level {
                levels.push(Vec::new());
            }
            levels[level].push(node.clone());

            if let Some(kids) = children.get(&node) {
                for kid in kids {
                    queue.push((kid.clone(), level + 1));
                }
            }
        }

        // Position this component
        let component_centre_x = 400.0; // centre of canvas

        for level in &levels {
            if level.is_empty() {
                continue;
            }

            // Calculate total width needed for this row
            let widths: Vec<f64> = level.iter()
                .filter_map(|name| node_ids.get(name).and_then(|id| scene.get(id)).map(|e| e.width))
                .collect();
            let total_w: f64 = widths.iter().sum::<f64>() + h_gap * (widths.len() as f64 - 1.0).max(0.0);
            let mut x_cursor = component_centre_x - total_w / 2.0;
            let mut row_height = 0.0f64;

            for name in level {
                if let Some(elem_id) = node_ids.get(name) {
                    if let Some(el) = scene.get_mut(elem_id) {
                        let w = el.width;
                        el.x = x_cursor;
                        el.y = current_y;
                        x_cursor += w + h_gap;
                        row_height = row_height.max(el.height);
                        placed.insert(name.clone());
                    }
                }
            }

            current_y += row_height + gap;
        }

        current_y += gap; // extra gap between components
    }

    // Place any remaining unconnected nodes
    for name in &all_names {
        if placed.contains(name) {
            continue;
        }
        if let Some(elem_id) = node_ids.get(name) {
            if let Some(el) = scene.get_mut(elem_id) {
                el.x = 400.0 - el.width / 2.0;
                el.y = current_y;
                current_y += el.height + gap;
                placed.insert(name.clone());
            }
        }
    }

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
            // Preserve multi-point routed paths (from smart_connect obstacle routing)
            let has_routing = el.points.as_ref().map(|p| p.len() > 2).unwrap_or(false);
            if has_routing {
                // Translate all waypoints relative to new start position
                let dx = sx - el.x;
                let dy = sy - el.y;
                if let Some(ref mut pts) = el.points {
                    for p in pts.iter_mut() {
                        p[0] += dx;
                        p[1] += dy;
                    }
                }
                el.x = sx;
                el.y = sy;
            } else {
                el.x = sx;
                el.y = sy;
                el.width = ex - sx;
                el.height = ey - sy;
                el.points = Some(vec![[0.0, 0.0], [ex - sx, ey - sy]]);
            }
        }
    }
}
