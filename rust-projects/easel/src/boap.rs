//! Book-on-a-Page (BOAP): spatial visual synthesis from a TOML manifest.
//!
//! Input: a TOML file describing clusters of concepts with spatial hints and cross-connections.
//! Output: an Excalidraw Scene with titled boundary rectangles, text blocks, and arrows.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::builder;
use crate::elements::{self, Element, Roundness};
use crate::scene::Scene;
use crate::style::Style;

// ---------------------------------------------------------------------------
// Manifest types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct BoapManifest {
    pub meta: Meta,
    #[serde(default)]
    pub cluster: Vec<Cluster>,
    #[serde(default)]
    pub connection: Vec<Connection>,
}

#[derive(Debug, Deserialize)]
pub struct Meta {
    pub title: String,
    #[serde(default)]
    pub author: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Cluster {
    pub name: String,
    #[serde(default = "default_hint")]
    pub hint: String,
    #[serde(default = "default_color")]
    pub color: String,
    #[serde(default)]
    pub items: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Connection {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub label: Option<String>,
}

fn default_hint() -> String { "center".into() }
fn default_color() -> String { "default".into() }

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

pub fn parse_manifest(toml_str: &str) -> Result<BoapManifest> {
    toml::from_str(toml_str).context("Failed to parse BOAP manifest")
}

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

const CANVAS_W: f64 = 1400.0;
const CANVAS_H: f64 = 1000.0;
const CLUSTER_PAD: f64 = 20.0;  // padding inside cluster boundary
const ITEM_GAP: f64 = 12.0;     // vertical gap between items
const ITEM_FONT: f64 = 14.0;
const TITLE_FONT: f64 = 28.0;
const CLUSTER_TITLE_FONT: f64 = 18.0;
const TITLE_Y: f64 = 30.0;

/// Map a spatial hint string to an approximate (x, y) anchor for the cluster centre.
fn hint_to_anchor(hint: &str) -> (f64, f64) {
    // 3x3 grid with margins
    let col = |c: f64| 200.0 + c * (CANVAS_W - 400.0) / 2.0;
    let row = |r: f64| 150.0 + r * (CANVAS_H - 300.0) / 2.0;

    match hint {
        "top-left"     => (col(0.0), row(0.0)),
        "top"          => (col(1.0), row(0.0)),
        "top-right"    => (col(2.0), row(0.0)),
        "left"         => (col(0.0), row(1.0)),
        "center"       => (col(1.0), row(1.0)),
        "right"        => (col(2.0), row(1.0)),
        "bottom-left"  => (col(0.0), row(2.0)),
        "bottom"       => (col(1.0), row(2.0)),
        "bottom-right" => (col(2.0), row(2.0)),
        _              => (col(1.0), row(1.0)),
    }
}

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

/// Holds the generated IDs for a cluster so we can draw connections later.
struct ClusterResult {
    name: String,
    boundary_id: String,
}

pub fn generate(manifest: &BoapManifest) -> Scene {
    let mut scene = Scene::new();

    // --- Title block ---
    let title_text = if let Some(ref author) = manifest.meta.author {
        format!("{}\n{}", manifest.meta.title, author)
    } else {
        manifest.meta.title.clone()
    };
    let title_w = builder::estimate_text_width(&manifest.meta.title, TITLE_FONT);
    let title_x = (CANVAS_W - title_w) / 2.0;
    builder::add_text(&mut scene, title_x, TITLE_Y, &title_text, TITLE_FONT, "#1e1e1e");

    // --- Clusters ---
    let mut cluster_results: Vec<ClusterResult> = Vec::new();

    for cluster in &manifest.cluster {
        let style = Style::by_name(&cluster.color);
        let (anchor_x, anchor_y) = hint_to_anchor(&cluster.hint);

        // Calculate cluster dimensions based on items
        let max_item_width = cluster.items.iter()
            .map(|item| builder::estimate_text_width(item, ITEM_FONT))
            .fold(0.0_f64, f64::max);

        let cluster_title_width = builder::estimate_text_width(&cluster.name, CLUSTER_TITLE_FONT);
        let content_width = max_item_width.max(cluster_title_width) + CLUSTER_PAD * 2.0;
        let cluster_w = content_width.max(250.0);

        let items_total_height = cluster.items.len() as f64 * (ITEM_FONT * 1.2 + ITEM_GAP);
        let cluster_h = (CLUSTER_TITLE_FONT * 1.4 + CLUSTER_PAD + items_total_height + CLUSTER_PAD * 2.0).max(120.0);

        // Position: anchor is the centre of the cluster
        let cx = anchor_x - cluster_w / 2.0;
        let cy = anchor_y - cluster_h / 2.0;

        // Boundary rectangle (low-opacity fill)
        let boundary_id = elements::new_id();
        scene.add(Element {
            id: boundary_id.clone(),
            element_type: "rectangle".into(),
            x: cx,
            y: cy,
            width: cluster_w,
            height: cluster_h,
            stroke_color: style.stroke.clone(),
            background_color: style.fill.clone(),
            fill_style: "solid".into(),
            stroke_width: 2.0,
            stroke_style: String::new(),
            roughness: 0,
            opacity: 30,
            font_family: 2,
            font_size: CLUSTER_TITLE_FONT,
            roundness: Some(Roundness { roundness_type: 3 }),
            label: None,
            bound_elements: None,
            text: None,
            original_text: None,
            text_align: None,
            vertical_align: None,
            container_id: None,
            points: None,
            end_arrowhead: None,
            start_arrowhead: None,
            start_binding: None,
            end_binding: None,
            angle: None,
            is_deleted: false,
            custom_data: None,
            group_ids: None,
            simulate_pressure: None,
        });

        // Cluster title text (inside boundary, top)
        let title_x = cx + CLUSTER_PAD;
        let title_y = cy + CLUSTER_PAD;
        builder::add_text(
            &mut scene,
            title_x,
            title_y,
            &cluster.name,
            CLUSTER_TITLE_FONT,
            &style.stroke,
        );

        // Item texts stacked below the title
        let mut item_y = title_y + CLUSTER_TITLE_FONT * 1.4 + CLUSTER_PAD;
        for item in &cluster.items {
            builder::add_text(
                &mut scene,
                cx + CLUSTER_PAD,
                item_y,
                item,
                ITEM_FONT,
                &style.text_color,
            );
            item_y += ITEM_FONT * 1.2 + ITEM_GAP;
        }

        cluster_results.push(ClusterResult {
            name: cluster.name.clone(),
            boundary_id,
        });
    }

    // --- Cross-cluster connections ---
    let arrow_style = Style::arrow();
    for conn in &manifest.connection {
        let from_cr = cluster_results.iter().find(|cr| cr.name == conn.from);
        let to_cr = cluster_results.iter().find(|cr| cr.name == conn.to);

        if let (Some(from_cr), Some(to_cr)) = (from_cr, to_cr) {
            // Determine best connection points based on relative positions
            let from_el = scene.get(&from_cr.boundary_id).unwrap().clone();
            let to_el = scene.get(&to_cr.boundary_id).unwrap().clone();

            let dx = to_el.center_x() - from_el.center_x();
            let dy = to_el.center_y() - from_el.center_y();

            // Pick the side to connect from based on the angle
            let (from_pt, to_pt) = if dx.abs() > dy.abs() {
                if dx > 0.0 {
                    ([1.0, 0.5], [0.0, 0.5]) // right -> left
                } else {
                    ([0.0, 0.5], [1.0, 0.5]) // left -> right
                }
            } else if dy > 0.0 {
                ([0.5, 1.0], [0.5, 0.0]) // bottom -> top
            } else {
                ([0.5, 0.0], [0.5, 1.0]) // top -> bottom
            };

            builder::add_arrow(
                &mut scene,
                &from_cr.boundary_id,
                &to_cr.boundary_id,
                from_pt,
                to_pt,
                &arrow_style,
                conn.label.as_deref(),
            );
        }
    }

    scene
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_MANIFEST: &str = r#"
[meta]
title = "Test Book"

[[cluster]]
name = "Ideas"
hint = "top-left"
color = "blue"
items = ["First idea", "Second idea"]
"#;

    const FULL_MANIFEST: &str = r#"
[meta]
title = "Building a Second Brain"
author = "Tiago Forte"

[[cluster]]
name = "Capture"
hint = "top-left"
color = "blue"
items = ["Keep what resonates", "Use capture tools"]

[[cluster]]
name = "Organize"
hint = "top-right"
color = "green"
items = ["PARA method", "Organize for actionability"]

[[cluster]]
name = "Distill"
hint = "bottom-left"
color = "amber"
items = ["Progressive summarization"]

[[cluster]]
name = "Express"
hint = "bottom-right"
color = "red"
items = ["Create intermediate packets", "Share early"]

[[connection]]
from = "Capture"
to = "Organize"
label = "flows into"

[[connection]]
from = "Organize"
to = "Distill"

[[connection]]
from = "Distill"
to = "Express"

[[connection]]
from = "Express"
to = "Capture"
label = "feeds back"
"#;

    #[test]
    fn parse_minimal_manifest() {
        let m = parse_manifest(MINIMAL_MANIFEST).unwrap();
        assert_eq!(m.meta.title, "Test Book");
        assert_eq!(m.cluster.len(), 1);
        assert_eq!(m.cluster[0].items.len(), 2);
        assert!(m.connection.is_empty());
    }

    #[test]
    fn parse_full_manifest() {
        let m = parse_manifest(FULL_MANIFEST).unwrap();
        assert_eq!(m.meta.title, "Building a Second Brain");
        assert_eq!(m.meta.author.as_deref(), Some("Tiago Forte"));
        assert_eq!(m.cluster.len(), 4);
        assert_eq!(m.connection.len(), 4);
        assert_eq!(m.connection[0].label.as_deref(), Some("flows into"));
        assert!(m.connection[1].label.is_none());
    }

    #[test]
    fn generate_scene_element_count() {
        let m = parse_manifest(FULL_MANIFEST).unwrap();
        let scene = generate(&m);

        // Count element types
        let rects = scene.elements.iter().filter(|e| e.element_type == "rectangle").count();
        let texts = scene.elements.iter().filter(|e| e.element_type == "text").count();
        let arrows = scene.elements.iter().filter(|e| e.element_type == "arrow").count();

        // 4 cluster boundaries
        assert_eq!(rects, 4, "Expected 4 cluster boundary rectangles");
        // 4 connections
        assert_eq!(arrows, 4, "Expected 4 cross-cluster arrows");
        // 1 title + 4 cluster titles + 7 items = 12 texts
        assert_eq!(texts, 12, "Expected 12 text elements");
    }

    #[test]
    fn cluster_positions_respect_hints() {
        let m = parse_manifest(FULL_MANIFEST).unwrap();
        let scene = generate(&m);

        let rects: Vec<&Element> = scene.elements.iter()
            .filter(|e| e.element_type == "rectangle")
            .collect();

        // top-left cluster should have smaller x,y than bottom-right cluster
        let top_left = &rects[0];   // Capture
        let bottom_right = &rects[3]; // Express

        assert!(top_left.center_x() < bottom_right.center_x(),
            "top-left cluster x ({}) should be less than bottom-right cluster x ({})",
            top_left.center_x(), bottom_right.center_x());
        assert!(top_left.center_y() < bottom_right.center_y(),
            "top-left cluster y ({}) should be less than bottom-right cluster y ({})",
            top_left.center_y(), bottom_right.center_y());
    }
}
