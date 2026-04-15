//! Billing configuration — reads [billing] section from config.toml.
//!
//! Also provides the setup wizard (`init_billing`) and settings
//! viewer/editor (`show_config`, `update_config`).

use anyhow::{bail, Context, Result};
use std::io::{self, Write};

/// Billing configuration from config.toml [billing] section.
#[derive(Debug, Clone)]
pub struct BillingConfig {
    /// Whether billing is enabled at all.
    pub enabled: bool,
    /// Which accounting backend to use: "manual", "xero" (future).
    pub provider: String,
    /// Which payment backend to use: "manual", "stripe" (future).
    pub payment_provider: String,
    /// Days after issue date before invoice is due.
    pub payment_terms_days: i64,
    /// Days overdue at which to send each reminder.
    /// e.g. [7, 14, 28] means reminders at 7, 14, and 28 days overdue.
    pub reminder_days: Vec<i64>,
    /// Tone preset for each reminder stage.
    /// e.g. ["sensitive", "businesslike", "assertive"]
    /// Must be same length as reminder_days.
    pub reminder_tones: Vec<String>,
    /// Currency code (ISO 4217).
    pub currency: String,
    /// Directory for invoice storage (Manual backend).
    /// Defaults to ~/.local/share/clinical-product/billing/
    pub storage_dir: Option<String>,
}

impl Default for BillingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "manual".to_string(),
            payment_provider: "manual".to_string(),
            payment_terms_days: 14,
            reminder_days: vec![7, 14, 28],
            reminder_tones: vec![
                "sensitive".to_string(),
                "businesslike".to_string(),
                "assertive".to_string(),
            ],
            currency: "GBP".to_string(),
            storage_dir: None,
        }
    }
}

impl BillingConfig {
    /// Load billing config from the [billing] section of config.toml.
    /// Returns default (disabled) config if section is missing.
    pub fn load() -> Result<Self> {
        let config = crate::config::load_config();
        let billing = config
            .as_ref()
            .and_then(|c| c.get("billing"))
            .and_then(|v| v.as_table());

        let Some(billing) = billing else {
            return Ok(Self::default());
        };

        let enabled = billing
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let provider = billing
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("manual")
            .to_string();

        let payment_provider = billing
            .get("payment_provider")
            .and_then(|v| v.as_str())
            .unwrap_or("manual")
            .to_string();

        let payment_terms_days = billing
            .get("payment_terms_days")
            .and_then(|v| v.as_integer())
            .unwrap_or(14);

        let reminder_days = billing
            .get("reminder_days")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_integer())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![7, 14, 28]);

        let reminder_tones = billing
            .get("reminder_tones")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                vec![
                    "sensitive".to_string(),
                    "businesslike".to_string(),
                    "assertive".to_string(),
                ]
            });

        let currency = billing
            .get("currency")
            .and_then(|v| v.as_str())
            .unwrap_or("GBP")
            .to_string();

        let storage_dir = billing
            .get("storage_dir")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(Self {
            enabled,
            provider,
            payment_provider,
            payment_terms_days,
            reminder_days,
            reminder_tones,
            currency,
            storage_dir,
        })
    }

    /// Resolve the storage directory for invoice files.
    /// Uses configured path or defaults to ~/.local/share/clinical-product/billing/
    pub fn resolve_storage_dir(&self) -> std::path::PathBuf {
        if let Some(dir) = &self.storage_dir {
            let expanded = if dir.starts_with("~/") {
                if let Some(home) = dirs::home_dir() {
                    home.join(&dir[2..])
                } else {
                    std::path::PathBuf::from(dir)
                }
            } else {
                std::path::PathBuf::from(dir)
            };
            return expanded;
        }

        dirs::data_local_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".local/share"))
            .join("clinical-product")
            .join("billing")
    }
}
