use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

fn config_path() -> PathBuf {
    dirs::home_dir()
        .expect("could not find home directory")
        .join(".bequest/config.toml")
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub trustees: Vec<Trustee>,
    #[serde(default)]
    pub enrolment: Option<Enrolment>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_threshold")]
    pub threshold_days: u64,
    #[serde(default = "default_grace")]
    pub grace_days: u64,
    #[serde(default)]
    pub from_email: Option<String>,
    /// Email addresses to warn William during grace period
    #[serde(default)]
    pub warning_emails: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            threshold_days: 14,
            grace_days: 7,
            from_email: None,
            warning_emails: Vec::new(),
        }
    }
}

fn default_threshold() -> u64 {
    14
}
fn default_grace() -> u64 {
    7
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Trustee {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Enrolment {
    pub threshold: u8,
    pub shares: u8,
    pub enrolled_at: String,
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path();
        if !path.exists() {
            return Ok(Config {
                settings: Settings::default(),
                trustees: Vec::new(),
                enrolment: None,
            });
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&content).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        fs::create_dir_all(path.parent().unwrap())?;
        let content = toml::to_string_pretty(self).context("serializing config")?;
        fs::write(&path, content).with_context(|| format!("writing {}", path.display()))
    }
}
