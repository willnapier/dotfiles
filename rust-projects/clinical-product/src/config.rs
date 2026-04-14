//! Shared configuration for clinical-product.
//!
//! Config file: `~/.config/clinical-product/config.toml`
//! Falls back to `voice-config.toml` for backward compatibility.

use std::path::PathBuf;

/// The config directory: `~/.config/clinical-product/`
pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .expect("no home dir")
        .join(".config")
        .join("clinical-product")
}

/// Path to the config file. Prefers `config.toml`, falls back to
/// `voice-config.toml` for backward compatibility.
pub fn config_file_path() -> PathBuf {
    let dir = config_dir();
    let preferred = dir.join("config.toml");
    if preferred.exists() {
        return preferred;
    }
    let legacy = dir.join("voice-config.toml");
    if legacy.exists() {
        return legacy;
    }
    // Default to preferred name (will be created on first write)
    preferred
}

/// Load the full config TOML as a `toml::Value`.
pub fn load_config() -> Option<toml::Value> {
    let path = config_file_path();
    let data = std::fs::read_to_string(&path).ok()?;
    toml::from_str(&data).ok()
}

/// Root directory for clinical data.
///
/// Reads `[paths] clinical_root` from config. Falls back to `~/Clinical`.
pub fn clinical_root() -> PathBuf {
    if let Some(config) = load_config() {
        if let Some(paths) = config.get("paths") {
            if let Some(root) = paths.get("clinical_root").and_then(|v| v.as_str()) {
                // Expand ~ to home dir
                if root.starts_with("~/") {
                    if let Some(home) = dirs::home_dir() {
                        return home.join(&root[2..]);
                    }
                }
                return PathBuf::from(root);
            }
        }
    }
    dirs::home_dir().expect("no home dir").join("Clinical")
}

/// Clients directory: `{clinical_root}/clients/`
pub fn clients_dir() -> PathBuf {
    clinical_root().join("clients")
}

/// Attendance directory: `{clinical_root}/attendance/`
pub fn attendance_dir() -> PathBuf {
    clinical_root().join("attendance")
}
