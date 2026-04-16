//! Search configuration — reads [search] section from config.toml.

use std::path::PathBuf;

/// Search configuration from config.toml [search] section.
#[derive(Debug, Clone)]
pub struct SearchConfig {
    /// Whether search is enabled.
    pub enabled: bool,
    /// Path to the Tantivy index directory.
    pub index_path: PathBuf,
    /// Whether to index session notes (notes.md).
    pub include_notes: bool,
    /// Whether to index correspondence/ files.
    pub include_correspondence: bool,
}

impl Default for SearchConfig {
    fn default() -> Self {
        let data_dir = dirs::home_dir()
            .unwrap_or_default()
            .join(".local")
            .join("share")
            .join("practiceforge")
            .join("search-index");
        Self {
            enabled: true,
            index_path: data_dir,
            include_notes: true,
            include_correspondence: true,
        }
    }
}

impl SearchConfig {
    /// Load search config from the [search] section of config.toml.
    /// Returns default (enabled) config if section is missing.
    pub fn load() -> Self {
        let config = crate::config::load_config();
        let section = config
            .as_ref()
            .and_then(|c| c.get("search"));

        let Some(section) = section else {
            return Self::default();
        };

        let default = Self::default();

        let index_path = section
            .get("index_path")
            .and_then(|v| v.as_str())
            .map(|s| {
                let expanded = shellexpand::tilde(s);
                PathBuf::from(expanded.as_ref())
            })
            .unwrap_or(default.index_path);

        Self {
            enabled: section
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            index_path,
            include_notes: section
                .get("include_notes")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            include_correspondence: section
                .get("include_correspondence")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
        }
    }
}
