use serde::{Deserialize, Serialize};
use crate::elements::Element;
use anyhow::{Context, Result};
use std::path::Path;

/// A complete Excalidraw scene/document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scene {
    #[serde(rename = "type")]
    pub scene_type: String,
    pub version: i32,
    pub source: String,
    pub elements: Vec<Element>,
    #[serde(rename = "appState")]
    pub app_state: AppState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub grid_size: Option<i32>,
    pub view_background_color: String,
}

impl Scene {
    /// Create a new empty scene.
    pub fn new() -> Self {
        Scene {
            scene_type: "excalidraw".into(),
            version: 2,
            source: "easel".into(),
            elements: Vec::new(),
            app_state: AppState {
                grid_size: None,
                view_background_color: "#ffffff".into(),
            },
        }
    }

    /// Load from an .excalidraw file.
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read: {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse: {}", path.display()))
    }

    /// Save to an .excalidraw file.
    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)
            .with_context(|| format!("Failed to write: {}", path.display()))
    }

    /// Add an element to the scene.
    pub fn add(&mut self, element: Element) -> String {
        let id = element.id.clone();
        self.elements.push(element);
        id
    }

    /// Find an element by ID.
    pub fn get(&self, id: &str) -> Option<&Element> {
        self.elements.iter().find(|e| e.id == id)
    }

    /// Find an element by ID (mutable).
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Element> {
        self.elements.iter_mut().find(|e| e.id == id)
    }

    /// Get the last added element's ID.
    pub fn last_id(&self) -> Option<&str> {
        self.elements.last().map(|e| e.id.as_str())
    }
}
