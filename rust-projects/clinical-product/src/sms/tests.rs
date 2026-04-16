//! Tests for the SMS module.
//!
//! Tests config loading, reminder preview logic, log read/write,
//! and message template formatting. Does NOT test actual Twilio API
//! calls (no credentials available in test environment).

use super::config::SmsConfig;
use super::log::{SmsLogEntry, get_log, log_send};
use super::remind::format_reminder;
use chrono::NaiveTime;

#[test]
fn config_defaults() {
    let config = SmsConfig::default();
    assert!(!config.enabled);
    assert_eq!(config.provider, "twilio");
    assert_eq!(config.reminder_hours_before, 24);
    assert!(!config.confirmation_enabled);
    assert_eq!(config.twilio_auth_token_keychain, "clinical-twilio");
    assert!(config.twilio_account_sid.is_empty());
    assert!(config.twilio_from_number.is_empty());
    assert!(config.twilio_auth_token.is_empty());
}

#[test]
fn message_template_with_all_fields() {
    let time = NaiveTime::from_hms_opt(14, 30, 0).unwrap();
    let msg = format_reminder("Dr Napier", "020 7123 4567", time);

    assert!(msg.contains("Dr Napier"));
    assert!(msg.contains("14:30"));
    assert!(msg.contains("020 7123 4567"));
    assert!(msg.contains("tomorrow"));
    assert!(msg.contains("Reminder"));
}

#[test]
fn message_template_no_practitioner_name() {
    let time = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
    let msg = format_reminder("", "", time);

    assert!(msg.contains("your practitioner"));
    assert!(msg.contains("09:00"));
    assert!(msg.contains("get in touch"));
    // Should not contain "Please call" since practice_phone is empty
    assert!(!msg.contains("Please call"));
}

#[test]
fn message_template_no_practice_phone() {
    let time = NaiveTime::from_hms_opt(16, 0, 0).unwrap();
    let msg = format_reminder("Will Napier", "", time);

    assert!(msg.contains("Will Napier"));
    assert!(msg.contains("16:00"));
    assert!(msg.contains("get in touch"));
}

#[test]
fn log_write_read_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let log_dir = tmp.path();

    let entry = SmsLogEntry {
        timestamp: "2026-04-16T10:00:00Z".to_string(),
        client_id: "EB76".to_string(),
        client_name: "Elizabeth Briscoe".to_string(),
        phone: "+447700900000".to_string(),
        message_sid: "SM1234567890abcdef".to_string(),
        status: "queued".to_string(),
        error: None,
        appointment_date: "2026-04-17".to_string(),
        appointment_time: "10:00".to_string(),
    };

    log_send(log_dir, &entry).unwrap();

    let entries = get_log(log_dir, "2026-04-17").unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].client_id, "EB76");
    assert_eq!(entries[0].message_sid, "SM1234567890abcdef");
    assert_eq!(entries[0].status, "queued");
    assert!(entries[0].error.is_none());
}

#[test]
fn log_write_multiple_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let log_dir = tmp.path();

    let entry1 = SmsLogEntry {
        timestamp: "2026-04-16T10:00:00Z".to_string(),
        client_id: "EB76".to_string(),
        client_name: "Elizabeth Briscoe".to_string(),
        phone: "+447700900001".to_string(),
        message_sid: "SM001".to_string(),
        status: "queued".to_string(),
        error: None,
        appointment_date: "2026-04-17".to_string(),
        appointment_time: "10:00".to_string(),
    };

    let entry2 = SmsLogEntry {
        timestamp: "2026-04-16T10:01:00Z".to_string(),
        client_id: "JL07".to_string(),
        client_name: "Jane Lawson".to_string(),
        phone: "+447700900002".to_string(),
        message_sid: "SM002".to_string(),
        status: "queued".to_string(),
        error: None,
        appointment_date: "2026-04-17".to_string(),
        appointment_time: "11:00".to_string(),
    };

    log_send(log_dir, &entry1).unwrap();
    log_send(log_dir, &entry2).unwrap();

    let entries = get_log(log_dir, "2026-04-17").unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].client_id, "EB76");
    assert_eq!(entries[1].client_id, "JL07");
}

#[test]
fn log_with_error() {
    let tmp = tempfile::tempdir().unwrap();
    let log_dir = tmp.path();

    let entry = SmsLogEntry {
        timestamp: "2026-04-16T10:00:00Z".to_string(),
        client_id: "EB76".to_string(),
        client_name: "Elizabeth Briscoe".to_string(),
        phone: "+447700900000".to_string(),
        message_sid: String::new(),
        status: "failed".to_string(),
        error: Some("Invalid phone number".to_string()),
        appointment_date: "2026-04-17".to_string(),
        appointment_time: "10:00".to_string(),
    };

    log_send(log_dir, &entry).unwrap();

    let entries = get_log(log_dir, "2026-04-17").unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].status, "failed");
    assert_eq!(entries[0].error.as_deref(), Some("Invalid phone number"));
}

#[test]
fn log_read_nonexistent_date_returns_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let entries = get_log(tmp.path(), "2099-12-31").unwrap();
    assert!(entries.is_empty());
}

#[test]
fn config_resolve_auth_token() {
    let mut config = SmsConfig::default();
    config.twilio_auth_token = "test-token-123".to_string();
    assert_eq!(config.resolve_auth_token(), "test-token-123");
}

#[test]
fn config_log_dir_is_under_clinical_product() {
    let config = SmsConfig::default();
    let log_dir = config.log_dir();
    assert!(log_dir.to_string_lossy().contains("clinical-product"));
    assert!(log_dir.to_string_lossy().contains("sms-log"));
}
