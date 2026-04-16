//! Calendar data export — reads TM3 diary capture archives and converts
//! to PracticeForge scheduling YAML files.

use anyhow::{Context, Result};
use chrono::{NaiveDate, NaiveTime};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A single appointment record from a TM3 diary capture archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TM3Appointment {
    pub client_name: String,
    pub start_time: String,
    pub end_time: String,
    pub practitioner: String,
    #[serde(default)]
    pub status: String,
}

/// Result of a calendar migration run.
#[derive(Debug, Clone, Default)]
pub struct CalendarReport {
    pub appointments_created: usize,
    pub series_detected: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

impl std::fmt::Display for CalendarReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Calendar: {} appointments created, {} series detected, {} skipped",
            self.appointments_created, self.series_detected, self.skipped
        )?;
        if !self.errors.is_empty() {
            write!(f, ", {} errors", self.errors.len())?;
        }
        Ok(())
    }
}

/// Parse a datetime string from TM3 diary capture.
/// Supports "YYYY-MM-DD HH:MM" and ISO 8601 "YYYY-MM-DDTHH:MM:SS" formats.
pub fn parse_datetime(s: &str) -> Option<(NaiveDate, NaiveTime)> {
    // Try "YYYY-MM-DD HH:MM" first
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M") {
        return Some((dt.date(), dt.time()));
    }
    // Try ISO 8601
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some((dt.date(), dt.time()));
    }
    // Try ISO 8601 without seconds
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M") {
        return Some((dt.date(), dt.time()));
    }
    None
}

/// Load TM3 diary capture archives from a directory.
/// Each file is a JSON array of TM3Appointment records.
fn load_diary_archives(diary_dir: &Path) -> Result<Vec<TM3Appointment>> {
    let mut all_appointments = Vec::new();

    if !diary_dir.exists() {
        anyhow::bail!(
            "TM3 diary capture directory not found: {}",
            diary_dir.display()
        );
    }

    let mut entries: Vec<_> = std::fs::read_dir(diary_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext == "json")
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read diary archive: {}", path.display()))?;

        match serde_json::from_str::<Vec<TM3Appointment>>(&content) {
            Ok(appointments) => {
                all_appointments.extend(appointments);
            }
            Err(e) => {
                eprintln!(
                    "  Warning: failed to parse {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }

    Ok(all_appointments)
}

/// Detect recurring series from a list of appointments.
///
/// Groups appointments by (client_name, practitioner, start_time.time())
/// and checks if they recur on the same weekday. Returns groups of 3+
/// as detected series.
pub fn detect_series(
    appointments: &[TM3Appointment],
) -> Vec<(String, String, NaiveTime, Vec<NaiveDate>)> {
    // Group by (client_name, practitioner, time_of_day)
    let mut groups: HashMap<(String, String, NaiveTime), Vec<NaiveDate>> = HashMap::new();

    for appt in appointments {
        if let Some((date, time)) = parse_datetime(&appt.start_time) {
            let key = (
                appt.client_name.clone(),
                appt.practitioner.clone(),
                time,
            );
            groups.entry(key).or_default().push(date);
        }
    }

    let mut series = Vec::new();
    for ((client, practitioner, time), mut dates) in groups {
        dates.sort();
        dates.dedup();
        // Consider it a series if 3+ appointments at the same time
        if dates.len() >= 3 {
            series.push((client, practitioner, time, dates));
        }
    }

    series
}

/// Export TM3 diary data into PracticeForge scheduling format.
///
/// Reads diary capture archives, converts appointments to scheduling YAML,
/// and writes them to the schedules directory.
pub fn export_calendar(
    schedules_dir: &Path,
    dry_run: bool,
) -> Result<CalendarReport> {
    let mut report = CalendarReport::default();

    let diary_dir = dirs::home_dir()
        .expect("no home dir")
        .join(".local/share/practiceforge/tm3-diary-capture");

    let appointments = match load_diary_archives(&diary_dir) {
        Ok(appts) => appts,
        Err(e) => {
            report.errors.push(format!("Failed to load diary archives: {}", e));
            return Ok(report);
        }
    };

    eprintln!(
        "[tm3-migrate] Loaded {} appointments from diary archives",
        appointments.len()
    );

    // Detect recurring series
    let series = detect_series(&appointments);
    report.series_detected = series.len();

    if dry_run {
        println!("  {} total appointments in diary archives", appointments.len());
        println!("  {} recurring series detected", series.len());
        for (client, practitioner, time, dates) in &series {
            println!(
                "    {} with {} at {} ({} occurrences, {} to {})",
                client,
                practitioner,
                time.format("%H:%M"),
                dates.len(),
                dates.first().map(|d| d.to_string()).unwrap_or_default(),
                dates.last().map(|d| d.to_string()).unwrap_or_default(),
            );
        }
        report.appointments_created = appointments.len();
        return Ok(report);
    }

    // Write one-off appointments that are NOT part of a detected series
    let series_keys: std::collections::HashSet<(String, String, NaiveTime)> = series
        .iter()
        .map(|(c, p, t, _)| (c.clone(), p.clone(), *t))
        .collect();

    for appt in &appointments {
        let Some((date, start_time)) = parse_datetime(&appt.start_time) else {
            report.errors.push(format!(
                "Failed to parse start_time '{}' for {}",
                appt.start_time, appt.client_name
            ));
            continue;
        };

        let end_time = parse_datetime(&appt.end_time)
            .map(|(_, t)| t)
            .unwrap_or_else(|| start_time + chrono::Duration::minutes(50));

        let key = (
            appt.client_name.clone(),
            appt.practitioner.clone(),
            start_time,
        );

        // Skip appointments that belong to a detected series
        if series_keys.contains(&key) {
            continue;
        }

        // Skip cancelled appointments
        if appt.status.to_lowercase().contains("cancel") {
            report.skipped += 1;
            continue;
        }

        let prac_slug = slugify(&appt.practitioner);
        let appts_dir = PathBuf::from(schedules_dir)
            .join(&prac_slug)
            .join("appointments");
        std::fs::create_dir_all(&appts_dir)?;

        let appointment = crate::scheduling::models::Appointment {
            id: uuid::Uuid::new_v4(),
            series_id: None,
            practitioner: prac_slug.clone(),
            client_id: String::new(), // Will be resolved later via registry lookup
            client_name: appt.client_name.clone(),
            date,
            start_time,
            end_time,
            status: crate::scheduling::models::AppointmentStatus::Completed,
            source: crate::scheduling::models::AppointmentSource::Migration,
            rate_tag: None,
            location: String::new(),
            sms_confirmation: None,
            notes: Some("Imported from TM3 diary capture".to_string()),
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        let path = appts_dir.join(format!("{}.yaml", appointment.id));
        let yaml = serde_yaml::to_string(&appointment)?;
        std::fs::write(&path, &yaml)?;
        report.appointments_created += 1;
    }

    Ok(report)
}

/// Convert a practitioner name to a slug (lowercase, hyphens).
fn slugify(name: &str) -> String {
    name.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}
