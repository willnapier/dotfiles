//! Per-practitioner identity config for invoice letterheads.
//!
//! Loaded from [practitioner] section of config.toml.
//! All fields optional except company_name — missing fields are omitted
//! from the invoice rather than causing an error.
//!
//! Example config.toml:
//!
//!   [practitioner]
//!   company_name   = "Napier Psychology Ltd"
//!   trading_name   = "William Napier, Chartered Psychologist"
//!   address        = "37 Gloucester Place, London W1U 8JB"
//!   phone          = "+44 7700 900000"
//!   email          = "will@willnapier.com"
//!   company_reg    = "12345678"
//!   vat_number     = ""          # omit if not VAT registered
//!   bank_name      = "HSBC"
//!   sort_code      = "40-12-34"
//!   account_number = "12345678"
//!   account_name   = "Napier Psychology Ltd"

#[derive(Debug, Clone, Default)]
pub struct PractitionerConfig {
    /// Legal company name (e.g. "Napier Psychology Ltd").
    pub company_name: String,
    /// Trading / display name shown on letterhead (e.g. "William Napier, Chartered Psychologist").
    /// Falls back to company_name if absent.
    pub trading_name: Option<String>,
    /// Full address for invoice header.
    pub address: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    /// Companies House registration number.
    pub company_reg: Option<String>,
    /// VAT registration number — omit if not VAT registered.
    pub vat_number: Option<String>,
    // Bank transfer details
    pub bank_name: Option<String>,
    pub sort_code: Option<String>,
    pub account_number: Option<String>,
    pub account_name: Option<String>,
}

impl PractitionerConfig {
    /// Load from [practitioner] section of config.toml.
    /// Returns a mostly-empty config rather than an error if section is missing.
    pub fn load() -> Self {
        let mut cfg = Self::default();
        let Some(config) = crate::config::load_config() else {
            return cfg;
        };
        let Some(p) = config.get("practitioner") else {
            return cfg;
        };

        macro_rules! str_field {
            ($field:ident, $key:literal) => {
                if let Some(v) = p.get($key).and_then(|v| v.as_str()) {
                    if !v.is_empty() {
                        cfg.$field = Some(v.to_string());
                    }
                }
            };
        }

        if let Some(v) = p.get("company_name").and_then(|v| v.as_str()) {
            if !v.is_empty() {
                cfg.company_name = v.to_string();
            }
        }
        str_field!(trading_name, "trading_name");
        str_field!(address, "address");
        str_field!(phone, "phone");
        str_field!(email, "email");
        str_field!(company_reg, "company_reg");
        str_field!(vat_number, "vat_number");
        str_field!(bank_name, "bank_name");
        str_field!(sort_code, "sort_code");
        str_field!(account_number, "account_number");
        str_field!(account_name, "account_name");

        cfg
    }

    /// The name to display on the letterhead.
    pub fn display_name(&self) -> &str {
        self.trading_name
            .as_deref()
            .unwrap_or(&self.company_name)
    }

    pub fn has_bank_details(&self) -> bool {
        self.sort_code.is_some() && self.account_number.is_some()
    }

    pub fn is_configured(&self) -> bool {
        !self.company_name.is_empty()
    }
}
