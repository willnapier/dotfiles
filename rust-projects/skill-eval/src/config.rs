use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Clone)]
pub struct Assertion {
    pub id: String,
    #[serde(rename = "assert")]
    pub assert_text: String,
    pub category: String,
    #[serde(default)]
    pub layer: Option<u8>,
    /// Optional condition — if present, assertion only applies when condition is met.
    /// If condition is not met in the log, result is NotApplicable.
    #[serde(default)]
    pub condition: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Scenario {
    pub id: String,
    pub prompt: String,
    pub exercises: Vec<String>,
    #[serde(default)]
    pub description: String,
}

/// Resolve the skill directory path
pub fn skill_dir(skill_name: &str) -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    let path = home.join(".claude").join("skills").join(skill_name);
    if !path.exists() {
        anyhow::bail!("Skill directory not found: {}", path.display());
    }
    Ok(path)
}

/// Load all assertions (universal + skill-specific) from eval/ directory
pub fn load_all_assertions(skill_dir: &PathBuf) -> Result<Vec<Assertion>> {
    let eval_dir = skill_dir.join("eval");
    if !eval_dir.exists() {
        anyhow::bail!("No eval/ directory found in {}", skill_dir.display());
    }

    let mut assertions = Vec::new();

    // Load universal assertions
    let universal_path = eval_dir.join("assertions-universal.json");
    if universal_path.exists() {
        let content = std::fs::read_to_string(&universal_path)
            .context("Failed to read assertions-universal.json")?;
        let universal: Vec<Assertion> =
            serde_json::from_str(&content).context("Failed to parse assertions-universal.json")?;
        assertions.extend(universal);
    }

    // Load skill-specific assertions
    let skill_path = eval_dir.join("assertions-skill.json");
    if skill_path.exists() {
        let content = std::fs::read_to_string(&skill_path)
            .context("Failed to read assertions-skill.json")?;
        let skill_assertions: Vec<Assertion> =
            serde_json::from_str(&content).context("Failed to parse assertions-skill.json")?;
        assertions.extend(skill_assertions);
    }

    if assertions.is_empty() {
        anyhow::bail!("No assertions found in {}", eval_dir.display());
    }

    Ok(assertions)
}

/// Load test scenarios
pub fn load_scenarios(skill_dir: &PathBuf) -> Result<Vec<Scenario>> {
    let path = skill_dir.join("eval").join("scenarios.json");
    if !path.exists() {
        anyhow::bail!("No scenarios.json found in {}", skill_dir.display());
    }

    let content =
        std::fs::read_to_string(&path).context("Failed to read scenarios.json")?;
    let scenarios: Vec<Scenario> =
        serde_json::from_str(&content).context("Failed to parse scenarios.json")?;

    Ok(scenarios)
}
