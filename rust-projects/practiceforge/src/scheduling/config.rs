//! Configuration for the scheduling module.
//!
//! Reads the `[scheduling]` section of config.toml. Disabled by default.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SchedulingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_practitioner")]
    pub default_practitioner: String,
    #[serde(default = "default_location")]
    pub location: String,
    #[serde(default = "default_schedules_dir")]
    pub schedules_dir: String,

    #[serde(default)]
    pub availability: AvailabilityConfig,

    #[serde(default)]
    pub blocks: BlockConfig,

    #[serde(default)]
    pub sms: SmsConfig,
}

impl SchedulingConfig {
    pub fn portal_base_url(&self) -> String {
        "http://localhost:3457".to_string()
    }
}

impl Default for SchedulingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_practitioner: default_practitioner(),
            location: default_location(),
            schedules_dir: default_schedules_dir(),
            availability: AvailabilityConfig::default(),
            blocks: BlockConfig::default(),
            sms: SmsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AvailabilityConfig {
    #[serde(default = "default_slot_duration")]
    pub slot_duration_minutes: u32,
    #[serde(default = "default_buffer")]
    pub buffer_minutes: u32,
    #[serde(default = "default_min_notice")]
    pub min_notice_hours: u32,
    #[serde(default = "default_max_advance")]
    pub max_advance_days: u32,
}

impl Default for AvailabilityConfig {
    fn default() -> Self {
        Self {
            slot_duration_minutes: default_slot_duration(),
            buffer_minutes: default_buffer(),
            min_notice_hours: default_min_notice(),
            max_advance_days: default_max_advance(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct BlockConfig {
    #[serde(default = "default_warning_threshold")]
    pub warning_threshold: u32,
}

impl Default for BlockConfig {
    fn default() -> Self {
        Self {
            warning_threshold: default_warning_threshold(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SmsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_reminder_hours")]
    pub reminder_hours_before: u32,
    #[serde(default)]
    pub twilio_account_sid: String,
    #[serde(default)]
    pub twilio_from_number: String,
}

impl Default for SmsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            reminder_hours_before: default_reminder_hours(),
            twilio_account_sid: String::new(),
            twilio_from_number: String::new(),
        }
    }
}

fn default_practitioner() -> String {
    "default".to_string()
}

fn default_location() -> String {
    "37 Gloucester Place".to_string()
}

fn default_schedules_dir() -> String {
    "~/Clinical/schedules".to_string()
}

fn default_slot_duration() -> u32 {
    50
}

fn default_buffer() -> u32 {
    10
}

fn default_min_notice() -> u32 {
    24
}

fn default_max_advance() -> u32 {
    56
}

fn default_warning_threshold() -> u32 {
    2
}

fn default_reminder_hours() -> u32 {
    24
}
