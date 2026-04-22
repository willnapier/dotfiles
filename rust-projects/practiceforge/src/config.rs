//! Shared configuration for practiceforge.
//!
//! Config file: `~/.config/practiceforge/config.toml`
//! Falls back to `voice-config.toml` for backward compatibility.

use std::path::PathBuf;

/// The config directory.
///
/// On macOS and Linux: `~/.config/practiceforge/`. Kept as the literal
/// `~/.config` subdirectory rather than `dirs::config_dir()` so existing
/// installs (which all live there) don't have to migrate to
/// `~/Library/Application Support` on macOS.
///
/// On Windows: `%APPDATA%\practiceforge\` via `dirs::config_dir()`. There
/// are no existing Windows installs to migrate, and `%APPDATA%` is the
/// idiomatic location a Windows user would look for application config.
pub fn config_dir() -> PathBuf {
    if cfg!(target_os = "windows") {
        dirs::config_dir()
            .expect("no config dir")
            .join("practiceforge")
    } else {
        dirs::home_dir()
            .expect("no home dir")
            .join(".config")
            .join("practiceforge")
    }
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
    // Default fallback when no `[paths] clinical_root` is set. On Windows,
    // prefer `~/Documents/Clinical` (the conventional location for user
    // documents); on macOS/Linux keep the existing `~/Clinical`.
    if cfg!(target_os = "windows") {
        dirs::document_dir()
            .or_else(dirs::home_dir)
            .expect("no home dir")
            .join("Clinical")
    } else {
        dirs::home_dir().expect("no home dir").join("Clinical")
    }
}

/// Clients directory: `{clinical_root}/clients/`
pub fn clients_dir() -> PathBuf {
    clinical_root().join("clients")
}

/// Attendance directory: `{clinical_root}/attendance/`
pub fn attendance_dir() -> PathBuf {
    clinical_root().join("attendance")
}

/// AI configuration from `[ai]` section in config.toml.
#[derive(Debug, Default)]
pub struct AiConfig {
    /// e.g. "anthropic" or "ollama"
    pub backend: Option<String>,
    /// Model name (provider-specific)
    pub model: Option<String>,
}

/// Load the `[ai]` section from config.toml.
pub fn load_ai_config() -> AiConfig {
    let Some(config) = load_config() else {
        return AiConfig::default();
    };
    let Some(ai) = config.get("ai") else {
        return AiConfig::default();
    };
    AiConfig {
        backend: ai.get("backend").and_then(|v| v.as_str()).map(str::to_owned),
        model: ai.get("model").and_then(|v| v.as_str()).map(str::to_owned),
    }
}
