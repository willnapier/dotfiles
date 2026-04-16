//! Handler implementations for the admin dashboard API.

use axum::{
    extract::{Path, Query},
    http::StatusCode,
    Json,
};
use chrono::{Datelike, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::registry;
use crate::registry::config::RegistryConfig;
use crate::scheduling;

// ---------------------------------------------------------------------------
// JSON response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct ClientResponse {
    pub client_id: String,
    pub name: String,
    pub status: String,
    pub funding_type: Option<String>,
    pub funding_rate: Option<f64>,
    pub referrer_name: Option<String>,
    pub dob: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub diagnosis: Option<String>,
}

#[derive(Serialize)]
pub struct AssignmentResponse {
    pub practitioner_id: String,
    pub since: String,
    pub primary: bool,
}

#[derive(Serialize)]
pub struct CalendarEntry {
    pub date: String,
    pub start_time: String,
    pub end_time: String,
    pub client_id: String,
    pub client_name: String,
    pub practitioner: String,
    pub status: String,
}

#[derive(Serialize)]
pub struct SearchResultResponse {
    pub client_id: String,
    pub name: String,
    pub score: f32,
    pub snippet: String,
}

#[derive(Serialize)]
pub struct BillingStatusEntry {
    pub reference: String,
    pub client_id: String,
    pub client_name: String,
    pub bill_to: String,
    pub total: f64,
    pub currency: String,
    pub issue_date: String,
    pub due_date: String,
    pub state: String,
    pub days_overdue: i64,
}

#[derive(Serialize)]
pub struct BillingSummaryResponse {
    pub total_outstanding: f64,
    pub total_overdue: f64,
    pub outstanding_count: usize,
    pub overdue_count: usize,
}

#[derive(Serialize)]
pub struct PracticeResponse {
    pub name: String,
    pub address: Option<String>,
    pub phone: Option<String>,
    pub session_notes_mirror: bool,
}

#[derive(Serialize)]
pub struct PractitionerResponse {
    pub id: String,
    pub name: String,
    pub email: String,
    pub role: Option<String>,
}

// ---------------------------------------------------------------------------
// Query parameter types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CalendarQuery {
    pub date: Option<String>,
    pub week: Option<bool>,
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Deserialize)]
pub struct ClientUpdateRequest {
    pub status: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub diagnosis: Option<String>,
}

// ---------------------------------------------------------------------------
// Client handlers
// ---------------------------------------------------------------------------

/// GET /api/clients - List all clients from the registry.
pub async fn list_clients() -> Result<Json<Vec<ClientResponse>>, (StatusCode, String)> {
    let config = RegistryConfig::load();
    let clients = registry::list_clients(&config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response: Vec<ClientResponse> = clients
        .into_iter()
        .map(|c| ClientResponse {
            client_id: c.client_id,
            name: c.name,
            status: c.status,
            funding_type: c.funding.funding_type,
            funding_rate: c.funding.rate,
            referrer_name: c.referrer.name,
            dob: c.dob,
            phone: c.phone,
            email: c.email,
            diagnosis: c.diagnosis,
        })
        .collect();

    Ok(Json(response))
}

/// GET /api/clients/:id - Get a single client's details.
pub async fn get_client(
    Path(id): Path<String>,
) -> Result<Json<ClientResponse>, (StatusCode, String)> {
    let config = RegistryConfig::load();
    let c = registry::get_client(&config, &id)
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    Ok(Json(ClientResponse {
        client_id: c.client_id,
        name: c.name,
        status: c.status,
        funding_type: c.funding.funding_type,
        funding_rate: c.funding.rate,
        referrer_name: c.referrer.name,
        dob: c.dob,
        phone: c.phone,
        email: c.email,
        diagnosis: c.diagnosis,
    }))
}

/// POST /api/clients/:id - Update client fields.
pub async fn update_client(
    Path(id): Path<String>,
    Json(req): Json<ClientUpdateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let config = RegistryConfig::load();
    let mut client = registry::get_client(&config, &id)
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    if let Some(status) = req.status {
        client.status = status;
    }
    if let Some(phone) = req.phone {
        client.phone = Some(phone);
    }
    if let Some(email) = req.email {
        client.email = Some(email);
    }
    if let Some(diagnosis) = req.diagnosis {
        client.diagnosis = Some(diagnosis);
    }

    registry::client::save_client(&config, &client)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({"ok": true})))
}

/// GET /api/clients/:id/assignments - Get practitioner assignments for a client.
pub async fn get_assignments(
    Path(id): Path<String>,
) -> Result<Json<Vec<AssignmentResponse>>, (StatusCode, String)> {
    let config = RegistryConfig::load();
    let assignments = registry::client::get_assignments(&config, &id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response: Vec<AssignmentResponse> = assignments
        .into_iter()
        .map(|a| AssignmentResponse {
            practitioner_id: a.practitioner_id,
            since: a.since,
            primary: a.primary,
        })
        .collect();

    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// Calendar handler
// ---------------------------------------------------------------------------

/// GET /api/calendar - Load appointments for today (or a specific date/week).
pub async fn calendar(
    Query(params): Query<CalendarQuery>,
) -> Result<Json<Vec<CalendarEntry>>, (StatusCode, String)> {
    let base_date = match &params.date {
        Some(d) => NaiveDate::parse_from_str(d, "%Y-%m-%d")
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid date: {}", e)))?,
        None => Local::now().date_naive(),
    };

    let week = params.week.unwrap_or(false);
    let (from, to) = if week {
        let weekday = base_date.weekday().num_days_from_monday();
        let monday = base_date - chrono::Duration::days(weekday as i64);
        let friday = monday + chrono::Duration::days(4);
        (monday, friday)
    } else {
        (base_date, base_date)
    };

    let sched_config = scheduling::SchedulingConfig::default();
    let schedules_dir = shellexpand::tilde(&sched_config.schedules_dir).to_string();
    let schedules_path = std::path::PathBuf::from(&schedules_dir);

    let mut all_entries: Vec<CalendarEntry> = Vec::new();

    // Iterate over all practitioner directories in schedules/
    if schedules_path.exists() {
        let practitioner_dirs: Vec<_> = std::fs::read_dir(&schedules_path)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                if entry.file_type().ok()?.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !name.starts_with('.') {
                        return Some((name, entry.path()));
                    }
                }
                None
            })
            .collect();

        for (prac_name, prac_path) in &practitioner_dirs {
            let series_dir = prac_path.join("series");
            let appts_dir = prac_path.join("appointments");
            let holidays_path = prac_path.join("holidays.yaml");

            let series_list = scheduling::ics::load_series_dir(&series_dir)
                .unwrap_or_default();
            let one_offs = scheduling::ics::load_appointments_dir(&appts_dir)
                .unwrap_or_default();

            let holidays = if holidays_path.exists() {
                std::fs::read_to_string(&holidays_path)
                    .ok()
                    .and_then(|yaml| scheduling::ics::load_holidays(&yaml).ok())
                    .unwrap_or_default()
            } else {
                vec![]
            };

            // Materialise recurring series
            for s in &series_list {
                if s.status != scheduling::SeriesStatus::Active {
                    continue;
                }
                let dates = scheduling::recurrence::materialise(s, from, to, &holidays)
                    .unwrap_or_default();
                for d in dates {
                    all_entries.push(CalendarEntry {
                        date: d.format("%Y-%m-%d").to_string(),
                        start_time: s.start_time.format("%H:%M").to_string(),
                        end_time: s.end_time.format("%H:%M").to_string(),
                        client_id: s.client_id.clone(),
                        client_name: s.client_name.clone(),
                        practitioner: prac_name.clone(),
                        status: "recurring".to_string(),
                    });
                }
            }

            // One-off appointments in range
            for appt in &one_offs {
                if appt.date >= from && appt.date <= to
                    && appt.status != scheduling::AppointmentStatus::Cancelled
                {
                    all_entries.push(CalendarEntry {
                        date: appt.date.format("%Y-%m-%d").to_string(),
                        start_time: appt.start_time.format("%H:%M").to_string(),
                        end_time: appt.end_time.format("%H:%M").to_string(),
                        client_id: appt.client_id.clone(),
                        client_name: appt.client_name.clone(),
                        practitioner: prac_name.clone(),
                        status: appt.status.to_string(),
                    });
                }
            }
        }
    }

    // Sort by date, then start time
    all_entries.sort_by(|a, b| {
        a.date.cmp(&b.date).then_with(|| a.start_time.cmp(&b.start_time))
    });

    Ok(Json(all_entries))
}

// ---------------------------------------------------------------------------
// Search handler
// ---------------------------------------------------------------------------

/// GET /api/search?q=query - Full-text search across all client data.
pub async fn search(
    Query(params): Query<SearchQuery>,
) -> Result<Json<Vec<SearchResultResponse>>, (StatusCode, String)> {
    let query_str = params.q.unwrap_or_default();
    if query_str.trim().is_empty() {
        return Ok(Json(vec![]));
    }

    let limit = params.limit.unwrap_or(20);
    let search_config = crate::search::SearchConfig::load();

    // Auto-rebuild stale index
    let clinical_root = crate::search::index::resolve_clinical_root();
    let max_age = std::time::Duration::from_secs(3600);
    if crate::search::index::is_index_stale(&search_config, max_age) {
        let _ = crate::search::index::build_index(&search_config, &clinical_root);
    }

    let results = crate::search::query::search(&search_config, &query_str, limit)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response: Vec<SearchResultResponse> = results
        .into_iter()
        .map(|r| SearchResultResponse {
            client_id: r.client_id,
            name: r.name,
            score: r.score,
            snippet: r.snippet,
        })
        .collect();

    Ok(Json(response))
}

// ---------------------------------------------------------------------------
// Billing handlers
// ---------------------------------------------------------------------------

/// GET /api/billing/status - Outstanding invoices.
pub async fn billing_status() -> Result<Json<Vec<BillingStatusEntry>>, (StatusCode, String)> {
    let config = crate::billing::config::BillingConfig::load()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !config.enabled {
        return Ok(Json(vec![]));
    }

    let provider = crate::billing::ManualProvider::new(&config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    use crate::billing::traits::{AccountingProvider, InvoiceFilter};
    let invoices = provider
        .list_invoices(InvoiceFilter::default())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response: Vec<BillingStatusEntry> = invoices
        .into_iter()
        .filter(|i| {
            i.state != crate::billing::invoice::InvoiceState::Paid
                && i.state != crate::billing::invoice::InvoiceState::Cancelled
        })
        .map(|i| BillingStatusEntry {
            reference: i.reference,
            client_id: i.client_id,
            client_name: i.client_name,
            bill_to: i.bill_to_name,
            total: i.total,
            currency: i.currency,
            issue_date: i.issue_date,
            due_date: i.due_date,
            state: i.state.to_string(),
            days_overdue: i.days_overdue,
        })
        .collect();

    Ok(Json(response))
}

/// GET /api/billing/summary - Practice-wide billing summary.
pub async fn billing_summary() -> Result<Json<BillingSummaryResponse>, (StatusCode, String)> {
    let config = crate::billing::config::BillingConfig::load()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !config.enabled {
        return Ok(Json(BillingSummaryResponse {
            total_outstanding: 0.0,
            total_overdue: 0.0,
            outstanding_count: 0,
            overdue_count: 0,
        }));
    }

    let provider = crate::billing::ManualProvider::new(&config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    use crate::billing::traits::{AccountingProvider, InvoiceFilter};
    let all = provider
        .list_invoices(InvoiceFilter::default())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let outstanding: Vec<_> = all
        .iter()
        .filter(|i| {
            i.state != crate::billing::invoice::InvoiceState::Paid
                && i.state != crate::billing::invoice::InvoiceState::Cancelled
        })
        .collect();

    let overdue: Vec<_> = outstanding
        .iter()
        .filter(|i| i.state == crate::billing::invoice::InvoiceState::Overdue)
        .collect();

    let total_outstanding: f64 = outstanding.iter().map(|i| i.total).sum();
    let total_overdue: f64 = overdue.iter().map(|i| i.total).sum();

    Ok(Json(BillingSummaryResponse {
        total_outstanding,
        total_overdue,
        outstanding_count: outstanding.len(),
        overdue_count: overdue.len(),
    }))
}

// ---------------------------------------------------------------------------
// Practice info handlers
// ---------------------------------------------------------------------------

/// GET /api/practice - Practice configuration.
pub async fn practice_info() -> Result<Json<PracticeResponse>, (StatusCode, String)> {
    let reg_config = RegistryConfig::load();
    let practice_yaml_path = reg_config.config_dir().join("practice.yaml");

    if practice_yaml_path.exists() {
        let content = std::fs::read_to_string(&practice_yaml_path)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let config: registry::PracticeConfig = serde_yaml::from_str(&content)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        Ok(Json(PracticeResponse {
            name: config.name,
            address: config.address,
            phone: config.phone,
            session_notes_mirror: config.session_notes_mirror,
        }))
    } else {
        Ok(Json(PracticeResponse {
            name: String::new(),
            address: None,
            phone: None,
            session_notes_mirror: false,
        }))
    }
}

/// GET /api/practitioners - All practitioners registered in the practice.
pub async fn practitioners() -> Result<Json<Vec<PractitionerResponse>>, (StatusCode, String)> {
    let reg_config = RegistryConfig::load();
    let practitioners_yaml_path = reg_config.config_dir().join("practitioners.yaml");

    if practitioners_yaml_path.exists() {
        let content = std::fs::read_to_string(&practitioners_yaml_path)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let practitioners: Vec<registry::PractitionerInfo> = serde_yaml::from_str(&content)
            .unwrap_or_default();

        let response: Vec<PractitionerResponse> = practitioners
            .into_iter()
            .map(|p| PractitionerResponse {
                id: p.id,
                name: p.name,
                email: p.email,
                role: p.role,
            })
            .collect();

        Ok(Json(response))
    } else {
        Ok(Json(vec![]))
    }
}

// ---------------------------------------------------------------------------
// Clinic workflow types
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ClinicSession {
    pub date: String,
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub clients: Vec<ClinicClient>,
    #[serde(default)]
    pub clinic_ended: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ClinicClient {
    pub id: String,
    #[serde(default)]
    pub client_name: String,
    #[serde(default)]
    pub time: String,
    #[serde(default)]
    pub end_time: String,
    #[serde(default = "default_pending")]
    pub status: String,
    #[serde(default)]
    pub rate_tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_observation: Option<String>,
}

fn default_pending() -> String { "pending".to_string() }

#[derive(Deserialize)]
pub struct SessionQuery {
    pub date: Option<String>,
}

#[derive(Deserialize)]
pub struct GenerateRequest {
    pub client_id: String,
    pub observation: String,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Serialize)]
pub struct GenerateResponse {
    pub note_text: String,
    pub elapsed_seconds: f64,
}

#[derive(Deserialize)]
pub struct SaveNoteRequest {
    pub client_id: String,
    pub note_text: String,
}

#[derive(Serialize)]
pub struct ClientMetadata {
    pub sessions_count: usize,
    pub sessions_authorised: Option<u32>,
    pub sessions_used: Option<u32>,
    pub funding_type: Option<String>,
    pub letter_cadence_until: Option<i32>,
    pub letter_due: bool,
    pub referrer_name: Option<String>,
    pub referrer_practice: Option<String>,
    pub referrer_email: Option<String>,
}

#[derive(Serialize)]
pub struct InferenceStatus {
    pub available: bool,
}

#[derive(Deserialize)]
pub struct EndClinicRequest {
    pub session: ClinicSession,
}

#[derive(Serialize)]
pub struct EndClinicResponse {
    pub report: String,
    pub ok: bool,
}

// ---------------------------------------------------------------------------
// Session file helpers
// ---------------------------------------------------------------------------

fn session_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"))
        .join("practiceforge")
}

fn session_path(date: &str) -> PathBuf {
    session_dir().join(format!("session-{}.json", date))
}

// ---------------------------------------------------------------------------
// Clinic workflow handlers
// ---------------------------------------------------------------------------

/// GET /api/session?date=YYYY-MM-DD — load or create session for a date.
pub async fn get_session(
    Query(params): Query<SessionQuery>,
) -> Result<Json<ClinicSession>, (StatusCode, String)> {
    let date = params.date.unwrap_or_else(|| Local::now().format("%Y-%m-%d").to_string());
    let path = session_path(&date);

    if path.exists() {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let session: ClinicSession = serde_json::from_str(&content)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Invalid session JSON: {}", e)))?;
        Ok(Json(session))
    } else {
        Ok(Json(ClinicSession {
            date,
            started_at: Local::now().to_rfc3339(),
            clients: vec![],
            clinic_ended: false,
        }))
    }
}

/// PUT /api/session — persist the full session state.
pub async fn save_session(
    Json(session): Json<ClinicSession>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let dir = session_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let path = session_path(&session.date);
    let json = serde_json::to_string_pretty(&session)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    std::fs::write(&path, json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({"ok": true})))
}

/// POST /api/generate — run `clinical note` with observation, return generated note.
pub async fn generate_note(
    Json(req): Json<GenerateRequest>,
) -> Result<Json<GenerateResponse>, (StatusCode, String)> {
    let start = std::time::Instant::now();

    let mut cmd = tokio::process::Command::new("clinical");
    cmd.arg("note")
        .arg(&req.client_id)
        .arg(&req.observation)
        .arg("--no-save")
        .arg("--yes");

    if let Some(model) = &req.model {
        cmd.arg("--model-override").arg(model);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to run clinical note: {}", e)))?;

    let elapsed = start.elapsed().as_secs_f64();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("Generation failed: {}", stderr)));
    }

    let note_text = String::from_utf8_lossy(&output.stdout).trim().to_string();

    Ok(Json(GenerateResponse {
        note_text,
        elapsed_seconds: elapsed,
    }))
}

/// POST /api/save-note — run `clinical note-save` with note text on stdin.
pub async fn save_note(
    Json(req): Json<SaveNoteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use tokio::io::AsyncWriteExt;

    let mut child = tokio::process::Command::new("clinical")
        .arg("note-save")
        .arg(&req.client_id)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to run clinical note-save: {}", e)))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(req.note_text.as_bytes()).await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    let output = child.wait_with_output().await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("Save failed: {}", stderr)));
    }

    Ok(Json(serde_json::json!({"ok": true})))
}

/// GET /api/client/:id/notes — read the client's notes.md.
pub async fn get_client_notes(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let clinical_root = crate::config::clinical_root();
    let notes_path = clinical_root.join("clients").join(&id).join("notes.md");

    if !notes_path.exists() {
        return Ok(Json(serde_json::json!({"content": "", "exists": false})));
    }

    let content = std::fs::read_to_string(&notes_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({"content": content, "exists": true})))
}

/// GET /api/client/:id/metadata — funding badges, letter cadence, referrer info.
pub async fn get_client_metadata(
    Path(id): Path<String>,
) -> Result<Json<ClientMetadata>, (StatusCode, String)> {
    let clinical_root = crate::config::clinical_root();
    let client_dir = clinical_root.join("clients").join(&id);

    // Count sessions from notes.md
    let notes_path = client_dir.join("notes.md");
    let sessions_count = if notes_path.exists() {
        std::fs::read_to_string(&notes_path)
            .map(|content| content.lines().filter(|l| l.starts_with("### ")).count())
            .unwrap_or(0)
    } else {
        0
    };

    // Read identity.yaml for funding + referrer
    let identity_path = if client_dir.join("identity.yaml").exists() {
        client_dir.join("identity.yaml")
    } else {
        client_dir.join("private").join("identity.yaml")
    };

    let (sessions_authorised, sessions_used, funding_type, referrer_name, referrer_practice, referrer_email) =
        if identity_path.exists() {
            let content = std::fs::read_to_string(&identity_path).unwrap_or_default();
            let val: serde_yaml::Value = serde_yaml::from_str(&content).unwrap_or_default();

            let auth = val.get("authorisation")
                .and_then(|a| a.get("sessions_authorised"))
                .and_then(|v| v.as_u64().map(|n| n as u32));
            let used = val.get("authorisation")
                .and_then(|a| a.get("sessions_used"))
                .and_then(|v| v.as_u64().map(|n| n as u32));
            let ft = val.get("funding")
                .and_then(|f| f.get("type"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let rn = val.get("referrer")
                .and_then(|r| r.get("name"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let rp = val.get("referrer")
                .and_then(|r| r.get("practice"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let re = val.get("referrer")
                .and_then(|r| r.get("email"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            (auth, used, ft, rn, rp, re)
        } else {
            (None, None, None, None, None, None)
        };

    // Letter cadence from config
    let config = crate::config::load_config();
    let first_letter_after = config.as_ref()
        .and_then(|c| c.get("letters"))
        .and_then(|l| l.get("first_letter_after"))
        .and_then(|v| v.as_integer())
        .unwrap_or(2) as i32;
    let cycle_length = config.as_ref()
        .and_then(|c| c.get("letters"))
        .and_then(|l| l.get("cycle_length"))
        .and_then(|v| v.as_integer())
        .unwrap_or(6) as i32;

    let sc = sessions_count as i32;
    let (letter_cadence_until, letter_due) = if sc < first_letter_after {
        (Some(first_letter_after - sc), false)
    } else {
        let since_first = sc - first_letter_after;
        let remaining = cycle_length - (since_first % cycle_length);
        let remaining = if remaining == cycle_length { 0 } else { remaining };
        (Some(remaining), remaining == 0)
    };

    Ok(Json(ClientMetadata {
        sessions_count,
        sessions_authorised,
        sessions_used,
        funding_type,
        letter_cadence_until,
        letter_due,
        referrer_name,
        referrer_practice,
        referrer_email,
    }))
}

/// GET /api/inference/status — check if inference server is reachable.
pub async fn inference_status() -> Json<InferenceStatus> {
    let available = reqwest::Client::new()
        .get("http://localhost:11434/api/tags")
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    Json(InferenceStatus { available })
}

/// POST /api/end-clinic — generate attendance report and daypage entry.
pub async fn end_clinic(
    Json(req): Json<EndClinicRequest>,
) -> Result<Json<EndClinicResponse>, (StatusCode, String)> {
    let session = &req.session;
    let date = &session.date;

    // Parse date for display
    let display_date = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|d| d.format("%A %e %B %Y").to_string())
        .unwrap_or_else(|_| date.clone());

    // Build attendance report
    let mut report_lines = vec![format!("{} — Attendance\n", display_date)];
    let mut attended = 0;
    let mut dna_count = 0;
    let mut insurer_count = 0;
    let mut client_ids = Vec::new();

    for c in &session.clients {
        if c.status == "cancelled" { continue; }
        let icon = match c.status.as_str() {
            "done" => { attended += 1; client_ids.push(c.id.clone()); "✓" }
            "dna" => { dna_count += 1; "✗" }
            _ => "?"
        };
        if !c.rate_tag.is_empty() && c.rate_tag != "self-pay" && c.rate_tag != "Private" {
            insurer_count += 1;
        }
        let time_range = if c.end_time.is_empty() {
            c.time.clone()
        } else {
            format!("{}-{}", c.time, c.end_time)
        };
        let tag = if c.rate_tag.is_empty() { String::new() } else { format!(" {}", c.rate_tag) };
        report_lines.push(format!("{} {} {}{}", icon, time_range, c.id, tag));
    }

    let total = attended + dna_count;
    report_lines.push(String::new());
    report_lines.push(format!(
        "{}/{} attended · {} DNA/LC · {} insurer",
        attended, total, dna_count, insurer_count
    ));

    let report = report_lines.join("\n");

    // Save attendance report
    let attendance_dir = crate::config::clinical_root().join("attendance");
    std::fs::create_dir_all(&attendance_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    std::fs::write(attendance_dir.join(format!("{}.txt", date)), &report)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Calculate duration
    let started = chrono::DateTime::parse_from_rfc3339(&session.started_at)
        .map(|dt| dt.with_timezone(&Local))
        .unwrap_or_else(|_| Local::now());
    let minutes = (Local::now() - started).num_minutes();

    // Daypage entry
    let ids_str = client_ids.join(", ");
    let entry = format!("clinic:: {} clients {}min - {}", attended, minutes, ids_str);
    let _ = std::process::Command::new("daypage-append")
        .arg(&entry)
        .output();

    Ok(Json(EndClinicResponse { report, ok: true }))
}
