//! SMS appointment reminders via Twilio REST API.
//!
//! Sends reminder texts to clients the day before their appointment.
//! Reads appointments from the scheduling module's YAML files and
//! client phone numbers from the registry (with identity.yaml fallback).
//!
//! Enable via `[sms]` section in config.toml.

pub mod config;
pub mod log;
pub mod remind;
pub mod twilio;

#[cfg(test)]
mod tests;

pub use config::SmsConfig;
pub use log::SmsLogEntry;
pub use remind::ReminderPreview;
pub use twilio::SmsResult;
