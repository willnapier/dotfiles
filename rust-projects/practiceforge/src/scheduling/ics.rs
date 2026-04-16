//! ICS (iCalendar) import/export for appointments and series.
//!
//! Generates RFC 5545-compliant VCALENDAR output that can be imported
//! into any calendar app (Apple Calendar, Google Calendar, Outlook).

use anyhow::Result;
use chrono::NaiveDate;
use icalendar::{Calendar, Component, Event, EventLike, Property};

use super::models::{Appointment, Frequency, RecurringSeries};

/// Export a list of appointments as an ICS calendar string.
pub fn appointments_to_ics(appointments: &[Appointment], cal_name: &str) -> String {
    let mut cal = Calendar::new();
    cal.name(cal_name);
    cal.append_property(Property::new("PRODID", "-//PracticeForge//EN"));

    for appt in appointments {
        let event = appointment_to_event(appt);
        cal.push(event);
    }

    cal.done().to_string()
}

/// Export a recurring series as an ICS event with RRULE.
pub fn series_to_ics(series: &RecurringSeries) -> String {
    let mut cal = Calendar::new();
    cal.append_property(Property::new("PRODID", "-//PracticeForge//EN"));

    let event = series_to_event(series);
    cal.push(event);

    cal.done().to_string()
}

/// Export multiple series and one-off appointments as a single calendar.
pub fn full_calendar_to_ics(
    series: &[RecurringSeries],
    one_offs: &[Appointment],
    cal_name: &str,
) -> String {
    let mut cal = Calendar::new();
    cal.name(cal_name);
    cal.append_property(Property::new("PRODID", "-//PracticeForge//EN"));

    for s in series {
        cal.push(series_to_event(s));
    }

    for appt in one_offs {
        cal.push(appointment_to_event(appt));
    }

    cal.done().to_string()
}

fn appointment_to_event(appt: &Appointment) -> Event {
    let dtstart = appt.date.and_time(appt.start_time);
    let dtend = appt.date.and_time(appt.end_time);

    let summary = format!("{} ({})", appt.client_name, appt.client_id);

    let mut event = Event::new();
    event.uid(&appt.id.to_string());
    event.summary(&summary);
    event.starts(dtstart);
    event.ends(dtend);
    event.location(&appt.location);
    event.append_property(Property::new("STATUS", &status_to_ics(&appt.status)));

    if let Some(ref notes) = appt.notes {
        event.description(notes);
    }

    event.done()
}

fn series_to_event(series: &RecurringSeries) -> Event {
    let dtstart = series.recurrence.dtstart.and_time(series.start_time);
    let dtend = series.recurrence.dtstart.and_time(series.end_time);

    let summary = format!("{} ({})", series.client_name, series.client_id);

    let mut event = Event::new();
    event.uid(&series.id.to_string());
    event.summary(&summary);
    event.starts(dtstart);
    event.ends(dtend);
    event.location(&series.location);

    // Build RRULE string
    let rrule = build_rrule_string(&series.recurrence.freq, series.recurrence.interval,
        series.recurrence.until, series.recurrence.count, &series.recurrence.by_day);
    event.append_property(Property::new("RRULE", &rrule));

    // Add EXDATE entries
    for exdate in &series.exdates {
        let exdate_str = format!("{}T{}", exdate.format("%Y%m%d"), series.start_time.format("%H%M%S"));
        event.append_property(Property::new("EXDATE", &exdate_str));
    }

    if let Some(ref notes) = series.notes {
        event.description(notes);
    }

    event.done()
}

fn build_rrule_string(
    freq: &Frequency,
    interval: u32,
    until: Option<NaiveDate>,
    count: Option<u32>,
    by_day: &Option<Vec<super::models::Weekday>>,
) -> String {
    let freq_str = match freq {
        Frequency::Weekly => "WEEKLY",
        Frequency::Monthly => "MONTHLY",
    };

    let mut parts = vec![format!("FREQ={freq_str}")];

    if interval > 1 {
        parts.push(format!("INTERVAL={interval}"));
    }

    if let Some(until) = until {
        parts.push(format!("UNTIL={}T235959Z", until.format("%Y%m%d")));
    }

    if let Some(count) = count {
        parts.push(format!("COUNT={count}"));
    }

    if let Some(days) = by_day {
        let day_strs: Vec<&str> = days
            .iter()
            .map(|d| match d {
                super::models::Weekday::Mon => "MO",
                super::models::Weekday::Tue => "TU",
                super::models::Weekday::Wed => "WE",
                super::models::Weekday::Thu => "TH",
                super::models::Weekday::Fri => "FR",
                super::models::Weekday::Sat => "SA",
                super::models::Weekday::Sun => "SU",
            })
            .collect();
        parts.push(format!("BYDAY={}", day_strs.join(",")));
    }

    parts.join(";")
}

fn status_to_ics(status: &super::models::AppointmentStatus) -> String {
    use super::models::AppointmentStatus::*;
    match status {
        Tentative => "TENTATIVE".to_string(),
        Confirmed | Arrived | Completed => "CONFIRMED".to_string(),
        Cancelled | LateCancellation => "CANCELLED".to_string(),
        NoShow => "CANCELLED".to_string(),
    }
}

/// Parse a list of holiday dates from a YAML file.
/// Expected format: a YAML list of date strings (YYYY-MM-DD).
pub fn load_holidays(yaml_str: &str) -> Result<Vec<NaiveDate>> {
    let dates: Vec<String> = serde_yaml::from_str(yaml_str)?;
    dates
        .iter()
        .map(|s| {
            NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map_err(|e| anyhow::anyhow!("Invalid date '{}': {}", s, e))
        })
        .collect()
}

/// Load one-off appointment definitions from a directory of YAML files.
pub fn load_appointments_dir(dir: &std::path::Path) -> Result<Vec<Appointment>> {
    let mut appointments = Vec::new();
    if !dir.exists() {
        return Ok(appointments);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "yaml" || e == "yml") {
            let content = std::fs::read_to_string(&path)?;
            let a: Appointment = serde_yaml::from_str(&content)?;
            appointments.push(a);
        }
    }
    Ok(appointments)
}

/// Load series definitions from a directory of YAML files.
pub fn load_series_dir(dir: &std::path::Path) -> Result<Vec<RecurringSeries>> {
    let mut series = Vec::new();
    if !dir.exists() {
        return Ok(series);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "yaml" || e == "yml") {
            let content = std::fs::read_to_string(&path)?;
            let s: RecurringSeries = serde_yaml::from_str(&content)?;
            series.push(s);
        }
    }
    Ok(series)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduling::models::*;
    use chrono::NaiveTime;
    use uuid::Uuid;

    fn make_appointment() -> Appointment {
        Appointment {
            id: Uuid::new_v4(),
            series_id: None,
            practitioner: "will-napier".to_string(),
            client_id: "EB76".to_string(),
            client_name: "Elizabeth Briscoe".to_string(),
            date: NaiveDate::from_ymd_opt(2026, 4, 16).unwrap(),
            start_time: NaiveTime::from_hms_opt(10, 0, 0).unwrap(),
            end_time: NaiveTime::from_hms_opt(10, 50, 0).unwrap(),
            status: AppointmentStatus::Confirmed,
            source: AppointmentSource::Practitioner,
            rate_tag: Some("insurer".to_string()),
            location: "37 Gloucester Place".to_string(),
            sms_confirmation: None,
            notes: None,
            created_at: "2026-04-16T00:00:00Z".to_string(),
        }
    }

    fn make_series() -> RecurringSeries {
        RecurringSeries {
            id: Uuid::new_v4(),
            practitioner: "will-napier".to_string(),
            client_id: "EB76".to_string(),
            client_name: "Elizabeth Briscoe".to_string(),
            start_time: NaiveTime::from_hms_opt(10, 0, 0).unwrap(),
            end_time: NaiveTime::from_hms_opt(10, 50, 0).unwrap(),
            location: "37 Gloucester Place".to_string(),
            rate_tag: None,
            recurrence: RecurrenceRule {
                freq: Frequency::Weekly,
                interval: 1,
                by_day: None,
                dtstart: NaiveDate::from_ymd_opt(2026, 4, 16).unwrap(),
                until: None,
                count: None,
            },
            exdates: vec![NaiveDate::from_ymd_opt(2026, 4, 30).unwrap()],
            status: SeriesStatus::Active,
            created_at: "2026-04-16T00:00:00Z".to_string(),
            notes: Some("Weekly session".to_string()),
        }
    }

    #[test]
    fn appointment_ics_contains_summary() {
        let appt = make_appointment();
        let ics = appointments_to_ics(&[appt], "Test Calendar");
        assert!(ics.contains("Elizabeth Briscoe (EB76)"));
        assert!(ics.contains("VCALENDAR"));
        assert!(ics.contains("VEVENT"));
    }

    #[test]
    fn series_ics_contains_rrule() {
        let series = make_series();
        let ics = series_to_ics(&series);
        assert!(ics.contains("RRULE:FREQ=WEEKLY"));
        assert!(ics.contains("EXDATE"));
    }

    #[test]
    fn fortnightly_rrule_string() {
        let rrule = build_rrule_string(&Frequency::Weekly, 2, None, None, &None);
        assert_eq!(rrule, "FREQ=WEEKLY;INTERVAL=2");
    }

    #[test]
    fn count_limited_rrule_string() {
        let rrule = build_rrule_string(&Frequency::Weekly, 1, None, Some(10), &None);
        assert_eq!(rrule, "FREQ=WEEKLY;COUNT=10");
    }

    #[test]
    fn rrule_with_byday() {
        let days = Some(vec![Weekday::Tue, Weekday::Thu]);
        let rrule = build_rrule_string(&Frequency::Weekly, 1, None, None, &days);
        assert_eq!(rrule, "FREQ=WEEKLY;BYDAY=TU,TH");
    }

    #[test]
    fn holidays_yaml_parsing() {
        let yaml = r#"
- "2026-12-25"
- "2026-12-26"
- "2027-01-01"
"#;
        let dates = load_holidays(yaml).unwrap();
        assert_eq!(dates.len(), 3);
        assert_eq!(dates[0], NaiveDate::from_ymd_opt(2026, 12, 25).unwrap());
    }

    #[test]
    fn full_calendar_combines_series_and_one_offs() {
        let series = make_series();
        let appt = make_appointment();
        let ics = full_calendar_to_ics(&[series], &[appt], "COHS Calendar");
        assert!(ics.contains("RRULE:FREQ=WEEKLY"));
        assert!(ics.contains("Elizabeth Briscoe (EB76)"));
        let event_count = ics.matches("BEGIN:VEVENT").count();
        assert_eq!(event_count, 2); // 1 series + 1 one-off
    }
}
