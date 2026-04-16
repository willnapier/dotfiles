//! SMS configuration — reads `[sms]` section from config.toml.
//!
//! Supports Twilio as the provider. Auth token can be stored directly
//! in config (for testing) or referenced via a keychain service name
//! (for production — actual keychain read is deferred to a later phase).

/// SMS configuration from config.toml `[sms]` section.
#[derive(Debug, Clone)]
pub struct SmsConfig {
    /// Whether SMS reminders are enabled.
    pub enabled: bool,
    /// SMS provider (currently only "twilio").
    pub provider: String,
    /// Twilio account SID.
    pub twilio_account_sid: String,
    /// Twilio auth token (direct, for testing).
    pub twilio_auth_token: String,
    /// Keychain service name for the auth token (future use).
    pub twilio_auth_token_keychain: String,
    /// Twilio phone number to send from (E.164 format).
    pub twilio_from_number: String,
    /// How many hours before the appointment to send reminders.
    pub reminder_hours_before: u32,
    /// Whether to track confirmation replies.
    pub confirmation_enabled: bool,
    /// Practitioner name for the reminder message.
    pub practitioner_name: String,
    /// Practice phone number for the "please call to reschedule" line.
    pub practice_phone: String,
}

impl Default for SmsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "twilio".to_string(),
            twilio_account_sid: String::new(),
            twilio_auth_token: String::new(),
            twilio_auth_token_keychain: "clinical-twilio".to_string(),
            twilio_from_number: String::new(),
            reminder_hours_before: 24,
            confirmation_enabled: false,
            practitioner_name: String::new(),
            practice_phone: String::new(),
        }
    }
}

impl SmsConfig {
    /// Load SMS config from the `[sms]` section of config.toml.
    /// Returns default (disabled) config if the section is missing.
    pub fn load() -> Self {
        let config = crate::config::load_config();
        let sms = config
            .as_ref()
            .and_then(|c| c.get("sms"))
            .and_then(|v| v.as_table());

        let Some(sms) = sms else {
            return Self::default();
        };

        let enabled = sms
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let provider = sms
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("twilio")
            .to_string();

        let twilio_account_sid = sms
            .get("twilio_account_sid")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let twilio_auth_token = sms
            .get("twilio_auth_token")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let twilio_auth_token_keychain = sms
            .get("twilio_auth_token_keychain")
            .and_then(|v| v.as_str())
            .unwrap_or("clinical-twilio")
            .to_string();

        let twilio_from_number = sms
            .get("twilio_from_number")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let reminder_hours_before = sms
            .get("reminder_hours_before")
            .and_then(|v| v.as_integer())
            .unwrap_or(24) as u32;

        let confirmation_enabled = sms
            .get("confirmation_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let practitioner_name = sms
            .get("practitioner_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let practice_phone = sms
            .get("practice_phone")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Self {
            enabled,
            provider,
            twilio_account_sid,
            twilio_auth_token,
            twilio_auth_token_keychain,
            twilio_from_number,
            reminder_hours_before,
            confirmation_enabled,
            practitioner_name,
            practice_phone,
        }
    }

    /// Resolve the auth token — currently returns the direct config value.
    /// Future: try OS keychain via `twilio_auth_token_keychain` first.
    pub fn resolve_auth_token(&self) -> &str {
        &self.twilio_auth_token
    }

    /// Resolve the SMS log directory.
    pub fn log_dir(&self) -> std::path::PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".local/share"))
            .join("practiceforge")
            .join("sms-log")
    }
}
