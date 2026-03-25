use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Generate a unique element ID.
pub fn new_id() -> String {
    Uuid::new_v4().to_string().replace('-', "")[..20].to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Element {
    pub id: String,
    #[serde(rename = "type")]
    pub element_type: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,

    #[serde(default = "default_stroke_color")]
    pub stroke_color: String,
    #[serde(default = "default_bg")]
    pub background_color: String,
    #[serde(default = "default_fill_style")]
    pub fill_style: String,
    #[serde(default = "default_stroke_width")]
    pub stroke_width: f64,
    #[serde(default)]
    pub stroke_style: String,
    #[serde(default = "default_roughness")]
    pub roughness: i32,
    #[serde(default = "default_opacity")]
    pub opacity: i32,
    #[serde(default = "default_font_family")]
    pub font_family: i32,
    #[serde(default)]
    pub font_size: f64,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_align: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vertical_align: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub points: Option<Vec<[f64; 2]>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_arrowhead: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_arrowhead: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_binding: Option<Binding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_binding: Option<Binding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bound_elements: Option<Vec<BoundElement>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub roundness: Option<Roundness>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub angle: Option<f64>,

    #[serde(default)]
    pub is_deleted: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<Label>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    pub element_id: String,
    pub fixed_point: [f64; 2],
    #[serde(default)]
    pub focus: f64,
    #[serde(default)]
    pub gap: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundElement {
    pub id: String,
    #[serde(rename = "type")]
    pub bound_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Roundness {
    #[serde(rename = "type")]
    pub roundness_type: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    pub text: String,
    #[serde(default = "default_label_font_size")]
    pub font_size: f64,
    #[serde(default = "default_font_family")]
    pub font_family: i32,
}

fn default_stroke_color() -> String { "#1e1e1e".into() }
fn default_bg() -> String { "transparent".into() }
fn default_fill_style() -> String { "solid".into() }
fn default_stroke_width() -> f64 { 2.0 }
fn default_roughness() -> i32 { 1 }
fn default_opacity() -> i32 { 100 }
fn default_font_family() -> i32 { 1 } // Excalifont
fn default_label_font_size() -> f64 { 20.0 }

impl Element {
    pub fn center_x(&self) -> f64 { self.x + self.width / 2.0 }
    pub fn center_y(&self) -> f64 { self.y + self.height / 2.0 }
    pub fn right(&self) -> f64 { self.x + self.width }
    pub fn bottom(&self) -> f64 { self.y + self.height }
}
