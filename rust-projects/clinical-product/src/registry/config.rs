use anyhow::{Context, Result};
use std::io::{self, Write};
use std::path::PathBuf;

/// Configuration for the central client registry.
#[derive(Debug, Clone)]
pub struct RegistryConfig {
    pub enabled: bool,
    pub local_path: PathBuf,
    pub remote_url: String,
    pub auto_sync: bool,
    pub sync_interval_minutes: u32,
    pub practitioner_id: String,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        Self {
            enabled: false,
            local_path: home.join("Clinical").join("registry"),
            remote_url: String::new(),
            auto_sync: true,
            sync_interval_minutes: 15,
            practitioner_id: String::new(),
        }
    }
}

impl RegistryConfig {
    /// Load registry config from the [registry] section of config.toml.
    pub fn load() -> Self {
        let config = crate::config::load_config();
        let Some(config) = config else {
            return Self::default();
        };
        let Some(section) = config.get("registry") else {
            return Self::default();
        };

        let home = dirs::home_dir().unwrap_or_default();
        let default_path = home.join("Clinical").join("registry");

        let local_path = section
            .get("local_path")
            .and_then(|v| v.as_str())
            .map(|s| {
                let expanded = shellexpand::tilde(s);
                PathBuf::from(expanded.as_ref())
            })
            .unwrap_or(default_path);

        Self {
            enabled: section
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            local_path,
            remote_url: section
                .get("remote_url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            auto_sync: section
                .get("auto_sync")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            sync_interval_minutes: section
                .get("sync_interval_minutes")
                .and_then(|v| v.as_integer())
                .unwrap_or(15) as u32,
            practitioner_id: section
                .get("practitioner_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        }
    }

    /// Resolve a relative path within the registry working copy.
    pub fn resolve_path(&self, relative: &str) -> PathBuf {
        self.local_path.join(relative)
    }

    /// Path to the clients directory in the registry.
    pub fn clients_dir(&self) -> PathBuf {
        self.local_path.join("clients")
    }

    /// Path to a specific client's directory in the registry.
    pub fn client_dir(&self, client_id: &str) -> PathBuf {
        self.clients_dir().join(client_id)
    }

    /// Path to the config directory in the registry.
    pub fn config_dir(&self) -> PathBuf {
        self.local_path.join("config")
    }
}

/// Interactive setup wizard for the registry.
pub fn init_registry() -> Result<()> {
    println!("=== PracticeForge Registry Setup ===\n");

    let practitioner_id = prompt("Practitioner ID (slug, e.g. 'william')", Some("william"))?;
    let local_path = prompt(
        "Local registry path",
        Some("~/Clinical/registry"),
    )?;
    let remote_url = prompt(
        "Remote git URL (leave empty for local-only)",
        Some(""),
    )?;

    // Write to config.toml
    let config_path = crate::config::config_file_path();
    let mut contents = std::fs::read_to_string(&config_path).unwrap_or_default();

    let section = format!(
        "\n[registry]\nenabled = true\nlocal_path = \"{}\"\nremote_url = \"{}\"\nauto_sync = true\nsync_interval_minutes = 15\npractitioner_id = \"{}\"\n",
        local_path, remote_url, practitioner_id
    );

    if contents.contains("[registry]") {
        // Replace existing section — find [registry] and replace until next section or EOF
        if let Some(start) = contents.find("[registry]") {
            let rest = &contents[start..];
            let end = rest[1..]
                .find("\n[")
                .map(|i| start + 1 + i)
                .unwrap_or(contents.len());
            contents.replace_range(start..end, section.trim());
        }
    } else {
        contents.push_str(&section);
    }

    std::fs::write(&config_path, &contents)
        .context("Failed to write config.toml")?;

    println!("\nRegistry config saved to {}", config_path.display());
    println!("Run `clinical-product registry init` to create the repository.");

    Ok(())
}

fn prompt(label: &str, default: Option<&str>) -> Result<String> {
    if let Some(d) = default {
        if !d.is_empty() {
            print!("{} [{}]: ", label, d);
        } else {
            print!("{}: ", label);
        }
    } else {
        print!("{}: ", label);
    }
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim().to_string();

    if input.is_empty() {
        Ok(default.unwrap_or("").to_string())
    } else {
        Ok(input)
    }
}
