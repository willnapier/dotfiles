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
    /// Defaults to ~/.local/share/practiceforge/billing/
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
    /// Uses configured path or defaults to ~/.local/share/practiceforge/billing/
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
            .join("practiceforge")
            .join("billing")
    }
}

// ---------------------------------------------------------------------------
// Setup wizard
// ---------------------------------------------------------------------------

fn prompt(label: &str, default: Option<&str>) -> Result<String> {
    if let Some(d) = default {
        eprint!("{} [{}]: ", label, d);
    } else {
        eprint!("{}: ", label);
    }
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();

    if input.is_empty() {
        match default {
            Some(d) => Ok(d.to_string()),
            None => bail!("Required field"),
        }
    } else {
        Ok(input.to_string())
    }
}

fn prompt_choice(label: &str, options: &[&str], default: &str) -> Result<String> {
    let options_str = options
        .iter()
        .map(|o| {
            if *o == default {
                format!("[{}]", o)
            } else {
                o.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" / ");

    eprint!("{} ({}): ", label, options_str);
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();

    if input.is_empty() {
        return Ok(default.to_string());
    }

    let lower = input.to_lowercase();
    if options.iter().any(|o| o.to_lowercase() == lower) {
        Ok(lower)
    } else {
        eprintln!("  Invalid choice. Using default: {}", default);
        Ok(default.to_string())
    }
}

/// Interactive setup wizard for billing configuration.
pub fn init_billing() -> Result<()> {
    println!("=== Billing Setup ===\n");

    let config_path = crate::config::config_file_path();
    let existing = std::fs::read_to_string(&config_path).unwrap_or_default();

    if existing.contains("[billing]") {
        eprintln!("Billing is already configured in {}", config_path.display());
        let overwrite = prompt("Overwrite existing settings?", Some("no"))?;
        if !overwrite.starts_with('y') && !overwrite.starts_with('Y') {
            println!("Keeping existing settings. Use 'billing config' to view or edit.");
            return Ok(());
        }
    }

    println!("This wizard sets up billing for your practice.\n");

    // Currency
    let currency = prompt("Currency (ISO code)", Some("GBP"))?
        .to_uppercase();

    // Payment terms
    let terms_str = prompt("Payment terms (days after invoice)", Some("14"))?;
    let payment_terms_days: i64 = terms_str
        .parse()
        .context("Invalid number for payment terms")?;

    // Reminder schedule
    println!("\nReminder schedule — when to send payment reminders after the due date.");
    println!("Default: 7, 14, 28 days with escalating tone.");
    let use_defaults = prompt("Use default reminder schedule?", Some("yes"))?;

    let (reminder_days, reminder_tones): (Vec<i64>, Vec<String>) =
        if use_defaults.starts_with('y') || use_defaults.starts_with('Y') {
            (
                vec![7, 14, 28],
                vec![
                    "sensitive".to_string(),
                    "businesslike".to_string(),
                    "assertive".to_string(),
                ],
            )
        } else {
            println!("\nEnter reminder days (comma-separated, e.g. 7,14,28):");
            let days_str = prompt("Reminder days", Some("7,14,28"))?;
            let days: Vec<i64> = days_str
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();

            let mut tones = Vec::new();
            let tone_options = ["sensitive", "tentative", "businesslike", "assertive"];
            for (i, day) in days.iter().enumerate() {
                let default_tone = match i {
                    0 => "sensitive",
                    1 => "businesslike",
                    _ => "assertive",
                };
                let tone = prompt_choice(
                    &format!("  Tone for {}-day reminder", day),
                    &tone_options,
                    default_tone,
                )?;
                tones.push(tone);
            }
            (days, tones)
        };

    // Provider (manual only for now, but ask so the config is future-proof)
    println!("\nAccounting provider — where invoices are stored.");
    println!("  manual: Local files (no API keys needed)");
    println!("  xero:   Xero integration (coming soon)");
    let provider = prompt_choice(
        "Accounting provider",
        &["manual", "xero"],
        "manual",
    )?;

    if provider == "xero" {
        println!("\n  Xero integration requires credentials.");
        println!("  After setup, run: billing xero-setup <client_id> <client_secret>");
        println!("  Then: billing xero-auth");
    }

    let provider_final = provider;

    // Payment provider
    println!("\nPayment provider — how self-pay clients pay.");
    println!("  manual: Bank transfer (you send details in the reminder)");
    println!("  stripe: Stripe payment links");
    let payment_provider = prompt_choice(
        "Payment provider",
        &["manual", "stripe"],
        "manual",
    )?;

    if payment_provider == "stripe" {
        println!("\n  Stripe requires a secret key.");
        println!("  After setup, run: billing stripe-key <sk_live_...>");
    }

    let payment_final = payment_provider;

    // Build the TOML section
    let reminder_days_str = reminder_days
        .iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join(", ");

    let reminder_tones_str = reminder_tones
        .iter()
        .map(|t| format!("\"{}\"", t))
        .collect::<Vec<_>>()
        .join(", ");

    let billing_section = format!(
        "\n[billing]\n\
         enabled = true\n\
         provider = \"{}\"\n\
         payment_provider = \"{}\"\n\
         payment_terms_days = {}\n\
         currency = \"{}\"\n\
         reminder_days = [{}]\n\
         reminder_tones = [{}]\n",
        provider_final,
        payment_final,
        payment_terms_days,
        currency,
        reminder_days_str,
        reminder_tones_str,
    );

    // Write config
    if existing.contains("[billing]") {
        // Remove existing [billing] section
        let mut new_content = String::new();
        let mut in_billing = false;
        for line in existing.lines() {
            if line.trim() == "[billing]" {
                in_billing = true;
                continue;
            }
            if in_billing && line.starts_with('[') && line.contains(']') {
                in_billing = false;
            }
            if !in_billing {
                new_content.push_str(line);
                new_content.push('\n');
            }
        }
        std::fs::write(&config_path, &new_content)?;
    }

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&config_path)?;
    file.write_all(billing_section.as_bytes())?;

    println!("\n✓ Billing enabled in {}", config_path.display());
    println!("\nYou can now:");
    println!("  practiceforge billing status        — check invoice status");
    println!("  practiceforge billing invoice <ID>  — create an invoice");
    println!("  practiceforge billing config        — view/edit settings");

    Ok(())
}

// ---------------------------------------------------------------------------
// Config viewer/editor
// ---------------------------------------------------------------------------

/// Display current billing settings.
pub fn show_config() -> Result<()> {
    let config = BillingConfig::load()?;
    let config_path = crate::config::config_file_path();

    println!("Billing settings (from {}):\n", config_path.display());
    println!("  enabled            = {}", config.enabled);
    println!("  provider           = {}", config.provider);
    println!("  payment_provider   = {}", config.payment_provider);
    println!("  payment_terms_days = {}", config.payment_terms_days);
    println!("  currency           = {}", config.currency);
    println!(
        "  reminder_days      = [{}]",
        config
            .reminder_days
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "  reminder_tones     = [{}]",
        config
            .reminder_tones
            .iter()
            .map(|t| format!("\"{}\"", t))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if let Some(dir) = &config.storage_dir {
        println!("  storage_dir        = {}", dir);
    } else {
        println!(
            "  storage_dir        = {} (default)",
            config.resolve_storage_dir().display()
        );
    }

    if !config.enabled {
        println!("\n  Billing is disabled. Run 'billing init' to set up.");
    }

    Ok(())
}

/// Update a single billing setting in config.toml.
///
/// Accepts "key=value" format. Validates the key and value before writing.
pub fn update_config(setting: &str) -> Result<()> {
    let parts: Vec<&str> = setting.splitn(2, '=').collect();
    if parts.len() != 2 {
        bail!("Expected format: key=value (e.g. payment_terms_days=21)");
    }

    let key = parts[0].trim();
    let value = parts[1].trim();

    // Validate key
    let valid_keys = [
        "enabled",
        "provider",
        "payment_provider",
        "payment_terms_days",
        "currency",
        "reminder_days",
        "reminder_tones",
        "storage_dir",
    ];

    if !valid_keys.contains(&key) {
        bail!(
            "Unknown setting '{}'. Valid: {}",
            key,
            valid_keys.join(", ")
        );
    }

    // Validate value by type
    let toml_value = match key {
        "enabled" => {
            match value.to_lowercase().as_str() {
                "true" | "yes" | "1" => "true".to_string(),
                "false" | "no" | "0" => "false".to_string(),
                _ => bail!("enabled must be true or false"),
            }
        }
        "provider" => {
            if !["manual", "xero"].contains(&value) {
                bail!("provider must be 'manual' or 'xero'");
            }
            format!("\"{}\"", value)
        }
        "payment_provider" => {
            if !["manual", "stripe"].contains(&value) {
                bail!("payment_provider must be 'manual' or 'stripe'");
            }
            format!("\"{}\"", value)
        }
        "payment_terms_days" => {
            let _: i64 = value.parse().context("payment_terms_days must be a number")?;
            value.to_string()
        }
        "currency" => {
            if value.len() != 3 {
                bail!("currency must be a 3-letter ISO code (e.g. GBP, USD, EUR)");
            }
            format!("\"{}\"", value.to_uppercase())
        }
        "reminder_days" => {
            // Parse comma-separated ints
            let days: Vec<i64> = value
                .split(',')
                .map(|s| s.trim().parse::<i64>().context("reminder_days must be comma-separated numbers"))
                .collect::<Result<Vec<_>>>()?;
            format!(
                "[{}]",
                days.iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        "reminder_tones" => {
            let valid_tones = ["sensitive", "tentative", "businesslike", "assertive"];
            let tones: Vec<&str> = value.split(',').map(|s| s.trim()).collect();
            for t in &tones {
                if !valid_tones.contains(t) {
                    bail!(
                        "Invalid tone '{}'. Valid: {}",
                        t,
                        valid_tones.join(", ")
                    );
                }
            }
            format!(
                "[{}]",
                tones
                    .iter()
                    .map(|t| format!("\"{}\"", t))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        "storage_dir" => format!("\"{}\"", value),
        _ => unreachable!(),
    };

    // Read, modify, write
    let config_path = crate::config::config_file_path();
    let content = std::fs::read_to_string(&config_path).unwrap_or_default();

    if !content.contains("[billing]") {
        bail!(
            "No [billing] section in config. Run 'billing init' first."
        );
    }

    // Find and replace the key line, or append it to the [billing] section
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    let mut found = false;
    let mut in_billing = false;
    let mut last_billing_line = 0;

    for (i, line) in lines.iter_mut().enumerate() {
        if line.trim() == "[billing]" {
            in_billing = true;
            last_billing_line = i;
            continue;
        }
        if in_billing {
            if line.starts_with('[') && line.contains(']') {
                in_billing = false;
                continue;
            }
            last_billing_line = i;
            if line.starts_with(key) && line.contains('=') {
                *line = format!("{} = {}", key, toml_value);
                found = true;
                break;
            }
        }
    }

    if !found {
        // Insert after the last line of the [billing] section
        lines.insert(last_billing_line + 1, format!("{} = {}", key, toml_value));
    }

    let new_content = lines.join("\n") + "\n";
    std::fs::write(&config_path, new_content)?;

    println!("✓ {} = {}", key, toml_value);
    Ok(())
}
