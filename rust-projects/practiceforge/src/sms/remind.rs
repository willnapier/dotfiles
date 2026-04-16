//! Reminder logic — preview and send appointment reminders.
//!
//! Reads appointments from the scheduling module's YAML files,
//! looks up client phone numbers from the registry (with identity.yaml
//! fallback), and constructs reminder messages.

use anyhow::{Context, Result};
use chrono::{Local, NaiveDate, NaiveTime};

use super::config::SmsConfig;
use super::log::{SmsLogEntry, self};
use super::twilio::{self, SmsResult};

/// Preview of a reminder that would be sent.
#[derive(Debug, Clone)]
pub struct ReminderPreview {
    pub client_id: String,
    pub client_name: String,
    pub phone: String,
    pub appointment_time: NaiveTime,
    pub appointment_date: NaiveDate,
    pub message_text: String,
}

/// An appointment loaded from the scheduling YAML files, flattened for
/// reminder processing.
#[derive(Debug, Clone)]
struct ScheduledAppointment {
    client_id: String,
    client_name: String,
    #[allow(dead_code)]
    date: NaiveDate,
    start_time: NaiveTime,
}

/// Build the reminder message from a template.
pub fn format_reminder(
    practitioner_name: &str,
    practice_phone: &str,
    appointment_time: NaiveTime,
) -> String {
    let time_str = appointment_time.format("%H:%M").to_string();

    let practitioner = if practitioner_name.is_empty() {
        "your practitioner"
    } else {
        practitioner_name
    };

    let reschedule = if practice_phone.is_empty() {
        "Please get in touch if you need to reschedule.".to_string()
    } else {
        format!("Please call {} if you need to reschedule.", practice_phone)
    };

    format!(
        "Reminder: your appointment with {} is tomorrow at {}. {}",
        practitioner, time_str, reschedule
    )
}

/// Load appointments for a given date from the scheduling module's YAML files.
fn load_appointments_for_date(date: NaiveDate) -> Result<Vec<ScheduledAppointment>> {
    let sched_config = crate::scheduling::config::SchedulingConfig::default();
    let schedules_dir = shellexpand::tilde(&sched_config.schedules_dir).to_string();
    let practitioner = &sched_config.default_practitioner;

    let series_dir = std::path::PathBuf::from(&schedules_dir)
        .join(practitioner)
        .join("series");
    let appts_dir = std::path::PathBuf::from(&schedules_dir)
        .join(practitioner)
        .join("appointments");

    // Load holidays
    let holidays_path = std::path::PathBuf::from(&schedules_dir)
        .join(practitioner)
        .join("holidays.yaml");
    let holidays = if holidays_path.exists() {
        let yaml = std::fs::read_to_string(&holidays_path)?;
        crate::scheduling::ics::load_holidays(&yaml)?
    } else {
        vec![]
    };

    let mut result = Vec::new();

    // Materialise recurring series for this date
    let series_list = crate::scheduling::ics::load_series_dir(&series_dir)?;
    for series in &series_list {
        if series.status != crate::scheduling::SeriesStatus::Active {
            continue;
        }
        let dates = crate::scheduling::recurrence::materialise(series, date, date, &holidays)?;
        if dates.contains(&date) {
            result.push(ScheduledAppointment {
                client_id: series.client_id.clone(),
                client_name: series.client_name.clone(),
                date,
                start_time: series.start_time,
            });
        }
    }

    // Load one-off appointments for this date
    let one_offs = crate::scheduling::ics::load_appointments_dir(&appts_dir)?;
    for appt in &one_offs {
        if appt.date == date
            && appt.status != crate::scheduling::AppointmentStatus::Cancelled
        {
            // Avoid duplicates — if a one-off already covers this client+date
            // from a series materialisation above, prefer the one-off (it may
            // have updated status/time).
            let dominated = result.iter().any(|r| r.client_id == appt.client_id);
            if dominated {
                // Replace the series entry with the one-off's time
                for r in &mut result {
                    if r.client_id == appt.client_id {
                        r.start_time = appt.start_time;
                    }
                }
            } else {
                result.push(ScheduledAppointment {
                    client_id: appt.client_id.clone(),
                    client_name: appt.client_name.clone(),
                    date: appt.date,
                    start_time: appt.start_time,
                });
            }
        }
    }

    Ok(result)
}

/// Look up a client's phone number.
///
/// Tries the registry first, then falls back to ~/Clinical/clients/{id}/identity.yaml.
fn lookup_phone(client_id: &str) -> Option<String> {
    // Try registry
    let reg_config = crate::registry::config::RegistryConfig::load();
    if let Ok(client) = crate::registry::get_client(&reg_config, client_id) {
        if let Some(ref phone) = client.phone {
            if !phone.is_empty() {
                return Some(phone.clone());
            }
        }
    }

    // Fall back to identity.yaml in ~/Clinical/clients/
    let identity_path = crate::config::clients_dir()
        .join(client_id)
        .join("identity.yaml");

    if identity_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&identity_path) {
            if let Ok(val) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
                if let Some(phone) = val.get("phone").and_then(|v| v.as_str()) {
                    if !phone.is_empty() {
                        return Some(phone.to_string());
                    }
                }
            }
        }
    }

    None
}

/// Parse a date string (YYYY-MM-DD), defaulting to tomorrow.
fn parse_date_or_tomorrow(date: Option<&str>) -> Result<NaiveDate> {
    match date {
        Some(d) => NaiveDate::parse_from_str(d, "%Y-%m-%d")
            .map_err(|e| anyhow::anyhow!("Invalid date '{}': {}", d, e)),
        None => Ok(Local::now().date_naive() + chrono::Duration::days(1)),
    }
}

/// Preview reminders that would be sent for a date (dry run).
pub fn preview_reminders(
    config: &SmsConfig,
    date: Option<&str>,
) -> Result<Vec<ReminderPreview>> {
    let target_date = parse_date_or_tomorrow(date)?;
    let appointments = load_appointments_for_date(target_date)
        .context("Failed to load appointments")?;

    let mut previews = Vec::new();

    for appt in &appointments {
        match lookup_phone(&appt.client_id) {
            Some(phone) => {
                let message = format_reminder(
                    &config.practitioner_name,
                    &config.practice_phone,
                    appt.start_time,
                );
                previews.push(ReminderPreview {
                    client_id: appt.client_id.clone(),
                    client_name: appt.client_name.clone(),
                    phone,
                    appointment_time: appt.start_time,
                    appointment_date: target_date,
                    message_text: message,
                });
            }
            None => {
                eprintln!(
                    "Warning: no phone number for {} ({}) — skipping SMS reminder",
                    appt.client_name, appt.client_id
                );
            }
        }
    }

    Ok(previews)
}

/// Send reminders for a date. Returns results for each sent message.
pub async fn send_reminders(
    config: &SmsConfig,
    date: Option<&str>,
) -> Result<Vec<SmsResult>> {
    if !config.enabled {
        anyhow::bail!("SMS reminders are not enabled. Set enabled = true in [sms] config.");
    }

    let previews = preview_reminders(config, date)?;
    if previews.is_empty() {
        println!("No reminders to send.");
        return Ok(vec![]);
    }

    let log_dir = config.log_dir();
    std::fs::create_dir_all(&log_dir)?;

    let mut results = Vec::new();

    for preview in &previews {
        let result = twilio::send_sms(config, &preview.phone, &preview.message_text).await;

        match result {
            Ok(sms_result) => {
                let log_entry = SmsLogEntry {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    client_id: preview.client_id.clone(),
                    client_name: preview.client_name.clone(),
                    phone: preview.phone.clone(),
                    message_sid: sms_result.message_sid.clone(),
                    status: sms_result.status.clone(),
                    error: sms_result.error_message.clone(),
                    appointment_date: preview.appointment_date.to_string(),
                    appointment_time: preview.appointment_time.format("%H:%M").to_string(),
                };

                if let Err(e) = log::log_send(&log_dir, &log_entry) {
                    eprintln!("Warning: failed to write SMS log: {}", e);
                }

                if sms_result.error_message.is_some() {
                    eprintln!(
                        "  Failed: {} ({}) — {}",
                        preview.client_name,
                        preview.client_id,
                        sms_result.error_message.as_deref().unwrap_or("unknown error")
                    );
                } else {
                    println!(
                        "  Sent: {} ({}) — {} [{}]",
                        preview.client_name,
                        preview.client_id,
                        preview.phone,
                        sms_result.message_sid
                    );
                }

                results.push(sms_result);
            }
            Err(e) => {
                eprintln!(
                    "  Error: {} ({}) — {}",
                    preview.client_name, preview.client_id, e
                );

                let log_entry = SmsLogEntry {
                    timestamp: chrono::Utc::now().to_rfc3339(),
                    client_id: preview.client_id.clone(),
                    client_name: preview.client_name.clone(),
                    phone: preview.phone.clone(),
                    message_sid: String::new(),
                    status: "error".to_string(),
                    error: Some(e.to_string()),
                    appointment_date: preview.appointment_date.to_string(),
                    appointment_time: preview.appointment_time.format("%H:%M").to_string(),
                };

                let _ = log::log_send(&log_dir, &log_entry);

                results.push(SmsResult {
                    message_sid: String::new(),
                    status: "error".to_string(),
                    error_message: Some(e.to_string()),
                });
            }
        }
    }

    Ok(results)
}

/// Show delivery status for sent reminders on a given date.
pub fn show_status(config: &SmsConfig, date: Option<&str>) -> Result<()> {
    let target_date = match date {
        Some(d) => NaiveDate::parse_from_str(d, "%Y-%m-%d")
            .map_err(|e| anyhow::anyhow!("Invalid date '{}': {}", d, e))?,
        None => Local::now().date_naive(),
    };

    let log_dir = config.log_dir();
    let entries = log::get_log(&log_dir, &target_date.to_string())?;

    if entries.is_empty() {
        println!("No SMS reminders logged for {}.", target_date);
        return Ok(());
    }

    println!("SMS reminders for {}:\n", target_date);
    for entry in &entries {
        let status_marker = match entry.status.as_str() {
            "queued" | "sent" | "delivered" => "ok",
            "failed" | "error" => "FAIL",
            _ => &entry.status,
        };

        println!(
            "  [{}] {} ({}) -> {} at {} — SID: {}",
            status_marker,
            entry.client_name,
            entry.client_id,
            entry.phone,
            entry.appointment_time,
            if entry.message_sid.is_empty() {
                "-"
            } else {
                &entry.message_sid
            }
        );

        if let Some(ref err) = entry.error {
            println!("         Error: {}", err);
        }
    }

    Ok(())
}
