use anyhow::Result;
use crate::builder;
use crate::scene::Scene;
use crate::style::Style;

/// Parse a D2 file and create an Excalidraw scene.
/// Handles basic D2 features: nodes, connections, styles, direction.
pub fn from_d2(d2_source: &str, style_preset: &str) -> Result<Scene> {
    let mut scene = Scene::new();
    let mut node_ids: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut connections: Vec<(String, String, Option<String>)> = Vec::new();

    let mut depth = 0; // brace depth tracker

    for line in d2_source.lines() {
        let line = line.trim();

        // Skip empty lines, comments, global styles, title blocks
        if line.is_empty() || line.starts_with('#') || line.starts_with("**") {
            continue;
        }
        if line.starts_with("direction:") || line.starts_with("title:") {
            continue;
        }
        if line.starts_with('|') || line == "}" {
            // Track closing braces
            if line == "}" {
                depth = (depth - 1).max(0);
            }
            continue;
        }

        // Track brace depth — lines inside blocks are properties, not nodes
        let opens = line.chars().filter(|&c| c == '{').count() as i32;
        let closes = line.chars().filter(|&c| c == '}').count() as i32;

        // If we're inside a block and this isn't the opening line, skip it
        if depth > 0 && opens == 0 {
            depth += opens - closes;
            continue;
        }

        // Connection: "a -> b" or "a -> b: label" or "a -> b: { style... }"
        if line.contains(" -> ") {
            let parts: Vec<&str> = line.splitn(2, " -> ").collect();
            if parts.len() == 2 {
                let from = parts[0].trim().to_string();
                let rest = parts[1].trim();

                // Handle "b: label {", "b: {", "b: label", or just "b"
                let (to, label) = if rest.contains(':') {
                    let p: Vec<&str> = rest.splitn(2, ':').collect();
                    let to = p[0].trim().split('{').next().unwrap_or("").trim().to_string();
                    let lbl = p[1].trim().split('{').next().unwrap_or("").trim();
                    let lbl = lbl.trim_matches('"');
                    if lbl.is_empty() { (to, None) } else { (to, Some(lbl.to_string())) }
                } else {
                    (rest.split('{').next().unwrap_or(rest).trim().to_string(), None)
                };

                // Handle chained connections: "a -> b -> c"
                let chain: Vec<&str> = line.split(" -> ").collect();
                if chain.len() > 2 {
                    for i in 0..chain.len() - 1 {
                        let f = chain[i].trim().split(':').next().unwrap_or("").trim().to_string();
                        let t = chain[i + 1].trim().split(':').next().unwrap_or("").split('{').next().unwrap_or("").trim().to_string();
                        if !f.is_empty() && !t.is_empty() {
                            connections.push((f, t, None));
                        }
                    }
                } else {
                    connections.push((from, to, label));
                }
                // Track depth for connection style blocks
                depth += opens - closes;
                continue;
            }
        }

        // Node definition: "name: Label" or "name: Label { ... }"
        if line.contains(':') && !line.starts_with("style") {
            let parts: Vec<&str> = line.splitn(2, ':').collect();
            if parts.len() == 2 {
                let name = parts[0].trim();
                let rest = parts[1].trim();

                // Skip sub-properties like "style.fill"
                if name.contains('.') {
                    continue;
                }

                // Extract label (before any { block)
                let label = rest.split('{').next().unwrap_or(rest).trim()
                    .trim_matches('"')
                    .replace("\\n", "\n");

                if label.is_empty() || label == "{" {
                    continue;
                }

                // Determine style from D2 fill colour or preset
                let style = guess_style_from_d2(d2_source, name, style_preset);

                // Check shape type for this node
                let node_block = full_block_for_node(d2_source, name);
                let is_diamond = node_block.map(|b| b.contains("shape: diamond")).unwrap_or(false);
                let is_oval = node_block.map(|b| b.contains("shape: oval") || b.contains("shape: circle")).unwrap_or(false);
                let is_page = node_block.map(|b| b.contains("shape: page")).unwrap_or(false);

                // Skip shape type specifiers from label
                let label = label.replace("shape: oval", "").replace("shape: circle", "")
                    .replace("shape: page", "").replace("shape: diamond", "")
                    .replace("shape: hexagon", "").replace("shape: package", "")
                    .replace("shape: queue", "").replace("shape: person", "")
                    .trim().to_string();

                if label.is_empty() {
                    continue;
                }

                let id = if is_diamond {
                    builder::add_diamond(&mut scene, 0.0, 0.0, &label, &style, false)
                } else if is_oval {
                    builder::add_ellipse(&mut scene, 0.0, 0.0, &label, &style, false)
                } else {
                    builder::add_rect(&mut scene, 0.0, 0.0, &label, &style, false)
                };

                node_ids.insert(name.to_string(), id);

                // Track depth for lines that open a block
                depth += opens - closes;
            }
        }
    }

    // Layout BEFORE connecting so smart_connect can detect obstacles
    crate::layout::flow(&mut scene, "down", 28.0);

    // Create connections
    for (from, to, label) in &connections {
        let from_key = from.split('.').next().unwrap_or(from);
        let to_key = to.split('.').next().unwrap_or(to);

        if let (Some(from_id), Some(to_id)) = (node_ids.get(from_key), node_ids.get(to_key)) {
            let s = Style::arrow();
            builder::smart_connect(
                &mut scene, from_id, to_id,
                [0.5, 1.0], [0.5, 0.0],
                &s, label.as_deref(),
            );
        }
    }

    // Reposition bound text after connecting (don't re-layout shapes — it destroys smart routing)
    crate::layout::reposition_bound_text(&mut scene);

    Ok(scene)
}

/// Extract the block of D2 source belonging to a specific node.
fn full_block_for_node<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    // Find "name: ... {" and extract until matching "}"
    let search = format!("{}: ", name);
    let start = source.find(&search)?;
    let block_start = source[start..].find('{')?;
    let mut depth = 0;
    let block_region = &source[start + block_start..];
    for (i, c) in block_region.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&source[start..start + block_start + i + 1]);
                }
            }
            _ => {}
        }
    }
    // No braces — single line node
    let end = source[start..].find('\n').unwrap_or(source.len() - start);
    Some(&source[start..start + end])
}

/// Guess a style from D2 colour annotations or use the preset.
fn guess_style_from_d2(full_source: &str, node_name: &str, preset: &str) -> Style {
    // Use the exact node block, not an arbitrary window
    if let Some(block) = full_block_for_node(full_source, node_name) {
        if block.contains("#aed6f1") || block.contains("#2471a3") {
            return Style::william();
        }
        if block.contains("#d2b4de") || block.contains("#7d3c98") {
            return Style::leigh();
        }
        if block.contains("#a9dfbf") || block.contains("#1e8449") {
            return Style::ai();
        }
        if block.contains("#f9e79f") || block.contains("#d4ac0d") {
            return Style::automated();
        }
        if block.contains("#f5b7b1") || block.contains("#c0392b") || block.contains("#ffebee") {
            return Style::urgent();
        }
    }

    // Fallback to preset default
    if preset == "clinical" {
        Style::automated()
    } else {
        Style::default()
    }
}
