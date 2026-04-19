//! Secure credential storage for billing API providers.
//!
//! Stored in `~/.config/practiceforge/secrets.toml` (separate from config.toml
//! so it can be chmod 600). This file should never be committed to version control.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Path to the secrets file.
pub fn secrets_path() -> PathBuf {
    crate::config::config_dir().join("secrets.toml")
}

/// Top-level secrets container.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BillingSecrets {
    #[serde(default)]
    pub xero: XeroSecrets,
    #[serde(default)]
    pub stripe: StripeSecrets,
}

/// Xero OAuth2 credentials and tokens.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct XeroSecrets {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub tenant_id: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    /// RFC3339 expiry timestamp, e.g. "2026-04-19T15:00:00Z"
    pub token_expires_at: Option<String>,
}

/// Stripe API credentials.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StripeSecrets {
    pub secret_key: Option<String>,
}

impl BillingSecrets {
    /// Load from secrets.toml, or return a default (all None) if the file
    /// doesn't exist yet.
    pub fn load() -> Result<Self> {
        let path = secrets_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("Cannot read secrets file: {}", path.display()))?;
        let secrets: BillingSecrets =
            toml::from_str(&data).context("Failed to parse secrets.toml")?;
        Ok(secrets)
    }

    /// Write to secrets.toml and set permissions to 600.
    pub fn save(&self) -> Result<()> {
        let path = secrets_path();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Cannot create config dir: {}", parent.display()))?;
        }

        let data = toml::to_string_pretty(self).context("Failed to serialise secrets")?;
        std::fs::write(&path, &data)
            .with_context(|| format!("Cannot write secrets file: {}", path.display()))?;

        // chmod 600 — owner read/write only
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&path, perms)
                .with_context(|| format!("Cannot set permissions on {}", path.display()))?;
        }

        Ok(())
    }
}
