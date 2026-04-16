//! Self-booking portal — clients book their own appointments.
//!
//! Flow: practitioner generates a booking link for a client →
//! client opens on phone → OTP via SMS → pick slot → confirm.
//!
//! Routes served at /book/{token}/*.

use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::Html,
    routing::{get, post},
    Json, Router,
};
use chrono::{Datelike, Local, NaiveDate, NaiveTime};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use uuid::Uuid;

use crate::scheduling;

const PORTAL_HTML: &str = include_str!("portal_assets/portal.html");

/// Dev mode path for live reload.
const DEV_HTML_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/portal_assets/portal.html");

// ---------------------------------------------------------------------------
// In-memory state (single-process, no persistence needed)
// ---------------------------------------------------------------------------

lazy_static::lazy_static! {
    /// Booking tokens: token → BookingLink
    static ref BOOKING_LINKS: Mutex<HashMap<String, BookingLink>> = Mutex::new(HashMap::new());
    /// OTP codes: token → (code, phone, expires_at)
    static ref OTP_CODES: Mutex<HashMap<String, OtpEntry>> = Mutex::new(HashMap::new());
    /// Session tokens: session_token → token (verified sessions)
    static ref SESSION_TOKENS: Mutex<HashMap<String, String>> = Mutex::new(HashMap::new());
}

#[derive(Clone, Debug)]
struct BookingLink {
    client_id: String,
    practitioner: String,
    created_at: String,
}

#[derive(Clone, Debug)]
struct OtpEntry {
    code: String,
    phone: String,
    expires_at: chrono::DateTime<chrono::Utc>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn build_router() -> Router {
    Router::new()
        .route("/book/{token}", get(portal_page))
        .route("/book/{token}/info", get(portal_info))
        .route("/book/{token}/send-code", post(portal_send_code))
        .route("/book/{token}/verify", post(portal_verify))
        .route("/book/{token}/slots", get(portal_slots))
        .route("/book/{token}/reserve", post(portal_reserve))
}

/// Generate a booking link for a client. Called from CLI or admin dashboard.
pub fn create_booking_link(client_id: &str, practitioner: &str) -> String {
    let token = Uuid::new_v4().to_string();
    let link = BookingLink {
        client_id: client_id.to_string(),
        practitioner: practitioner.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    BOOKING_LINKS.lock().unwrap().insert(token.clone(), link);
    token
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn portal_page(Path(token): Path<String>) -> Result<Html<String>, StatusCode> {
    // Verify token exists
    let links = BOOKING_LINKS.lock().unwrap();
    if !links.contains_key(&token) {
        return Err(StatusCode::NOT_FOUND);
    }
    drop(links);

    // Serve the portal HTML
    if std::env::var("PF_DEV").is_ok() {
        if let Ok(content) = std::fs::read_to_string(DEV_HTML_PATH) {
            return Ok(Html(content));
        }
    }
    Ok(Html(PORTAL_HTML.to_string()))
}

#[derive(Serialize)]
struct PortalInfo {
    practice_name: String,
    practitioner_name: String,
}

async fn portal_info(Path(token): Path<String>) -> Result<Json<PortalInfo>, StatusCode> {
    let links = BOOKING_LINKS.lock().unwrap();
    let link = links.get(&token).ok_or(StatusCode::NOT_FOUND)?;
    let prac = link.practitioner.clone();
    drop(links);

    Ok(Json(PortalInfo {
        practice_name: load_practice_name(),
        practitioner_name: prac,
    }))
}

#[derive(Deserialize)]
struct SendCodeRequest {
    phone: String,
}

async fn portal_send_code(
    Path(token): Path<String>,
    Json(req): Json<SendCodeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Verify token
    {
        let links = BOOKING_LINKS.lock().unwrap();
        if !links.contains_key(&token) {
            return Err((StatusCode::NOT_FOUND, "Invalid booking link".to_string()));
        }
    }

    // Generate 6-digit code
    let code = format!("{:06}", rand_code());
    let expires = chrono::Utc::now() + chrono::Duration::minutes(10);

    // Store OTP
    OTP_CODES.lock().unwrap().insert(token.clone(), OtpEntry {
        code: code.clone(),
        phone: req.phone.clone(),
        expires_at: expires,
    });

    // Send via Twilio (if configured) or log for dev
    let sms_config = crate::sms::config::SmsConfig::load();
    if sms_config.enabled {
        let msg = format!("Your PracticeForge booking code is: {}. Valid for 10 minutes.", code);
        match crate::sms::twilio::send_sms(&sms_config, &req.phone, &msg).await {
            Ok(_) => Ok(Json(serde_json::json!({"ok": true}))),
            Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, format!("SMS failed: {}", e))),
        }
    } else {
        // Dev mode: log the code to stderr
        eprintln!("[portal] OTP for {}: {} (SMS not configured)", req.phone, code);
        Ok(Json(serde_json::json!({"ok": true, "dev_code": code})))
    }
}

#[derive(Deserialize)]
struct VerifyCodeRequest {
    code: String,
}

async fn portal_verify(
    Path(token): Path<String>,
    Json(req): Json<VerifyCodeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let otp_store = OTP_CODES.lock().unwrap();
    let entry = otp_store.get(&token)
        .ok_or((StatusCode::BAD_REQUEST, "No code sent for this link".to_string()))?;

    if chrono::Utc::now() > entry.expires_at {
        return Err((StatusCode::BAD_REQUEST, "Code expired. Request a new one.".to_string()));
    }

    if entry.code != req.code {
        return Err((StatusCode::BAD_REQUEST, "Invalid code".to_string()));
    }

    drop(otp_store);

    // Create session token
    let session_token = Uuid::new_v4().to_string();
    SESSION_TOKENS.lock().unwrap().insert(session_token.clone(), token);

    Ok(Json(serde_json::json!({"ok": true, "session_token": session_token})))
}

#[derive(Serialize)]
struct SlotResponse {
    slots: Vec<AvailableSlot>,
    practitioner_name: String,
}

#[derive(Serialize)]
struct AvailableSlot {
    date: String,
    start_time: String,
    end_time: String,
}

async fn portal_slots(
    Path(token): Path<String>,
) -> Result<Json<SlotResponse>, (StatusCode, String)> {
    // Get booking link info
    let links = BOOKING_LINKS.lock().unwrap();
    let link = links.get(&token)
        .ok_or((StatusCode::NOT_FOUND, "Invalid booking link".to_string()))?;
    let practitioner = link.practitioner.clone();
    drop(links);

    let sched_config = scheduling::SchedulingConfig::default();
    let schedules_dir = shellexpand::tilde(&sched_config.schedules_dir).to_string();
    let prac_path = std::path::PathBuf::from(&schedules_dir).join(&practitioner);

    let today = Local::now().date_naive();
    let min_notice = chrono::Duration::hours(sched_config.availability.min_notice_hours as i64);
    let earliest = (Local::now() + min_notice).date_naive();
    let latest = today + chrono::Duration::days(sched_config.availability.max_advance_days as i64);

    // Load existing appointments (series + one-offs)
    let series_dir = prac_path.join("series");
    let appts_dir = prac_path.join("appointments");
    let holidays_path = prac_path.join("holidays.yaml");

    let series_list = scheduling::ics::load_series_dir(&series_dir).unwrap_or_default();
    let one_offs = scheduling::ics::load_appointments_dir(&appts_dir).unwrap_or_default();

    let holidays = if holidays_path.exists() {
        std::fs::read_to_string(&holidays_path)
            .ok()
            .and_then(|yaml| scheduling::ics::load_holidays(&yaml).ok())
            .unwrap_or_default()
    } else {
        vec![]
    };

    // Materialise all occupied slots in the window
    let mut occupied: Vec<(NaiveDate, NaiveTime, NaiveTime)> = Vec::new();

    for s in &series_list {
        if s.status != scheduling::SeriesStatus::Active { continue; }
        let dates = scheduling::recurrence::materialise(s, earliest, latest, &holidays)
            .unwrap_or_default();
        for d in dates {
            occupied.push((d, s.start_time, s.end_time));
        }
    }

    for appt in &one_offs {
        if appt.date >= earliest && appt.date <= latest
            && appt.status != scheduling::AppointmentStatus::Cancelled
        {
            occupied.push((appt.date, appt.start_time, appt.end_time));
        }
    }

    // Load availability config (or use defaults)
    let slot_dur = sched_config.availability.slot_duration_minutes;
    let buffer = sched_config.availability.buffer_minutes;
    let step = slot_dur + buffer;

    // Generate available slots
    let mut slots = Vec::new();
    let mut date = earliest;
    while date <= latest {
        let weekday = date.weekday();
        // Skip weekends
        if weekday == chrono::Weekday::Sat || weekday == chrono::Weekday::Sun {
            date += chrono::Duration::days(1);
            continue;
        }
        // Skip holidays
        if holidays.contains(&date) {
            date += chrono::Duration::days(1);
            continue;
        }

        // Working hours: 08:00 - 20:00 (default, could read from availability.yaml)
        let work_start = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
        let work_end = NaiveTime::from_hms_opt(20, 0, 0).unwrap();

        let mut time = work_start;
        while time + chrono::Duration::minutes(slot_dur as i64) <= work_end {
            let end = time + chrono::Duration::minutes(slot_dur as i64);

            // Check if this slot overlaps any occupied slot
            let is_occupied = occupied.iter().any(|(od, os, oe)| {
                *od == date && time < *oe && end > *os
            });

            if !is_occupied {
                slots.push(AvailableSlot {
                    date: date.format("%Y-%m-%d").to_string(),
                    start_time: time.format("%H:%M").to_string(),
                    end_time: end.format("%H:%M").to_string(),
                });
            }

            time += chrono::Duration::minutes(step as i64);
        }

        date += chrono::Duration::days(1);
    }

    Ok(Json(SlotResponse {
        slots,
        practitioner_name: practitioner,
    }))
}

#[derive(Deserialize)]
struct ReserveRequest {
    date: String,
    time: String,
}

async fn portal_reserve(
    Path(token): Path<String>,
    Json(req): Json<ReserveRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Get booking link info
    let links = BOOKING_LINKS.lock().unwrap();
    let link = links.get(&token)
        .ok_or((StatusCode::NOT_FOUND, "Invalid booking link".to_string()))?;
    let client_id = link.client_id.clone();
    let practitioner = link.practitioner.clone();
    drop(links);

    let sched_config = scheduling::SchedulingConfig::default();
    let schedules_dir = shellexpand::tilde(&sched_config.schedules_dir).to_string();

    let date = NaiveDate::parse_from_str(&req.date, "%Y-%m-%d")
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid date: {}", e)))?;
    let start_time = NaiveTime::parse_from_str(&req.time, "%H:%M")
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid time: {}", e)))?;
    let end_time = start_time + chrono::Duration::minutes(sched_config.availability.slot_duration_minutes as i64);

    // Look up client name
    let client_name = lookup_client_name(&client_id);

    // Create the appointment
    let appt = scheduling::Appointment {
        id: Uuid::new_v4(),
        series_id: None,
        practitioner: practitioner.clone(),
        client_id: client_id.clone(),
        client_name,
        date,
        start_time,
        end_time,
        status: scheduling::AppointmentStatus::Tentative,
        source: scheduling::AppointmentSource::SelfBooked,
        modality: None,
        rate_tag: None,
        location: sched_config.location.clone(),
        reschedule_for: None,
        sms_confirmation: None,
        notes: Some("Self-booked via portal".to_string()),
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    let appts_dir = std::path::PathBuf::from(&schedules_dir)
        .join(&practitioner)
        .join("appointments");
    std::fs::create_dir_all(&appts_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let path = appts_dir.join(format!("{}.yaml", appt.id));
    let yaml = serde_yaml::to_string(&appt)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    std::fs::write(&path, &yaml)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "ok": true,
        "appointment_id": appt.id.to_string(),
        "date": req.date,
        "time": req.time,
    })))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn rand_code() -> u32 {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    (seed % 900000) + 100000
}

fn load_practice_name() -> String {
    let config = crate::config::load_config();
    config.as_ref()
        .and_then(|c| c.get("practice"))
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("PracticeForge")
        .to_string()
}

fn lookup_client_name(client_id: &str) -> String {
    let clinical_root = crate::config::clinical_root();
    let identity_path = clinical_root.join("clients").join(client_id).join("identity.yaml");
    if identity_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&identity_path) {
            if let Ok(val) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
                if let Some(name) = val.get("name").and_then(|v| v.as_str()) {
                    return name.to_string();
                }
            }
        }
    }
    client_id.to_string()
}
