use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    pub settings: Settings,
    #[serde(rename = "capture")]
    pub captures: Vec<Capture>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Settings {
    pub state_dir: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Capture {
    pub name: String,
    pub command: String,
    pub output: String,
    #[serde(default)]
    pub sort: bool,
}

/// Expand ~ to the user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

/// Default config path: ~/.config/state-capture/config.toml
pub fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("state-capture")
        .join("config.toml")
}

pub fn load_config(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config: {}", path.display()))?;
    let config: Config =
        toml::from_str(&content).with_context(|| "Failed to parse config TOML")?;
    validate(&config)?;
    Ok(config)
}

fn validate(config: &Config) -> Result<()> {
    if config.captures.is_empty() {
        anyhow::bail!("Config has no [[capture]] entries");
    }
    for cap in &config.captures {
        if cap.name.is_empty() {
            anyhow::bail!("Capture entry has empty name");
        }
        if cap.command.is_empty() {
            anyhow::bail!("Capture '{}' has empty command", cap.name);
        }
        if cap.output.is_empty() {
            anyhow::bail!("Capture '{}' has empty output filename", cap.name);
        }
    }
    Ok(())
}

impl Config {
    pub fn state_dir(&self) -> PathBuf {
        expand_tilde(&self.settings.state_dir)
    }
}
