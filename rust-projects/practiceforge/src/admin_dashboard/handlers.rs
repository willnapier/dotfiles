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
// ICS calendar export
// ---------------------------------------------------------------------------

/// GET /api/calendar/ics — export full calendar as an ICS file.
///
/// Returns text/calendar content suitable for import into any calendar app.
/// Query params: ?practitioner=name (optional, filter to one practitioner)
pub async fn calendar_ics(
    Query(params): Query<CalendarIcsQuery>,
) -> Result<(StatusCode, [(axum::http::HeaderName, &'static str); 2], String), (StatusCode, String)> {
    let sched_config = scheduling::SchedulingConfig::default();
    let schedules_dir = shellexpand::tilde(&sched_config.schedules_dir).to_string();
    let schedules_path = std::path::PathBuf::from(&schedules_dir);

    let mut all_series: Vec<scheduling::models::RecurringSeries> = Vec::new();
    let mut all_one_offs: Vec<scheduling::models::Appointment> = Vec::new();

    if schedules_path.exists() {
        for entry in std::fs::read_dir(&schedules_path)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            let entry = entry.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }

            // Filter by practitioner if requested
            if let Some(ref filter) = params.practitioner {
                if &name != filter {
                    continue;
                }
            }

            let prac_path = entry.path();
            let series = scheduling::ics::load_series_dir(&prac_path.join("series"))
                .unwrap_or_default();
            let appts = scheduling::ics::load_appointments_dir(&prac_path.join("appointments"))
                .unwrap_or_default();

            all_series.extend(series);
            all_one_offs.extend(appts);
        }
    }

    let cal_name = params
        .practitioner
        .as_deref()
        .unwrap_or("PracticeForge Calendar");

    let ics = scheduling::ics::full_calendar_to_ics(&all_series, &all_one_offs, cal_name);

    Ok((
        StatusCode::OK,
        [
            (axum::http::header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
            (axum::http::header::CONTENT_DISPOSITION, "attachment; filename=\"calendar.ics\""),
        ],
        ics,
    ))
}

#[derive(Deserialize)]
pub struct CalendarIcsQuery {
    pub practitioner: Option<String>,
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
// Billing action handlers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateInvoiceRequest {
    pub client_id: String,
    #[serde(default)]
    pub dates: Option<Vec<String>>,
}

/// POST /api/billing/invoice — create an invoice for a client's uninvoiced sessions.
pub async fn create_invoice(
    Json(req): Json<CreateInvoiceRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let config = crate::billing::config::BillingConfig::load()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !config.enabled {
        return Err((StatusCode::BAD_REQUEST, "Billing not enabled".to_string()));
    }

    let provider = crate::billing::ManualProvider::new(&config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let clinical_root = crate::config::clinical_root();
    let client_dir = clinical_root.join("clients").join(&req.client_id);
    let identity_path = if client_dir.join("identity.yaml").exists() {
        client_dir.join("identity.yaml")
    } else {
        client_dir.join("private").join("identity.yaml")
    };

    if !identity_path.exists() {
        return Err((StatusCode::NOT_FOUND, format!("No identity.yaml for {}", req.client_id)));
    }

    // Get billable sessions
    let uninvoiced: Vec<crate::billing::sessions::BillableSession> = if let Some(dates) = req.dates {
        dates
            .into_iter()
            .map(|date| crate::billing::sessions::BillableSession {
                date,
                reason: crate::billing::sessions::BillReason::Attended,
            })
            .collect()
    } else {
        let all_sessions = crate::billing::sessions::billable_sessions_for_client(
            &req.client_id,
            &crate::billing::sessions::default_session_dir(),
            Some(&crate::billing::sessions::default_schedules_dir()),
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let already_invoiced = {
            use crate::billing::traits::AccountingProvider;
            provider
                .invoiced_dates_for_client(&req.client_id)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        };

        crate::billing::sessions::uninvoiced_billable(&all_sessions, &already_invoiced)
    };

    if uninvoiced.is_empty() {
        return Ok(Json(serde_json::json!({"ok": false, "error": "No uninvoiced sessions"})));
    }

    use crate::billing::traits::AccountingProvider;
    let reference = provider.next_invoice_number()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let invoice = crate::billing::invoice::build_invoice(
        reference,
        &req.client_id,
        &identity_path,
        &uninvoiced,
        config.payment_terms_days,
        &config.currency,
    ).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let inv_ref = provider.create_invoice(&invoice)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "ok": true,
        "reference": inv_ref.reference,
        "total": invoice.total(),
        "sessions": uninvoiced.len()
    })))
}

/// POST /api/billing/invoice-batch — create invoices for all clients with uninvoiced sessions.
pub async fn create_invoice_batch() -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let config = crate::billing::config::BillingConfig::load()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !config.enabled {
        return Err((StatusCode::BAD_REQUEST, "Billing not enabled".to_string()));
    }

    let provider = crate::billing::ManualProvider::new(&config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let clinical_root = crate::config::clinical_root();
    let clients_dir = clinical_root.join("clients");
    let mut created = 0;
    let mut errors = Vec::new();

    if clients_dir.exists() {
        use crate::billing::traits::AccountingProvider;
        for entry in std::fs::read_dir(&clients_dir).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
            let entry = entry.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            let client_id = entry.file_name().to_string_lossy().to_string();
            let client_dir = entry.path();
            let identity_path = if client_dir.join("identity.yaml").exists() {
                client_dir.join("identity.yaml")
            } else {
                client_dir.join("private").join("identity.yaml")
            };

            if !identity_path.exists() { continue; }

            let all_sessions = match crate::billing::sessions::billable_sessions_for_client(
                &client_id,
                &crate::billing::sessions::default_session_dir(),
                Some(&crate::billing::sessions::default_schedules_dir()),
            ) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let already_invoiced = provider
                .invoiced_dates_for_client(&client_id)
                .unwrap_or_default();
            let uninvoiced = crate::billing::sessions::uninvoiced_billable(&all_sessions, &already_invoiced);

            if uninvoiced.is_empty() { continue; }

            let reference = match provider.next_invoice_number() {
                Ok(r) => r,
                Err(e) => { errors.push(format!("{}: {}", client_id, e)); continue; }
            };

            match crate::billing::invoice::build_invoice(reference, &client_id, &identity_path, &uninvoiced, config.payment_terms_days, &config.currency) {
                Ok(invoice) => {
                    match provider.create_invoice(&invoice) {
                        Ok(_) => { created += 1; }
                        Err(e) => { errors.push(format!("{}: {}", client_id, e)); }
                    }
                }
                Err(e) => { errors.push(format!("{}: {}", client_id, e)); }
            }
        }
    }

    Ok(Json(serde_json::json!({
        "ok": true,
        "created": created,
        "errors": errors
    })))
}

#[derive(Deserialize)]
pub struct MarkPaidRequest {
    pub reference: String,
    #[serde(default)]
    pub date: Option<String>,
}

/// POST /api/billing/paid — mark an invoice as paid.
pub async fn mark_paid(
    Json(req): Json<MarkPaidRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let config = crate::billing::config::BillingConfig::load()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let provider = crate::billing::ManualProvider::new(&config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    use crate::billing::traits::AccountingProvider;
    let date = req.date.unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string());
    provider.mark_paid(&req.reference, &date, None)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({"ok": true})))
}

#[derive(Deserialize)]
pub struct CancelInvoiceRequest {
    pub reference: String,
    #[serde(default = "default_cancel_reason")]
    pub reason: String,
}

fn default_cancel_reason() -> String { "Cancelled".to_string() }

/// POST /api/billing/cancel — cancel an invoice.
pub async fn cancel_invoice(
    Json(req): Json<CancelInvoiceRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let config = crate::billing::config::BillingConfig::load()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let provider = crate::billing::ManualProvider::new(&config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    use crate::billing::traits::AccountingProvider;
    provider.cancel_invoice(&req.reference, &req.reason)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({"ok": true})))
}

/// GET /api/billing/reminders — list reminders due.
pub async fn list_reminders() -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let config = crate::billing::config::BillingConfig::load()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !config.enabled {
        return Ok(Json(serde_json::json!({"reminders": []})));
    }

    let provider = crate::billing::ManualProvider::new(&config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    use crate::billing::traits::{AccountingProvider, InvoiceFilter};
    let all = provider.list_invoices(InvoiceFilter { overdue_only: true, ..Default::default() })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let reminders = crate::billing::remind::due_reminders(&config, &all);

    let reminder_list: Vec<serde_json::Value> = reminders.iter().map(|(inv, tone)| {
        serde_json::json!({
            "reference": inv.reference,
            "client_id": inv.client_id,
            "client_name": inv.client_name,
            "days_overdue": inv.days_overdue,
            "tone": tone,
            "total": inv.total,
        })
    }).collect();

    Ok(Json(serde_json::json!({"reminders": reminder_list})))
}

/// POST /api/billing/reminders/send — send a specific reminder by invoice reference.
///
/// Request body: { "reference": "INV-2026-0001" }
/// Sends the reminder email and logs it. Returns success/failure.
pub async fn send_reminder(
    Json(req): Json<SendReminderRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let config = crate::billing::config::BillingConfig::load()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !config.enabled {
        return Err((StatusCode::BAD_REQUEST, "Billing is not enabled".to_string()));
    }

    let provider = crate::billing::ManualProvider::new(&config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Find the specific overdue invoice
    use crate::billing::traits::{AccountingProvider, InvoiceFilter};
    let overdue = provider
        .list_invoices(InvoiceFilter {
            overdue_only: true,
            ..Default::default()
        })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let inv = overdue
        .iter()
        .find(|i| i.reference == req.reference)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("Invoice {} not found or not overdue", req.reference),
            )
        })?;

    // Get the reminder tone for this invoice
    let tone = crate::billing::remind::next_reminder_tone(&config, inv).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!("No reminder due for {}", req.reference),
        )
    })?;

    // Load email config
    let email_config = crate::email::load_email_config().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Email not configured: {}", e),
        )
    })?;

    let practitioner = email_config.from_name.clone();

    let is_insurer = !inv.bill_to_name.is_empty() && inv.bill_to_name != inv.client_name;
    let reminder = if is_insurer {
        crate::billing::remind::render_insurer_reminder(inv, &practitioner)
    } else {
        crate::billing::remind::render_client_reminder(inv, &tone, &practitioner, "")
    };

    let to_email = reminder.to_email.as_deref().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!("No email address for {}", reminder.to_name),
        )
    })?;

    // Send the email
    crate::email::send_email(
        &email_config,
        to_email,
        &reminder.to_name,
        &reminder.subject,
        &reminder.body,
        None,
        None,
    )
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to send: {}", e),
        )
    })?;

    // Log the sent reminder
    let now = chrono::Local::now()
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string();
    let log_entry = crate::billing::ReminderLogEntry {
        reference: inv.reference.clone(),
        sent_at: now,
        tone: tone.clone(),
        to_email: to_email.to_string(),
        to_name: reminder.to_name.clone(),
    };
    provider.log_reminder(log_entry).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Sent but failed to log: {}", e),
        )
    })?;

    Ok(Json(serde_json::json!({
        "ok": true,
        "reference": inv.reference,
        "to_email": to_email,
        "tone": tone,
    })))
}

#[derive(Deserialize)]
pub struct SendReminderRequest {
    pub reference: String,
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

fn default_true() -> bool { true }

#[derive(Deserialize)]
pub struct GenerateRequest {
    pub client_id: String,
    pub observation: String,
    #[serde(default)]
    pub model: Option<String>,
    /// Whether to apply Prompt-Rail grounding constraints. Default true.
    /// Pass false to generate the no-rail variant for A/B analysis.
    #[serde(default = "default_true")]
    pub with_rail: bool,
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

/// Payload for /api/log-pair — records a Q4/Q8 comparison from the dashboard's
/// side-by-side view. Both notes are appended to ~/Clinical/comparisons.jsonl
/// with the user's accepted choice (or None if both were rejected).
#[derive(Deserialize)]
pub struct LogPairRequest {
    pub client_id: String,
    pub observation: String,
    pub q4_note: String,
    pub q4_generation_secs: f64,
    pub q8_note: String,
    pub q8_generation_secs: f64,
    /// "q4", "q8", or null (both rejected)
    #[serde(default)]
    pub accepted: Option<String>,
    /// Whether Prompt-Rail was active for this pair. Default true.
    #[serde(default = "default_true")]
    pub prompt_rail: bool,
}

/// One row in ~/Clinical/comparisons.jsonl. Mirrors the CLI schema
/// (clinical::faithfulness::ComparisonEntry) so downstream analysis
/// tooling can consume both CLI- and dashboard-generated rows.
#[derive(Serialize)]
struct ComparisonEntry {
    timestamp: String,
    client_id: String,
    model: String,
    observation: String,
    note: String,
    hard_failures: usize,
    soft_flags: usize,
    flag_details: Vec<String>,
    attempts: usize,
    regen_reasons: Vec<String>,
    generation_secs: f64,
    accepted: Option<bool>,
    prompt_rail: bool,
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
    /// Whether any backend is configured (frontier or Ollama).
    /// False only when nothing is set up at all.
    pub configured: bool,
    /// "anthropic", "claude-cli", "ollama", or "none"
    pub backend: String,
    /// Active model name, if a frontier backend is configured.
    pub model: Option<String>,
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
        let mut session: ClinicSession = serde_json::from_str(&content)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Invalid session JSON: {}", e)))?;
        session.clients.sort_by(|a, b| a.time.cmp(&b.time));
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

    // Frontier backend (Anthropic API or Claude CLI subscription).
    if let Some(backend) = crate::llm::load_backend() {
        let (prompt, pmap) = build_fallback_prompt(&req.client_id, &req.observation, req.with_rail);
        let mut system_prompt = load_system_prompt();
        if let Some(addendum) = pmap.system_addendum() {
            system_prompt.push_str(&addendum);
        }
        let raw = backend
            .generate(&system_prompt, &prompt)
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
        let note_text = pmap.revert(&raw);
        return Ok(Json(GenerateResponse {
            note_text,
            elapsed_seconds: start.elapsed().as_secs_f64(),
        }));
    }

    // Ollama/CLI fallback path.
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

/// POST /api/generate-stream — streaming note generation via SSE.
///
/// Uses the Anthropic backend when configured (`[ai] backend = "anthropic"`);
/// falls back to direct Ollama streaming otherwise. The Ollama path remains
/// in archive posture — functional but not the production default.
pub async fn generate_note_stream(
    Json(req): Json<GenerateRequest>,
) -> Result<axum::response::Sse<axum::response::sse::KeepAliveStream<futures_util::stream::BoxStream<'static, Result<axum::response::sse::Event, std::convert::Infallible>>>>, (StatusCode, String)> {
    use axum::response::sse::{Event, KeepAlive};
    use futures_util::StreamExt;

    let start = std::time::Instant::now();
    let client_id = req.client_id.clone();
    let observation = req.observation.clone();
    let with_rail = req.with_rail;

    let (prompt, pmap) = build_fallback_prompt(&client_id, &observation, with_rail);
    let mut system_prompt = load_system_prompt();
    if let Some(addendum) = pmap.system_addendum() {
        system_prompt.push_str(&addendum);
    }

    // Frontier backend (Anthropic API or Claude CLI subscription).
    if let Some(backend) = crate::llm::load_backend() {
        eprintln!("[gen] {} client={client_id}", backend.backend_name());
        let token_stream = backend
            .generate_stream(system_prompt, prompt)
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;

        // Revert placeholders server-side as each chunk arrives.
        // Rare chunk-boundary straddle (e.g. "[CLI" + "ENT]") is handled by
        // the client-side JS final revert on [DONE].
        let sse_stream = token_stream.map(move |tokens| {
            if tokens.is_empty() {
                Ok(Event::default().comment("keepalive"))
            } else {
                Ok(Event::default().data(pmap.revert(&tokens)))
            }
        });

        let done_event = futures_util::stream::once(async move {
            Ok(Event::default().data(format!("[DONE] {:.1}s", start.elapsed().as_secs_f64())))
        });

        let combined = sse_stream.chain(done_event).boxed();
        return Ok(axum::response::Sse::new(combined).keep_alive(KeepAlive::default()));
    }

    // Ollama fallback path (archive posture)
    let config = crate::config::load_config();
    let endpoint = config.as_ref()
        .and_then(|c| c.get("voice"))
        .and_then(|v| v.get("endpoint"))
        .and_then(|v| v.as_str())
        .unwrap_or("http://localhost:11434")
        .to_string();
    let model = req.model.clone().unwrap_or_else(|| {
        config.as_ref()
            .and_then(|c| c.get("voice"))
            .and_then(|v| v.get("model"))
            .and_then(|v| v.as_str())
            .unwrap_or("clinical-voice-q4")
            .to_string()
    });

    eprintln!("[gen] ollama client={client_id} endpoint={endpoint}");
    let ollama_url = format!("{}/api/generate", endpoint);
    let body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "system": system_prompt,
        "stream": true,
        "keep_alive": -1,
        "options": {"num_ctx": 4096, "repeat_penalty": 1.15},
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&ollama_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Ollama connection failed: {}. Is the inference tunnel up?", e)))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err((StatusCode::BAD_GATEWAY, format!("Ollama returned {}: {}", status, body)));
    }

    let byte_stream = resp.bytes_stream();
    let stream = byte_stream.map(move |chunk| {
        let chunk = chunk.unwrap_or_default();
        let text = String::from_utf8_lossy(&chunk);
        let mut tokens = String::new();
        let mut is_done = false;

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() { continue; }
            if let Ok(obj) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(tok) = obj.get("response").and_then(|v| v.as_str()) {
                    tokens.push_str(tok);
                }
                if obj.get("done").and_then(|v| v.as_bool()).unwrap_or(false) {
                    is_done = true;
                }
            }
        }

        if is_done {
            let elapsed = start.elapsed().as_secs_f64();
            Ok(Event::default().data(format!("[DONE] {:.1}s", elapsed)))
        } else if !tokens.is_empty() {
            Ok(Event::default().data(tokens))
        } else {
            Ok(Event::default().comment("keepalive"))
        }
    }).boxed();

    Ok(axum::response::Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// Load system prompt from modality + faithfulness prompt files.
fn load_system_prompt() -> String {
    let home = dirs::home_dir().unwrap_or_default();
    let prompts_dir = crate::config::config_dir().join("prompts");
    let skills_dir = home.join(".claude/skills/clinical-notes");

    // [a] Therapeutic model
    let model_text = read_first_existing(&[
        prompts_dir.join("therapeutic_model.md"),
        skills_dir.join("modality-act.md"),
    ]);

    // [b] Practitioner additions (optional, no fallback)
    let additions_text = read_first_existing(&[
        prompts_dir.join("practitioner_additions.md"),
    ]);

    // [c] Rails — universal grounding rules
    let rails_text = read_first_existing(&[
        prompts_dir.join("rails.md"),
        skills_dir.join("faithfulness-prompt.md"),
    ]);

    if model_text.is_empty() && rails_text.is_empty() {
        return "You are a clinical psychologist's session note writer. \
                Produce a session note in the practitioner's established style. \
                Frame clinical reasoning using explicit ACT/CBS process terminology.".to_string();
    }

    let mut parts: Vec<&str> = Vec::new();
    if !model_text.is_empty() { parts.push(model_text.trim()); }
    if !additions_text.is_empty() { parts.push(additions_text.trim()); }
    if !rails_text.is_empty() { parts.push(rails_text.trim()); }
    parts.join("\n\n")
}

fn read_first_existing(paths: &[std::path::PathBuf]) -> String {
    for p in paths {
        if let Ok(s) = std::fs::read_to_string(p) {
            if !s.trim().is_empty() {
                return s;
            }
        }
    }
    String::new()
}

/// Build a de-identified prompt for the AI backend.
///
/// Returns the prompt text (with all PHI replaced by placeholders) and the
/// PseudonymMap needed to revert the AI output before display or saving.
/// Scan identity.yaml for correspondence entries with type: referral or type: intake.
/// Returns filenames in order encountered.
fn referral_files_from_identity(content: &str) -> Vec<String> {
    let mut in_correspondence = false;
    let mut pending_file: Option<String> = None;
    let mut results = Vec::new();

    for line in content.lines() {
        if line.trim_start().starts_with('#') { continue; }
        let trimmed = line.trim_start();

        if !in_correspondence {
            if trimmed.starts_with("correspondence:") {
                in_correspondence = true;
            }
        } else {
            if !line.is_empty() && !line.starts_with(' ') && !line.starts_with('\t') {
                break;
            }
            if let Some(rest) = trimmed.strip_prefix("- file:") {
                // New entry — commit any prior file that was waiting without a match
                pending_file = Some(rest.trim().trim_matches('"').trim_matches('\'').to_string());
            } else if let Some(rest) = trimmed.strip_prefix("type:") {
                let t = rest.trim().trim_matches('"').trim_matches('\'');
                if t == "referral" || t == "intake" {
                    if let Some(f) = pending_file.take() {
                        results.push(f);
                    }
                } else {
                    pending_file = None;
                }
            }
        }
    }
    results
}

fn build_fallback_prompt(
    client_id: &str,
    observation: &str,
    with_rail: bool,
) -> (String, crate::dephi::PseudonymMap) {
    let clinical_root = crate::config::clinical_root();
    let client_dir = clinical_root.join("clients").join(client_id);

    // Build the pseudonym map from identity.yaml before loading any other text.
    let identity_path = client_dir.join("identity.yaml");
    let pmap = crate::dephi::PseudonymMap::from_identity_file(&identity_path);

    // Referral letter — first correspondence entry with type: referral or type: intake
    let referral_context = {
        let identity_content = std::fs::read_to_string(&identity_path).unwrap_or_default();
        let referral_files = referral_files_from_identity(&identity_content);
        let corr_dir = client_dir.join("correspondence");
        let mut text = String::new();
        for fname in referral_files {
            let path = corr_dir.join(&fname);
            if let Ok(content) = std::fs::read_to_string(&path) {
                if !content.trim().is_empty() {
                    text = pmap.apply(&content);
                    break; // first referral only
                }
            }
        }
        text
    };

    // Summary
    let summary_path = client_dir.join("summary.md");
    let summary = std::fs::read_to_string(&summary_path).unwrap_or_default();
    let summary = pmap.apply(&summary);

    // Full session notes history
    let notes_path = client_dir.join("notes.md");
    let session_notes = if notes_path.exists() {
        let content = std::fs::read_to_string(&notes_path).unwrap_or_default();
        if content.trim().is_empty() {
            String::new()
        } else {
            pmap.apply(&content)
        }
    } else {
        String::new()
    };

    // De-identify the observation too (belt-and-suspenders for practitioner slip).
    let observation = pmap.apply(observation);

    // Use [CLIENT] placeholder as the client identifier in the prompt header.
    let client_label = if pmap.is_empty() { client_id.to_string() } else { "[CLIENT]".to_string() };
    let mut prompt = format!("Client: {} ({})\n\n", client_label, client_id);

    if !referral_context.is_empty() {
        prompt.push_str("Referral context:\n");
        prompt.push_str(&referral_context);
        prompt.push_str("\n\n");
    }

    if !summary.is_empty() {
        prompt.push_str("Clinical summary:\n");
        prompt.push_str(&summary);
        prompt.push_str("\n\n");
    }

    if !session_notes.is_empty() {
        prompt.push_str("Session notes:\n");
        prompt.push_str(&session_notes);
        prompt.push_str("\n\n");
    }

    // Prompt-Rail: grounding constraints to prevent confabulation.
    // Skipped when with_rail=false (norail A/B variant).
    if with_rail {
        let obs_lower = observation.to_lowercase();
        let mut absences = Vec::new();
        if !obs_lower.contains("homework") && !obs_lower.contains("between-session") && !obs_lower.contains("task") {
            absences.push("No homework or between-session tasks were discussed");
        }
        if !obs_lower.contains("metaphor") && !obs_lower.contains("exercise") && !obs_lower.contains("experiential") {
            absences.push("No specific metaphors or experiential exercises were used");
        }
        if !obs_lower.contains("risk") && !obs_lower.contains("suicid") && !obs_lower.contains("harm") {
            absences.push("No risk factors were noted — use brief default risk statement");
        }
        if !absences.is_empty() {
            prompt.push_str("\nGROUNDING CONSTRAINTS:\n");
            for a in &absences {
                prompt.push_str("- ");
                prompt.push_str(a);
                prompt.push('\n');
            }
            prompt.push_str("Do not invent details not present in the observation.\n\n");
        }
    }

    let today = {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let days = secs / 86400;
        // Days since 1970-01-01 → Gregorian date
        let mut y = 1970u32;
        let mut d = days as u32;
        loop {
            let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
            let days_in_year = if leap { 366 } else { 365 };
            if d < days_in_year { break; }
            d -= days_in_year;
            y += 1;
        }
        let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
        let month_days = [31u32, if leap { 29 } else { 28 }, 31,30,31,30,31,31,30,31,30,31];
        let mut m = 1u32;
        for md in &month_days {
            if d < *md { break; }
            d -= md;
            m += 1;
        }
        format!("{:04}-{:02}-{:02}", y, m, d + 1)
    };

    prompt.push_str("Today's observation:\n");
    prompt.push_str(&observation);
    prompt.push_str(&format!("\n\nWrite the session note using exactly this template (date is {today}):\n\
### {today}\n\
\n\
**Risk**: \n\
\n\
[narrative]\n\
\n\
**Formulation**: "));

    (prompt, pmap)
}

/// POST /api/save-note — run `clinical note-save` with note text on stdin.
pub async fn save_note(
    Json(req): Json<SaveNoteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use tokio::io::AsyncWriteExt;

    // Final revert pass — catches any placeholders that slipped through
    // chunk-boundary straddle in streaming or were missed client-side.
    let identity_path = crate::config::clinical_root()
        .join("clients").join(&req.client_id).join("identity.yaml");
    let pmap = crate::dephi::PseudonymMap::from_identity_file(&identity_path);
    let note_text = pmap.revert(&req.note_text);

    let mut child = tokio::process::Command::new("clinical")
        .arg("note-save")
        .arg(&req.client_id)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to run clinical note-save: {}", e)))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(note_text.as_bytes()).await
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

/// POST /api/log-pair — append a Q4/Q8 comparison pair to ~/Clinical/comparisons.jsonl.
///
/// Called by the dashboard's side-by-side compare view once the practitioner
/// has accepted one variant or rejected both. Faithfulness stats (hard_failures,
/// soft_flags, attempts, regen_reasons) aren't tracked at this layer — analysis
/// tools that want them can re-run `check_faithfulness` offline against stored
/// (observation, note) pairs.
pub async fn log_pair(
    Json(req): Json<LogPairRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if !req
        .client_id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '+')
    {
        return Err((StatusCode::BAD_REQUEST, "Invalid client ID".to_string()));
    }

    let home = dirs::home_dir()
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "No home dir".to_string()))?;
    let path = home.join("Clinical/comparisons.jsonl");

    let now = chrono::Local::now()
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string();

    let q4_accepted = req.accepted.as_deref() == Some("q4");
    let q8_accepted = req.accepted.as_deref() == Some("q8");

    let q4_entry = ComparisonEntry {
        timestamp: now.clone(),
        client_id: req.client_id.clone(),
        model: "clinical-voice-q4".to_string(),
        observation: req.observation.clone(),
        note: req.q4_note,
        hard_failures: 0,
        soft_flags: 0,
        flag_details: Vec::new(),
        attempts: 0,
        regen_reasons: Vec::new(),
        generation_secs: req.q4_generation_secs,
        accepted: Some(q4_accepted),
        prompt_rail: req.prompt_rail,
    };
    let q8_entry = ComparisonEntry {
        timestamp: now,
        client_id: req.client_id.clone(),
        model: "clinical-voice-q8".to_string(),
        observation: req.observation,
        note: req.q8_note,
        hard_failures: 0,
        soft_flags: 0,
        flag_details: Vec::new(),
        attempts: 0,
        regen_reasons: Vec::new(),
        generation_secs: req.q8_generation_secs,
        accepted: Some(q8_accepted),
        prompt_rail: req.prompt_rail,
    };

    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("open jsonl: {e}")))?;

    for entry in [&q4_entry, &q8_entry] {
        let line = serde_json::to_string(entry)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("serialize: {e}")))?;
        file.write_all(line.as_bytes())
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("write jsonl: {e}")))?;
        file.write_all(b"\n")
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("write jsonl: {e}")))?;
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
    // Frontier backends (Anthropic API, Claude CLI) are always reachable — no local server.
    if let Some(b) = crate::llm::load_backend() {
        let ai = crate::config::load_ai_config();
        return Json(InferenceStatus {
            available: true,
            configured: true,
            backend: b.backend_name().to_string(),
            model: ai.model,
        });
    }

    let client = reqwest::Client::new();

    let available = client
        .get("http://localhost:11434/api/tags")
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    if available {
        // Check if clinical model is loaded; if cold, warm it up in background.
        // Prevents the 5-8s cold-start penalty on the first generation.
        let needs_warmup = match client
            .get("http://localhost:11434/api/ps")
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await
        {
            Ok(resp) => match resp.json::<serde_json::Value>().await {
                Ok(val) => {
                    val.get("models")
                        .and_then(|m| m.as_array())
                        .map(|a| a.is_empty())
                        .unwrap_or(true)
                }
                Err(_) => true,
            },
            Err(_) => false,
        };

        if needs_warmup {
            // Warm both Q4 and Q8 — keep_alive: -1 pins them indefinitely.
            // Use stream:true so Ollama sends back headers immediately after first
            // token, preventing this warmup from blocking the generate queue.
            for model in ["clinical-voice-q4", "clinical-voice-q8"] {
                let model = model.to_string();
                tokio::spawn(async move {
                    use futures_util::StreamExt;
                    let Ok(resp) = reqwest::Client::new()
                        .post("http://127.0.0.1:11434/api/generate")
                        .json(&serde_json::json!({
                            "model": model,
                            "prompt": "warmup",
                            "keep_alive": -1,
                            "stream": true,
                            "options": {"num_predict": 3},
                        }))
                        .timeout(std::time::Duration::from_secs(180))
                        .send()
                        .await else { return; };
                    // Drain 3 chunks then drop — warms GPU caches without occupying queue long.
                    let mut stream = resp.bytes_stream();
                    let mut count = 0;
                    while let Some(Ok(_)) = stream.next().await {
                        count += 1;
                        if count >= 3 { break; }
                    }
                });
            }
        }
    }

    let backend = if available { "ollama" } else { "none" }.to_string();
    Json(InferenceStatus { available, configured: available, backend, model: None })
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

    // Clinic data lives in PracticeForge session files — no DayPage entry needed.
    // The attendance report at ~/Clinical/attendance/{date}.txt is the permanent record.

    Ok(Json(EndClinicResponse { report, ok: true }))
}

// ---------------------------------------------------------------------------
// Auth handlers
// ---------------------------------------------------------------------------

use std::sync::Mutex;

lazy_static::lazy_static! {
    /// OTP codes: email → (code, expires_at)
    static ref AUTH_CODES: Mutex<std::collections::HashMap<String, (String, chrono::DateTime<chrono::Utc>)>> =
        Mutex::new(std::collections::HashMap::new());
    /// Valid session tokens: token → (email, created_at)
    static ref AUTH_SESSIONS: Mutex<std::collections::HashMap<String, (String, chrono::DateTime<chrono::Utc>)>> =
        Mutex::new(std::collections::HashMap::new());
}

/// Check if a session token is valid (not expired — 30 day max).
pub fn validate_session_token(token: &str) -> bool {
    let sessions = AUTH_SESSIONS.lock().unwrap();
    if let Some((_email, created)) = sessions.get(token) {
        let age = chrono::Utc::now() - *created;
        age.num_days() < 30
    } else {
        false
    }
}

/// Send an OTP code to the practitioner's email.
pub async fn auth_send_code_handler(
    email: &str,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Generate 6-digit code
    let code = format!("{:06}", {
        use std::time::SystemTime;
        let seed = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        (seed % 900000) + 100000
    });

    let expires = chrono::Utc::now() + chrono::Duration::minutes(10);
    AUTH_CODES.lock().unwrap().insert(email.to_string(), (code.clone(), expires));

    // Try to send via email
    match crate::email::load_email_config() {
        Ok(config) => {
            let body = format!(
                "Your PracticeForge login code is: {}\n\nThis code is valid for 10 minutes.",
                code
            );
            match crate::email::send_email(
                &config,
                email,
                "",
                "PracticeForge Login Code",
                &body,
                None,
                None,
            ) {
                Ok(_) => Ok(Json(serde_json::json!({"ok": true}))),
                Err(e) => {
                    // Email failed — log the code for dev use
                    eprintln!("[auth] OTP for {}: {} (email send failed: {})", email, code, e);
                    Ok(Json(serde_json::json!({"ok": true, "dev_note": "email failed, check server logs"})))
                }
            }
        }
        Err(_) => {
            // No email configured — log code to stderr (dev mode)
            eprintln!("[auth] OTP for {}: {} (email not configured)", email, code);
            Ok(Json(serde_json::json!({"ok": true, "dev_code": code})))
        }
    }
}

/// Verify an OTP code and return a session token.
pub fn auth_verify_handler(
    email: &str,
    code: &str,
) -> Result<String, (StatusCode, String)> {
    let codes = AUTH_CODES.lock().unwrap();
    let (stored_code, expires) = codes
        .get(email)
        .ok_or((StatusCode::BAD_REQUEST, "No code sent to this email".to_string()))?;

    if chrono::Utc::now() > *expires {
        return Err((StatusCode::BAD_REQUEST, "Code expired — request a new one".to_string()));
    }

    if stored_code != code {
        return Err((StatusCode::BAD_REQUEST, "Invalid code".to_string()));
    }

    drop(codes);

    // Create session token
    let token = uuid::Uuid::new_v4().to_string();
    AUTH_SESSIONS.lock().unwrap().insert(
        token.clone(),
        (email.to_string(), chrono::Utc::now()),
    );

    Ok(token)
}

// ---------------------------------------------------------------------------
// Scheduling action handlers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ScheduleCreateRequest {
    pub client_id: String,
    pub date: String,
    pub time: String,
    #[serde(default = "default_duration")]
    pub duration: u32,
    /// "weekly", "fortnightly", "every3w", "monthly", or null for one-off.
    #[serde(default)]
    pub recur: Option<String>,
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub infinite: bool,
    #[serde(default)]
    pub practitioner: Option<String>,
}

fn default_duration() -> u32 { 50 }

/// POST /api/schedule/create — create appointment or recurring series.
pub async fn schedule_create(
    Json(req): Json<ScheduleCreateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use chrono::NaiveTime;

    let sched_config = scheduling::SchedulingConfig::default();
    let schedules_dir = shellexpand::tilde(&sched_config.schedules_dir).to_string();
    let prac = req.practitioner.as_deref().unwrap_or(&sched_config.default_practitioner);

    let date = NaiveDate::parse_from_str(&req.date, "%Y-%m-%d")
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid date: {}", e)))?;
    let start_time = NaiveTime::parse_from_str(&req.time, "%H:%M")
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid time: {}", e)))?;
    let end_time = start_time + chrono::Duration::minutes(req.duration as i64);

    // Look up client name from registry or identity.yaml
    let client_name = lookup_client_name(&req.client_id);

    let (freq, interval) = match req.recur.as_deref() {
        Some("weekly") => (Some(scheduling::Frequency::Weekly), 1u32),
        Some("fortnightly") => (Some(scheduling::Frequency::Weekly), 2),
        Some("every3w") => (Some(scheduling::Frequency::Weekly), 3),
        Some("monthly") => (Some(scheduling::Frequency::Monthly), 1),
        _ => (None, 1),
    };

    if let Some(freq) = freq {
        let series_count = if req.infinite { None } else { req.count };
        let series = scheduling::RecurringSeries {
            id: uuid::Uuid::new_v4(),
            practitioner: prac.to_string(),
            client_id: req.client_id.clone(),
            client_name: client_name.clone(),
            start_time,
            end_time,
            location: sched_config.location.clone(),
            modality: None,
            rate_tag: None,
            recurrence: scheduling::RecurrenceRule {
                freq,
                interval,
                by_day: None,
                dtstart: date,
                until: None,
                count: series_count,
            },
            exdates: vec![],
            status: scheduling::SeriesStatus::Active,
            created_at: chrono::Utc::now().to_rfc3339(),
            notes: None,
        };

        let series_dir = std::path::PathBuf::from(&schedules_dir).join(prac).join("series");
        std::fs::create_dir_all(&series_dir)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let path = series_dir.join(format!("{}.yaml", series.id));
        let yaml = serde_yaml::to_string(&series)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        std::fs::write(&path, &yaml)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        Ok(Json(serde_json::json!({
            "ok": true,
            "type": "series",
            "series_id": series.id.to_string(),
            "client_name": client_name,
        })))
    } else {
        // One-off appointment
        let appt = scheduling::Appointment {
            id: uuid::Uuid::new_v4(),
            series_id: None,
            practitioner: prac.to_string(),
            client_id: req.client_id.clone(),
            client_name: client_name.clone(),
            date,
            start_time,
            end_time,
            status: scheduling::AppointmentStatus::Confirmed,
            source: scheduling::AppointmentSource::Admin,
            modality: None,
            rate_tag: None,
            location: sched_config.location.clone(),
            reschedule_for: None,
            sms_confirmation: None,
            notes: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        let appts_dir = std::path::PathBuf::from(&schedules_dir).join(prac).join("appointments");
        std::fs::create_dir_all(&appts_dir)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let path = appts_dir.join(format!("{}.yaml", appt.id));
        let yaml = serde_yaml::to_string(&appt)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        std::fs::write(&path, &yaml)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        Ok(Json(serde_json::json!({
            "ok": true,
            "type": "one-off",
            "appointment_id": appt.id.to_string(),
            "client_name": client_name,
        })))
    }
}

#[derive(Deserialize)]
pub struct ScheduleCancelRequest {
    pub client_id: String,
    /// Date to cancel (YYYY-MM-DD). If series, adds EXDATE; if one-off, removes file.
    pub date: String,
    /// Cancel the entire series (not just one date)
    #[serde(default)]
    pub series: bool,
}

/// POST /api/schedule/cancel — cancel an appointment or series.
pub async fn schedule_cancel(
    Json(req): Json<ScheduleCancelRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let sched_config = scheduling::SchedulingConfig::default();
    let schedules_dir = shellexpand::tilde(&sched_config.schedules_dir).to_string();
    let schedules_path = std::path::PathBuf::from(&schedules_dir);

    let cancel_date = NaiveDate::parse_from_str(&req.date, "%Y-%m-%d")
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid date: {}", e)))?;

    // Search all practitioner dirs for matching series/appointments
    if !schedules_path.exists() {
        return Err((StatusCode::NOT_FOUND, "No schedules directory".to_string()));
    }

    for entry in std::fs::read_dir(&schedules_path).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
        let entry = entry.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) { continue; }
        let prac_path = entry.path();

        // Check series
        let series_dir = prac_path.join("series");
        if series_dir.exists() {
            for se in std::fs::read_dir(&series_dir).unwrap_or_else(|_| std::fs::read_dir("/dev/null").unwrap()) {
                let se = match se { Ok(s) => s, Err(_) => continue };
                let path = se.path();
                if !path.extension().is_some_and(|e| e == "yaml" || e == "yml") { continue; }
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                let mut series: scheduling::RecurringSeries = match serde_yaml::from_str(&content) {
                    Ok(s) => s, Err(_) => continue,
                };

                if series.client_id != req.client_id { continue; }

                if req.series {
                    // End the entire series
                    series.status = scheduling::SeriesStatus::Ended;
                    let yaml = serde_yaml::to_string(&series).unwrap_or_default();
                    let _ = std::fs::write(&path, &yaml);
                    return Ok(Json(serde_json::json!({"ok": true, "action": "series_ended"})));
                } else {
                    // Add EXDATE for this specific date
                    if !series.exdates.contains(&cancel_date) {
                        series.exdates.push(cancel_date);
                        let yaml = serde_yaml::to_string(&series).unwrap_or_default();
                        let _ = std::fs::write(&path, &yaml);
                        return Ok(Json(serde_json::json!({"ok": true, "action": "exdate_added", "date": req.date})));
                    }
                }
            }
        }

        // Check one-off appointments
        let appts_dir = prac_path.join("appointments");
        if appts_dir.exists() {
            for ae in std::fs::read_dir(&appts_dir).unwrap_or_else(|_| std::fs::read_dir("/dev/null").unwrap()) {
                let ae = match ae { Ok(a) => a, Err(_) => continue };
                let path = ae.path();
                if !path.extension().is_some_and(|e| e == "yaml" || e == "yml") { continue; }
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                let mut appt: scheduling::Appointment = match serde_yaml::from_str(&content) {
                    Ok(a) => a, Err(_) => continue,
                };
                if appt.client_id == req.client_id && appt.date == cancel_date {
                    appt.status = scheduling::AppointmentStatus::Cancelled;
                    let yaml = serde_yaml::to_string(&appt).unwrap_or_default();
                    let _ = std::fs::write(&path, &yaml);
                    return Ok(Json(serde_json::json!({"ok": true, "action": "appointment_cancelled"})));
                }
            }
        }
    }

    Err((StatusCode::NOT_FOUND, format!("No appointment found for {} on {}", req.client_id, req.date)))
}

#[derive(Deserialize)]
pub struct ScheduleMoveRequest {
    pub client_id: String,
    pub from_date: String,
    pub to_date: String,
    pub to_time: String,
}

/// POST /api/schedule/move — reschedule an appointment.
pub async fn schedule_move(
    Json(req): Json<ScheduleMoveRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Cancel the old date (adds EXDATE for series, cancels one-off)
    let cancel_req = ScheduleCancelRequest {
        client_id: req.client_id.clone(),
        date: req.from_date.clone(),
        series: false,
    };
    schedule_cancel(Json(cancel_req)).await?;

    // Create a new one-off at the new date/time
    let create_req = ScheduleCreateRequest {
        client_id: req.client_id,
        date: req.to_date,
        time: req.to_time,
        duration: default_duration(),
        recur: None,
        count: None,
        infinite: false,
        practitioner: None,
    };
    schedule_create(Json(create_req)).await
}

/// GET /api/schedule/blocks — block expiry warnings for all clients.
pub async fn schedule_blocks() -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let sched_config = scheduling::SchedulingConfig::default();
    let clinical_root = crate::config::clinical_root();
    let clients_dir = clinical_root.join("clients");

    let mut blocks = Vec::new();
    if clients_dir.exists() {
        for entry in std::fs::read_dir(&clients_dir).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))? {
            let entry = entry.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            let blocks_path = entry.path().join("blocks.yaml");
            if !blocks_path.exists() { continue; }

            let yaml = std::fs::read_to_string(&blocks_path).unwrap_or_default();
            let client_blocks: Vec<scheduling::AuthorisationBlock> = serde_yaml::from_str(&yaml).unwrap_or_default();

            for block in &client_blocks {
                let warning = scheduling::recurrence::check_block_expiry(block, sched_config.blocks.warning_threshold);
                blocks.push(serde_json::json!({
                    "client_id": block.client_id,
                    "insurer": block.insurer,
                    "authorised": block.authorised_sessions,
                    "used": block.used_sessions,
                    "remaining": block.remaining(),
                    "status": block.status.to_string(),
                    "warning": warning.as_ref().map(|w| &w.message),
                }));
            }
        }
    }

    Ok(Json(serde_json::json!({"blocks": blocks})))
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

// ---------------------------------------------------------------------------
// Reschedule handlers
// ---------------------------------------------------------------------------

/// GET /api/reschedule/slots?client_id=XX&date=YYYY-MM-DD — find available reschedule slots.
pub async fn reschedule_slots(
    Query(params): Query<RescheduleQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::scheduling::{availability, ics};

    let client_id = params.client_id.as_deref().ok_or_else(|| {
        (StatusCode::BAD_REQUEST, "client_id is required".to_string())
    })?;
    let date_str = params.date.as_deref().ok_or_else(|| {
        (StatusCode::BAD_REQUEST, "date is required".to_string())
    })?;

    let cancelled_date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid date: {}", e)))?;

    let sched_config = crate::scheduling::SchedulingConfig::default();
    let schedules_dir = shellexpand::tilde(&sched_config.schedules_dir).to_string();
    let prac_dir = std::path::PathBuf::from(&schedules_dir)
        .join(&sched_config.default_practitioner);

    let avail = availability::load_availability(&prac_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("No availability config: {}", e)))?;

    let series = ics::load_series_dir(&prac_dir.join("series")).unwrap_or_default();
    let one_offs = ics::load_appointments_dir(&prac_dir.join("appointments")).unwrap_or_default();

    let holidays_path = prac_dir.join("holidays.yaml");
    let holidays = if holidays_path.exists() {
        std::fs::read_to_string(&holidays_path)
            .ok()
            .and_then(|yaml| ics::load_holidays(&yaml).ok())
            .unwrap_or_default()
    } else {
        vec![]
    };

    // Get session duration from identity.yaml
    let clinical_root = shellexpand::tilde("~/Clinical").to_string();
    let id_path = std::path::PathBuf::from(&clinical_root)
        .join("clients")
        .join(client_id)
        .join("identity.yaml");

    let session_dur = if id_path.exists() {
        std::fs::read_to_string(&id_path)
            .ok()
            .and_then(|content| serde_yaml::from_str::<serde_yaml::Value>(&content).ok())
            .and_then(|id| {
                id.get("funding")
                    .and_then(|f| f.get("session_duration"))
                    .and_then(|v| v.as_u64())
            })
            .unwrap_or(45) as u32
    } else {
        params.duration.unwrap_or(45)
    };

    // Find original appointment time
    let original_time = one_offs
        .iter()
        .find(|a| a.client_id == client_id && a.date == cancelled_date)
        .map(|a| a.start_time)
        .or_else(|| {
            series
                .iter()
                .find(|s| s.client_id == client_id)
                .map(|s| s.start_time)
        })
        .unwrap_or(chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap());

    let from = cancelled_date.and_time(original_time);

    let mut slots = availability::find_reschedule_slots(
        &avail, &series, &one_offs, from, session_dur, &holidays,
    );

    let mut fallback = false;
    if slots.is_empty() && session_dur > 45 {
        slots = availability::find_reschedule_slots(
            &avail, &series, &one_offs, from, 45, &holidays,
        );
        fallback = true;
    }

    let slot_list: Vec<serde_json::Value> = slots
        .iter()
        .take(8)
        .enumerate()
        .map(|(i, s)| {
            let dur = (s.end_time - s.start_time).num_minutes();
            serde_json::json!({
                "index": i + 1,
                "date": s.date.format("%Y-%m-%d").to_string(),
                "day": s.day_name,
                "start": s.start_time.format("%H:%M").to_string(),
                "end": s.end_time.format("%H:%M").to_string(),
                "duration": dur,
                "modality": s.modality.to_string(),
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "client_id": client_id,
        "cancelled_date": date_str,
        "session_duration": session_dur,
        "fallback_to_45": fallback,
        "slots": slot_list,
    })))
}

/// POST /api/reschedule/book — book a reschedule slot (no-charge).
pub async fn reschedule_book(
    Json(req): Json<RescheduleBookRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use crate::scheduling::{ics, models::*};

    let sched_config = crate::scheduling::SchedulingConfig::default();
    let schedules_dir = shellexpand::tilde(&sched_config.schedules_dir).to_string();
    let prac_dir = std::path::PathBuf::from(&schedules_dir)
        .join(&sched_config.default_practitioner);
    let appts_dir = prac_dir.join("appointments");
    std::fs::create_dir_all(&appts_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let date = NaiveDate::parse_from_str(&req.date, "%Y-%m-%d")
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid date: {}", e)))?;
    let start = chrono::NaiveTime::parse_from_str(&req.start, "%H:%M")
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid start time: {}", e)))?;
    let end = chrono::NaiveTime::parse_from_str(&req.end, "%H:%M")
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid end time: {}", e)))?;

    // Determine modality from day
    let modality = match date.weekday() {
        chrono::Weekday::Mon | chrono::Weekday::Fri => Some(SessionModality::Remote),
        _ => Some(SessionModality::InPerson),
    };

    // Look up client name
    let clinical_root = shellexpand::tilde("~/Clinical").to_string();
    let id_path = std::path::PathBuf::from(&clinical_root)
        .join("clients")
        .join(&req.client_id)
        .join("identity.yaml");
    let client_name = if id_path.exists() {
        std::fs::read_to_string(&id_path)
            .ok()
            .and_then(|c| serde_yaml::from_str::<serde_yaml::Value>(&c).ok())
            .and_then(|id| id.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .unwrap_or_else(|| req.client_id.clone())
    } else {
        req.client_id.clone()
    };

    let appt = Appointment {
        id: uuid::Uuid::new_v4(),
        series_id: None,
        practitioner: sched_config.default_practitioner.clone(),
        client_id: req.client_id.clone(),
        client_name: client_name.clone(),
        date,
        start_time: start,
        end_time: end,
        status: AppointmentStatus::Confirmed,
        source: AppointmentSource::Reschedule,
        modality,
        rate_tag: None,
        location: sched_config.location.clone(),
        reschedule_for: Some(req.cancelled_date.clone()),
        sms_confirmation: None,
        notes: Some(format!("No-charge reschedule for cancelled session {}", req.cancelled_date)),
        created_at: chrono::Local::now().to_rfc3339(),
    };

    // Save appointment YAML
    let appt_path = appts_dir.join(format!("{}.yaml", appt.id));
    let yaml = serde_yaml::to_string(&appt)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    std::fs::write(&appt_path, &yaml)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "ok": true,
        "appointment_id": appt.id.to_string(),
        "client_id": req.client_id,
        "client_name": client_name,
        "date": req.date,
        "start": req.start,
        "end": req.end,
        "reschedule_for": req.cancelled_date,
        "no_charge": true,
    })))
}

#[derive(Deserialize)]
pub struct RescheduleQuery {
    pub client_id: Option<String>,
    pub date: Option<String>,
    pub duration: Option<u32>,
}

#[derive(Deserialize)]
pub struct RescheduleBookRequest {
    pub client_id: String,
    pub date: String,
    pub start: String,
    pub end: String,
    pub cancelled_date: String,
}

// ---------------------------------------------------------------------------
// Email setup handlers
// ---------------------------------------------------------------------------

/// GET /api/email/status — check if email is configured.
pub async fn email_status() -> Json<serde_json::Value> {
    match crate::email::load_email_config() {
        Ok(config) => {
            // Check for additional identities in config
            let identities = load_email_identities();
            Json(serde_json::json!({
                "configured": true,
                "from_email": config.from_email,
                "from_name": config.from_name,
                "smtp_server": config.smtp_server,
                "identities": identities,
            }))
        },
        Err(_) => Json(serde_json::json!({"configured": false})),
    }
}

fn load_email_identities() -> Vec<serde_json::Value> {
    let config_path = dirs::config_dir()
        .map(|d| d.join("practiceforge/config.toml"))
        .unwrap_or_default();
    let content = std::fs::read_to_string(&config_path).unwrap_or_default();
    let table: toml::Table = content.parse().unwrap_or_default();

    let email = match table.get("email").and_then(|v| v.as_table()) {
        Some(e) => e,
        None => return vec![],
    };

    let mut identities = vec![];
    if let Some(arr) = email.get("identities").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(t) = item.as_table() {
                identities.push(serde_json::json!({
                    "label": t.get("label").and_then(|v| v.as_str()).unwrap_or(""),
                    "from_email": t.get("from_email").and_then(|v| v.as_str()).unwrap_or(""),
                    "from_name": t.get("from_name").and_then(|v| v.as_str()).unwrap_or(""),
                }));
            }
        }
    }
    identities
}

#[derive(Deserialize)]
pub struct EmailSetupRequest {
    pub from_email: String,
    pub from_name: String,
    pub smtp_server: String,
    pub smtp_port: u16,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub signature: String,
    /// Secondary "send as" email (optional — for clients not via main practice)
    #[serde(default)]
    pub alt_email: Option<String>,
    #[serde(default)]
    pub alt_label: Option<String>,
}

/// POST /api/email/setup — save email configuration.
pub async fn email_setup(
    Json(req): Json<EmailSetupRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let config_dir = dirs::config_dir()
        .map(|d| d.join("practiceforge"))
        .ok_or_else(|| (StatusCode::INTERNAL_SERVER_ERROR, "No config dir".to_string()))?;
    std::fs::create_dir_all(&config_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let config_path = config_dir.join("config.toml");
    let mut config_str = if config_path.exists() {
        std::fs::read_to_string(&config_path).unwrap_or_default()
    } else {
        String::new()
    };

    // Remove existing [email] section if present
    if let Some(start) = config_str.find("\n[email]") {
        let rest = &config_str[start + 1..];
        let end = rest.find("\n[").map(|p| start + 1 + p).unwrap_or(config_str.len());
        config_str.replace_range(start..end, "");
    } else if config_str.starts_with("[email]") {
        let end = config_str.find("\n[").unwrap_or(config_str.len());
        config_str.replace_range(0..end, "");
    }

    // Append new [email] section
    config_str.push_str(&format!(
        "\n[email]\nfrom_email = \"{}\"\nfrom_name = \"{}\"\nsmtp_server = \"{}\"\nsmtp_port = {}\nusername = \"{}\"\nsignature = \"{}\"\n",
        req.from_email, req.from_name, req.smtp_server, req.smtp_port, req.username,
        req.signature.replace('"', "\\\"")
    ));

    // Add secondary identity if provided
    if let Some(ref alt_email) = req.alt_email {
        let alt_label = req.alt_label.as_deref().unwrap_or("secondary");
        config_str.push_str(&format!(
            "\n[[email.identities]]\nlabel = \"{}\"\nfrom_email = \"{}\"\nfrom_name = \"{}\"\n",
            alt_label, alt_email, req.from_name
        ));
    }

    std::fs::write(&config_path, &config_str)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Store password in keychain
    let _ = std::process::Command::new("security")
        .args(["add-generic-password", "-a", "clinical-email", "-s", "clinical-email",
               "-w", &req.password, "-U"])
        .output();

    Ok(Json(serde_json::json!({"ok": true})))
}

/// POST /api/email/test — send a test email to self.
pub async fn email_test() -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let config = crate::email::load_email_config()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Email not configured: {}", e)))?;

    crate::email::send_test(&config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Test email failed: {}", e)))?;

    Ok(Json(serde_json::json!({"ok": true, "sent_to": config.from_email})))
}

// ---------------------------------------------------------------------------
// Letter workflow handlers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LetterDraftRequest {
    pub client_id: String,
}

/// POST /api/letter/draft — generate a letter draft for a client.
pub async fn letter_draft(
    Json(req): Json<LetterDraftRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Run clinical update-letter to get the draft
    let output = tokio::process::Command::new("clinical")
        .arg("update-letter")
        .arg(&req.client_id)
        .arg("--dry-run")
        .output()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to run clinical update-letter: {}", e)))?;

    let draft = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Get referrer info from identity
    let clinical_root = crate::config::clinical_root();
    let client_dir = clinical_root.join("clients").join(&req.client_id);
    let identity_path = if client_dir.join("identity.yaml").exists() {
        client_dir.join("identity.yaml")
    } else {
        client_dir.join("private").join("identity.yaml")
    };

    let (referrer_name, referrer_email) = if identity_path.exists() {
        let content = std::fs::read_to_string(&identity_path).unwrap_or_default();
        let val: serde_yaml::Value = serde_yaml::from_str(&content).unwrap_or_default();
        let rn = val.get("referrer").and_then(|r| r.get("name")).and_then(|v| v.as_str()).map(|s| s.to_string());
        let re = val.get("referrer").and_then(|r| r.get("email")).and_then(|v| v.as_str()).map(|s| s.to_string());
        (rn, re)
    } else {
        (None, None)
    };

    Ok(Json(serde_json::json!({
        "draft": draft,
        "referrer_name": referrer_name,
        "referrer_email": referrer_email,
    })))
}

#[derive(Deserialize)]
pub struct LetterBuildRequest {
    pub client_id: String,
    pub content: String,
}

/// POST /api/letter/build — build a PDF from letter content.
pub async fn letter_build(
    Json(req): Json<LetterBuildRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use tokio::io::AsyncWriteExt;

    let mut child = tokio::process::Command::new("clinical-letter-build")
        .arg(&req.client_id)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to run clinical-letter-build: {}", e)))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(req.content.as_bytes()).await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    let output = child.wait_with_output().await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("Build failed: {}", stderr)));
    }

    let pdf_path = String::from_utf8_lossy(&output.stdout).trim().to_string();

    Ok(Json(serde_json::json!({"ok": true, "pdf_path": pdf_path})))
}

#[derive(Deserialize)]
pub struct LetterSendRequest {
    pub client_id: String,
    pub pdf_path: String,
    /// Override "from" email address (identity picker)
    #[serde(default)]
    pub from_email: Option<String>,
}

/// POST /api/letter/send — send a letter via email and upload to TM3.
pub async fn letter_send(
    Json(req): Json<LetterSendRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let mut cmd = tokio::process::Command::new("clinical-letter-send");
    cmd.arg(&req.client_id)
        .arg("--pdf")
        .arg(&req.pdf_path);

    if let Some(ref from) = req.from_email {
        cmd.arg("--from").arg(from);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to run clinical-letter-send: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("Send failed: {}", stderr)));
    }

    Ok(Json(serde_json::json!({"ok": true})))
}

// ─── AI model selection ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SetModelRequest {
    pub model: String,
}

/// POST /api/ai/model — update the active model in config.toml.
///
/// Only changes the `model =` line; leaves backend and all other config intact.
/// Returns the updated model name so the UI can confirm the change.
pub async fn set_ai_model(
    Json(req): Json<SetModelRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let allowed = ["claude-haiku-4-5-20251001", "claude-sonnet-4-6", "claude-opus-4-7"];
    if !allowed.contains(&req.model.as_str()) {
        return Err((StatusCode::BAD_REQUEST, format!("Unknown model: {}", req.model)));
    }

    let config_path = crate::config::config_dir().join("config.toml");
    let existing = std::fs::read_to_string(&config_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Replace or insert model = "..." within the [ai] section.
    let updated = if let Some(ai_pos) = existing.find("[ai]") {
        let after = ai_pos + 4;
        let next = existing[after..].find("\n[").map(|p| after + p).unwrap_or(existing.len());
        let ai_section = &existing[ai_pos..next];
        let new_model_line = format!("model = \"{}\"", req.model);
        let new_section = if ai_section.contains("model = ") {
            // Replace existing model line
            ai_section.lines()
                .map(|l| if l.trim_start().starts_with("model = ") { new_model_line.as_str() } else { l })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            // Append model line to [ai] section
            format!("{}\n{}", ai_section.trim_end(), new_model_line)
        };
        format!("{}{}{}", &existing[..ai_pos], new_section, &existing[next..])
    } else {
        return Err((StatusCode::CONFLICT, "No [ai] section in config — run `practiceforge ai setup` first".to_string()));
    };

    std::fs::write(&config_path, updated)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({ "ok": true, "model": req.model })))
}

#[derive(Deserialize)]
pub struct AiSetupRequest {
    pub backend: String,
    pub model: String,
    #[serde(default)]
    pub api_key: Option<String>,
}

/// POST /api/ai/setup — write [ai] section to config.toml and optionally save API key.
pub async fn ai_setup(
    Json(req): Json<AiSetupRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let allowed_backends = ["anthropic", "claude-cli", "ollama"];
    if !allowed_backends.contains(&req.backend.as_str()) {
        return Err((StatusCode::BAD_REQUEST, format!("Unknown backend: {}", req.backend)));
    }

    let config_path = crate::config::config_dir().join("config.toml");
    std::fs::create_dir_all(crate::config::config_dir())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let existing = std::fs::read_to_string(&config_path).unwrap_or_default();

    let new_ai = format!("[ai]\nbackend = \"{}\"\nmodel = \"{}\"\n", req.backend, req.model);
    let updated = if let Some(ai_pos) = existing.find("[ai]") {
        let after = ai_pos + 4;
        let next = existing[after..].find("\n[").map(|p| after + p).unwrap_or(existing.len());
        format!("{}{}{}", &existing[..ai_pos], new_ai, &existing[next..])
    } else {
        format!("{}\n{}", existing.trim_end(), new_ai)
    };

    std::fs::write(&config_path, updated)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Save API key for Anthropic backend
    if req.backend == "anthropic" {
        if let Some(key) = &req.api_key {
            if !key.is_empty() {
                let mut secrets = crate::billing::secrets::BillingSecrets::load()
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                secrets.ai.api_key = Some(key.clone());
                secrets.save()
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            }
        }
    }

    Ok(Json(serde_json::json!({ "ok": true, "backend": req.backend, "model": req.model })))
}

