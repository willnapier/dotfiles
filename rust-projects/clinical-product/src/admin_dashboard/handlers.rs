//! Handler implementations for the admin dashboard API.

use axum::{
    extract::{Path, Query},
    http::StatusCode,
    Json,
};
use chrono::{Datelike, Local, NaiveDate};
use serde::{Deserialize, Serialize};

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
