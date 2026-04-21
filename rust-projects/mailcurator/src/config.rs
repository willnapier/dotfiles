// Config loading — parses ~/.config/mailcurator/policies.toml into typed structs.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::Path;

use crate::policy::Policy;

#[derive(Deserialize)]
pub struct Config {
    #[serde(default, rename = "policy")]
    pub policies: Vec<Policy>,
}

pub fn load(path: &Path) -> Result<Config> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let cfg: Config = toml::from_str(&text)
        .with_context(|| format!("parsing {}", path.display()))?;

    // Validate names are unique (needed for the curator-<name>-seen tag convention)
    let mut names = std::collections::HashSet::new();
    for p in &cfg.policies {
        if !names.insert(&p.name) {
            anyhow::bail!("duplicate policy name: {}", p.name);
        }
        p.validate()
            .with_context(|| format!("policy '{}' is invalid", p.name))?;
    }

    Ok(cfg)
}
