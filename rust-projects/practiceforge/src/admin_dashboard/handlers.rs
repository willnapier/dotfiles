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

#[derive(Deserialize)]
pub struct OnboardManualRequest {
    pub name: String,
    /// ISO-format DOB (YYYY-MM-DD).
    pub dob: String,
    /// Optional TM3 numeric ID. If present, docs are downloaded.
    #[serde(default)]
    pub tm3_id: Option<String>,
    /// Session-file date whose `???` rows should be remapped to the new
    /// client ID. Defaults to today.
    #[serde(default)]
    pub date: Option<String>,
}

/// POST /api/clients/onboard-manual — onboard a client using practitioner-
/// supplied name + DOB, bypassing the TM3 cache lookup. Intended for clinic
/// rows that didn't resolve via the automatic cache refresh.
///
/// After a successful onboard, any rows in the target date's session file
/// where `id == "???"` and `client_name == name` have their `id` rewritten
/// to the newly derived client ID, so the clinic view shows the correct
/// mapping without requiring a full diary re-capture.
pub async fn onboard_manual(
    Json(req): Json<OnboardManualRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if req.name.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "name is required".to_string()));
    }
    if req.dob.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "dob is required".to_string()));
    }

    let name = req.name.trim().to_string();
    let dob = req.dob.trim().to_string();
    let tm3_id = req.tm3_id.as_ref().and_then(|s| {
        let t = s.trim();
        if t.is_empty() { None } else { Some(t.to_string()) }
    });

    let name_for_task = name.clone();
    let dob_for_task = dob.clone();
    let tm3_id_for_task = tm3_id.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        crate::onboard::onboard_manual(
            &name_for_task,
            &dob_for_task,
            tm3_id_for_task.as_deref(),
        )
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("join error: {e}")))?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;

    let date = req
        .date
        .unwrap_or_else(|| Local::now().format("%Y-%m-%d").to_string());
    let remapped = remap_session_unknowns(&date, &name, &outcome.client_id).unwrap_or(0);

    Ok(Json(serde_json::json!({
        "ok": true,
        "client_id": outcome.client_id,
        "name": outcome.name,
        "docs_imported": outcome.docs_imported,
        "skipped": outcome.skipped,
        "rows_remapped": remapped,
    })))
}

/// Rewrite `???` rows in the session file for `date` whose `client_name`
/// matches `target_name` so their `id` becomes `new_id`. Returns the row
/// count that was changed; 0 if the file doesn't exist or nothing matched.
fn remap_session_unknowns(
    date: &str,
    target_name: &str,
    new_id: &str,
) -> std::io::Result<usize> {
    let path = session_path(date);
    if !path.exists() {
        return Ok(0);
    }

    let content = std::fs::read_to_string(&path)?;
    let mut session: ClinicSession = serde_json::from_str(&content)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let mut count = 0;
    for c in session.clients.iter_mut() {
        if c.id == "???" && c.client_name == target_name {
            c.id = new_id.to_string();
            count += 1;
        }
    }
    if count > 0 {
        let json = serde_json::to_string_pretty(&session)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&path, json)?;
    }
    Ok(count)
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

    // Load email identity
    let identity = crate::email::primary_identity().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Email not configured — run `practiceforge email init`".to_string(),
        )
    })?;

    let practitioner = identity.from_name.clone();

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
    crate::email::send_as(
        &identity.from_email,
        to_email,
        &reminder.to_name,
        &reminder.subject,
        crate::email::Body::Text(reminder.body.clone()),
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
    /// Optional named preset from `~/.config/practiceforge/prompt-presets/*.md`.
    /// Content is appended to the system prompt after practitioner additions.
    #[serde(default)]
    pub prompt_preset: Option<String>,
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

/// A single variant identifier in a compare pair. Matches the `variant`
/// shape the frontend sends on `/api/log-pair` and the shape stored in
/// `~/Clinical/comparisons.jsonl` (v2 schema).
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct VariantSpec {
    pub model: String,
    #[serde(default = "default_true")]
    pub with_rail: bool,
    #[serde(default = "default_preset")]
    pub prompt_preset: String,
}

fn default_preset() -> String {
    "default".to_string()
}

/// Payload for /api/log-pair (v2 schema) — records a side-by-side comparison
/// of two variants of {model, with_rail, prompt_preset}. One row is written
/// to `~/Clinical/comparisons.jsonl` per pair.
#[derive(Deserialize)]
pub struct LogPairRequest {
    pub client_id: String,
    pub observation: String,
    pub variant_a: VariantSpec,
    pub variant_a_note: String,
    pub variant_a_secs: f64,
    pub variant_b: VariantSpec,
    pub variant_b_note: String,
    pub variant_b_secs: f64,
    /// "a", "b", or null (both rejected)
    #[serde(default)]
    pub accepted: Option<String>,
}

/// Per-variant payload inside a v2 comparisons.jsonl row.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VariantRecord {
    pub model: String,
    pub with_rail: bool,
    pub prompt_preset: String,
    pub note: String,
    pub generation_secs: f64,
    pub hard_failures: usize,
    pub soft_flags: usize,
    pub flag_details: Vec<String>,
    pub accepted: bool,
}

/// v2 row in ~/Clinical/comparisons.jsonl — one entry per pair.
#[derive(Serialize)]
struct ComparisonPairEntry {
    timestamp: String,
    schema_version: u32,
    client_id: String,
    observation: String,
    variant_a: VariantRecord,
    variant_b: VariantRecord,
    regen_reasons: Vec<String>,
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

// ---------------------------------------------------------------------------
// Background TM3 refresh coordination.
//
// On dashboard page load (clinic view mount), the frontend calls
// /api/session/refresh?date=today. The handler:
//   - Reports the age of the session file
//   - If stale (> STALE_THRESHOLD_SECS), spawns `tm3-diary-capture --live`
//     as a background task, returns immediately with status "refreshing"
//   - Deduplicates concurrent refresh attempts via a global Mutex
//
// The frontend polls the same endpoint every few seconds until status
// flips to "fresh". It then refetches /api/session to show new data.
//
// If the capture fails with session expiry, status becomes "expired" and
// the frontend surfaces a banner offering to re-authenticate, which hits
// /api/session/tm3-relogin (subprocess `tm3-upload login` — requires
// user to touch the Touch ID sensor or enter password).
// ---------------------------------------------------------------------------

/// Session data older than this is considered stale and a refresh gets
/// kicked off. 5 minutes hits a good compromise: frequent enough to keep
/// the Clinic view current during active use, infrequent enough not to
/// wear out TM3 or fire Touch ID re-auth prompts unnecessarily.
const STALE_THRESHOLD_SECS: u64 = 300;

/// State of the most-recent background refresh attempt. Keyed by date.
#[derive(Clone, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
enum RefreshState {
    /// No refresh in flight. Session file is current (or never stale).
    Idle,
    /// Background capture is running right now.
    Running,
    /// Last capture completed at this instant; data on disk is fresh.
    Done { at_epoch: u64 },
    /// Last capture failed because TM3 session cookies expired. UI should
    /// surface a re-auth prompt.
    Expired,
    /// No TM3 cookies exist yet on this machine — first-time setup. UI
    /// should surface an informational "Connect TM3" prompt (blue, not red).
    NeverConnected,
    /// Last capture failed for some other reason; message is stderr tail.
    Failed { message: String },
}

fn refresh_state_table() -> &'static std::sync::Mutex<std::collections::HashMap<String, RefreshState>> {
    static TABLE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, RefreshState>>> =
        std::sync::OnceLock::new();
    TABLE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[derive(Deserialize)]
pub struct SessionRefreshQuery {
    pub date: Option<String>,
    /// If true, trigger a refresh even if the file isn't stale. Frontend
    /// sends this on explicit "Refresh" button clicks.
    #[serde(default)]
    pub force: bool,
}

/// GET /api/session/refresh?date=YYYY-MM-DD[&force=true]
///
/// Reports freshness of the session file and kicks off a background
/// `tm3-diary-capture --live` if the file is stale (or force=true).
/// Deduplicates: if a capture is already in flight for this date, just
/// returns "running" without spawning another.
pub async fn session_refresh(
    Query(params): Query<SessionRefreshQuery>,
) -> Json<serde_json::Value> {
    let date = params
        .date
        .unwrap_or_else(|| Local::now().format("%Y-%m-%d").to_string());
    let path = session_path(&date);

    let (age_secs, mtime_epoch) = match std::fs::metadata(&path) {
        Ok(meta) => match meta.modified() {
            Ok(mtime) => {
                let epoch = mtime
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                (Some(now.saturating_sub(epoch)), Some(epoch))
            }
            Err(_) => (None, None),
        },
        Err(_) => (None, None),
    };

    let is_stale = age_secs.map(|a| a > STALE_THRESHOLD_SECS).unwrap_or(true);
    let current_state = refresh_state_table()
        .lock()
        .unwrap()
        .get(&date)
        .cloned()
        .unwrap_or(RefreshState::Idle);

    // Never stack refreshes: if one's already running, just report it.
    let already_running = matches!(current_state, RefreshState::Running);

    let should_refresh = (params.force || is_stale) && !already_running;

    if should_refresh {
        refresh_state_table()
            .lock()
            .unwrap()
            .insert(date.clone(), RefreshState::Running);
        let date_for_task = date.clone();
        tokio::task::spawn_blocking(move || {
            run_tm3_capture(&date_for_task);
        });
    }

    let state_after = refresh_state_table()
        .lock()
        .unwrap()
        .get(&date)
        .cloned()
        .unwrap_or(RefreshState::Idle);

    Json(serde_json::json!({
        "date": date,
        "mtime_epoch": mtime_epoch,
        "age_seconds": age_secs,
        "stale": is_stale,
        "refresh": state_after,
        "threshold_seconds": STALE_THRESHOLD_SECS,
    }))
}

/// Run `tm3-diary-capture --live` and record the outcome in the refresh
/// state table. Invoked from a tokio blocking task; must not panic even
/// on weird subprocess output.
fn run_tm3_capture(date: &str) {
    use std::process::Command;

    let tm3 = dirs::home_dir()
        .unwrap_or_default()
        .join(".local/bin/tm3-diary-capture");

    let output = Command::new(&tm3)
        .arg("--live")
        .output();

    let new_state = match output {
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stderr.contains("Session expired") {
                RefreshState::Expired
            } else if stderr.contains("No TM3 session") {
                RefreshState::NeverConnected
            } else if !out.status.success() {
                // Keep the last ~400 chars of stderr for the frontend.
                let msg: String = stderr.chars().rev().take(400).collect::<String>().chars().rev().collect();
                RefreshState::Failed { message: msg }
            } else {
                let at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                RefreshState::Done { at_epoch: at }
            }
        }
        Err(e) => RefreshState::Failed {
            message: format!("could not spawn tm3-diary-capture: {e}"),
        },
    };

    refresh_state_table()
        .lock()
        .unwrap()
        .insert(date.to_string(), new_state);
}

/// POST /api/session/tm3-relogin
///
/// Runs `tm3-upload login` as a subprocess. Blocks until done (typically
/// 5-15s while the user authenticates via Touch ID or password). On
/// success, clears the expired state so the next refresh proceeds
/// normally. The frontend should call /api/session/refresh?force=true
/// after this returns to immediately repopulate today's data.
pub async fn session_tm3_relogin() -> Json<serde_json::Value> {
    use std::process::Command;

    let tm3_upload = dirs::home_dir()
        .unwrap_or_default()
        .join(".local/bin/tm3-upload");

    let outcome = tokio::task::spawn_blocking(move || {
        Command::new(&tm3_upload).arg("login").output()
    })
    .await;

    match outcome {
        Ok(Ok(out)) if out.status.success() => {
            // Clear any lingering "Expired" state for any date — a fresh
            // login makes all of them retry-able.
            refresh_state_table().lock().unwrap().clear();
            Json(serde_json::json!({
                "ok": true,
                "stdout": String::from_utf8_lossy(&out.stdout).to_string(),
            }))
        }
        Ok(Ok(out)) => Json(serde_json::json!({
            "ok": false,
            "error": format!(
                "tm3-upload login exited {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            ),
        })),
        Ok(Err(e)) => Json(serde_json::json!({
            "ok": false,
            "error": format!("could not spawn tm3-upload: {e}"),
        })),
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "error": format!("join error: {e}"),
        })),
    }
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
    let model_override = req.model.clone();
    let preset_name = req.prompt_preset.clone();

    let (prompt, pmap) = build_fallback_prompt(&client_id, &observation, with_rail);
    let mut system_prompt = load_system_prompt();
    if let Some(addendum) = pmap.system_addendum() {
        system_prompt.push_str(&addendum);
    }
    // Append the preset body (if any) after practitioner_additions slot.
    // Missing / unknown presets log to stderr inside `load_preset` and return
    // empty — we just no-op append in that case.
    if let Some(name) = preset_name.as_deref() {
        let body = crate::prompt_presets::load_preset(name);
        if !body.trim().is_empty() {
            system_prompt.push_str("\n\n");
            system_prompt.push_str(body.trim());
        }
    }

    // Frontier backend (Anthropic API or Claude CLI subscription).
    if let Some(backend) = crate::llm::load_backend() {
        eprintln!(
            "[gen] {} client={client_id} model={} preset={} rail={}",
            backend.backend_name(),
            model_override.as_deref().unwrap_or("<default>"),
            preset_name.as_deref().unwrap_or("<none>"),
            with_rail,
        );
        let token_stream = backend
            .generate_stream(system_prompt, prompt, model_override)
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

/// POST /api/log-pair — append a multi-variant comparison pair to
/// ~/Clinical/comparisons.jsonl (v2 schema, one row per pair).
///
/// Called by the dashboard's side-by-side compare view once the practitioner
/// has accepted one variant or rejected both. Faithfulness stats
/// (hard_failures / soft_flags / flag_details) are populated by running the
/// same lightweight in-process check the legacy handler used — at this layer
/// those are stubs; deeper analysis is still run offline against the stored
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

    // "a" | "b" | null — anything else also means "both rejected" defensively.
    let accepted_code = req.accepted.as_deref();
    let a_accepted = accepted_code == Some("a");
    let b_accepted = accepted_code == Some("b");

    let (a_hard, a_soft, a_details) = faithfulness_stub(&req.variant_a_note);
    let (b_hard, b_soft, b_details) = faithfulness_stub(&req.variant_b_note);

    let home = dirs::home_dir()
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "No home dir".to_string()))?;
    let path = home.join("Clinical/comparisons.jsonl");

    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();

    let entry = ComparisonPairEntry {
        timestamp: now,
        schema_version: 2,
        client_id: req.client_id,
        observation: req.observation,
        variant_a: VariantRecord {
            model: req.variant_a.model,
            with_rail: req.variant_a.with_rail,
            prompt_preset: req.variant_a.prompt_preset,
            note: req.variant_a_note,
            generation_secs: req.variant_a_secs,
            hard_failures: a_hard,
            soft_flags: a_soft,
            flag_details: a_details,
            accepted: a_accepted,
        },
        variant_b: VariantRecord {
            model: req.variant_b.model,
            with_rail: req.variant_b.with_rail,
            prompt_preset: req.variant_b.prompt_preset,
            note: req.variant_b_note,
            generation_secs: req.variant_b_secs,
            hard_failures: b_hard,
            soft_flags: b_soft,
            flag_details: b_details,
            accepted: b_accepted,
        },
        regen_reasons: Vec::new(),
    };

    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("open jsonl: {e}")))?;

    let line = serde_json::to_string(&entry)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("serialize: {e}")))?;
    file.write_all(line.as_bytes())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("write jsonl: {e}")))?;
    file.write_all(b"\n")
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("write jsonl: {e}")))?;

    Ok(Json(serde_json::json!({"ok": true})))
}

/// In-process faithfulness stub — matches the legacy log_pair handler, which
/// writes zeroed fields and delegates real grounding analysis to offline
/// tools (`clinical check_faithfulness`) against the persisted note. We keep
/// the call-site shape (hard / soft / details tuple) so a richer check can
/// drop in here without touching `log_pair`.
fn faithfulness_stub(_note: &str) -> (usize, usize, Vec<String>) {
    (0, 0, Vec::new())
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
// Prompt presets handler
// ---------------------------------------------------------------------------

/// GET /api/prompt-presets — enumerate the named prompt presets available in
/// `~/.config/practiceforge/prompt-presets/`. Seeds a `default.md` on first
/// run when the directory is missing. Always returns at least one entry.
pub async fn prompt_presets() -> Json<Vec<crate::prompt_presets::PresetMeta>> {
    Json(crate::prompt_presets::list_presets())
}

// ---------------------------------------------------------------------------
// Compare analytics handler
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct VariantStats {
    pub model: String,
    pub with_rail: bool,
    pub prompt_preset: String,
    pub runs: u64,
    pub accepts: u64,
    pub accept_rate: f64,
    pub avg_secs: f64,
    pub avg_hard_failures: f64,
    pub avg_soft_flags: f64,
}

#[derive(Serialize)]
pub struct CompareAnalytics {
    pub variants: Vec<VariantStats>,
    pub total_pairs: u64,
    pub reject_both_pairs: u64,
}

/// Accumulator for one `(model, with_rail, prompt_preset)` group.
#[derive(Default)]
struct Accum {
    runs: u64,
    accepts: u64,
    sum_secs: f64,
    sum_hard: f64,
    sum_soft: f64,
}

/// Map a legacy model name to a stable, visible analytics label.
fn map_legacy_model(model: &str) -> String {
    match model {
        "clinical-voice-q4" => "legacy-q4".to_string(),
        "clinical-voice-q8" => "legacy-q8".to_string(),
        _ => model.to_string(),
    }
}

/// GET /api/compare/analytics — aggregate `~/Clinical/comparisons.jsonl` into
/// per-variant stats. Tolerates both v2 pair rows and legacy per-generation
/// rows. Unreadable or malformed rows are skipped with a stderr warning.
pub async fn compare_analytics()
    -> Result<Json<CompareAnalytics>, (StatusCode, String)>
{
    use std::collections::HashMap;

    let home = dirs::home_dir()
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "No home dir".to_string()))?;
    let path = home.join("Clinical/comparisons.jsonl");

    if !path.exists() {
        return Ok(Json(CompareAnalytics {
            variants: Vec::new(),
            total_pairs: 0,
            reject_both_pairs: 0,
        }));
    }

    let raw = std::fs::read_to_string(&path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("read jsonl: {e}")))?;

    // Keyed on (model, with_rail, prompt_preset) for grouping.
    let mut groups: HashMap<(String, bool, String), Accum> = HashMap::new();
    let mut total_pairs: u64 = 0;
    let mut reject_both_pairs: u64 = 0;

    for (line_idx, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let val: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!(
                    "[compare_analytics] skipping malformed row {}: {}",
                    line_idx + 1,
                    e
                );
                continue;
            }
        };

        // v2 rows have schema_version == 2 and variant_a/variant_b objects.
        let is_v2 = val.get("schema_version").and_then(|v| v.as_u64()) == Some(2)
            && val.get("variant_a").is_some()
            && val.get("variant_b").is_some();

        if is_v2 {
            total_pairs += 1;
            // v2 pair rows store "which variant was accepted" via the
            // per-variant `accepted: bool` field — that's the authoritative
            // source (log_pair writes it from the request's `accepted` code).
            let a_accepted = val["variant_a"].get("accepted")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let b_accepted = val["variant_b"].get("accepted")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !a_accepted && !b_accepted {
                reject_both_pairs += 1;
            }

            for (obj, accepted) in [
                (&val["variant_a"], a_accepted),
                (&val["variant_b"], b_accepted),
            ] {
                let Some(key) = extract_variant_key(obj) else {
                    eprintln!(
                        "[compare_analytics] v2 row {} missing variant key fields; skipping variant",
                        line_idx + 1
                    );
                    continue;
                };
                let a = groups.entry(key).or_default();
                a.runs += 1;
                if accepted {
                    a.accepts += 1;
                }
                a.sum_secs += obj.get("generation_secs")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                a.sum_hard += obj.get("hard_failures")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as f64)
                    .unwrap_or(0.0);
                a.sum_soft += obj.get("soft_flags")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as f64)
                    .unwrap_or(0.0);
            }
        } else {
            // Legacy row: one generation per row.
            // {timestamp, client_id, model, observation, note, hard_failures,
            //  soft_flags, flag_details, attempts, regen_reasons,
            //  generation_secs, accepted, prompt_rail}
            let Some(model_raw) = val.get("model").and_then(|v| v.as_str()) else {
                eprintln!(
                    "[compare_analytics] legacy row {} missing model; skipping",
                    line_idx + 1
                );
                continue;
            };
            let model = map_legacy_model(model_raw);
            // Per contract: legacy rows are grouped as {with_rail=true,
            // prompt_preset="legacy"} regardless of their actual prompt_rail
            // field — the legacy schema predates the multi-variant concept so
            // treating them as a single grouping keeps the analytics stable.
            let with_rail = true;
            let prompt_preset = "legacy".to_string();

            let accepted = val.get("accepted")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let key = (model, with_rail, prompt_preset);
            let a = groups.entry(key).or_default();
            a.runs += 1;
            if accepted {
                a.accepts += 1;
            }
            a.sum_secs += val.get("generation_secs")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            a.sum_hard += val.get("hard_failures")
                .and_then(|v| v.as_u64())
                .map(|n| n as f64)
                .unwrap_or(0.0);
            a.sum_soft += val.get("soft_flags")
                .and_then(|v| v.as_u64())
                .map(|n| n as f64)
                .unwrap_or(0.0);
        }
    }

    let mut variants: Vec<VariantStats> = groups
        .into_iter()
        .map(|((model, with_rail, prompt_preset), a)| {
            let runs_f = a.runs.max(1) as f64;
            VariantStats {
                model,
                with_rail,
                prompt_preset,
                runs: a.runs,
                accepts: a.accepts,
                accept_rate: if a.runs == 0 { 0.0 } else { a.accepts as f64 / a.runs as f64 },
                avg_secs: a.sum_secs / runs_f,
                avg_hard_failures: a.sum_hard / runs_f,
                avg_soft_flags: a.sum_soft / runs_f,
            }
        })
        .collect();

    // Stable UI order: model ASC, then rail on-before-off, then preset ASC.
    variants.sort_by(|a, b| {
        a.model.cmp(&b.model)
            .then_with(|| b.with_rail.cmp(&a.with_rail))
            .then_with(|| a.prompt_preset.cmp(&b.prompt_preset))
    });

    Ok(Json(CompareAnalytics {
        variants,
        total_pairs,
        reject_both_pairs,
    }))
}

/// Extract the grouping key `(model, with_rail, prompt_preset)` from a v2
/// variant object. Returns `None` if `model` is missing.
fn extract_variant_key(obj: &serde_json::Value)
    -> Option<(String, bool, String)>
{
    let model = obj.get("model")
        .and_then(|v| v.as_str())?
        .to_string();
    let with_rail = obj.get("with_rail")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let prompt_preset = obj.get("prompt_preset")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();
    Some((model, with_rail, prompt_preset))
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
    match crate::email::primary_identity() {
        Some(identity) => {
            let body = format!(
                "Your PracticeForge login code is: {}\n\nThis code is valid for 10 minutes.",
                code
            );
            match crate::email::send_as(
                &identity.from_email,
                email,
                "",
                "PracticeForge Login Code",
                crate::email::Body::Text(body),
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
        None => {
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
    use crate::email::backends::BackendConfig;

    let identities = crate::email::load_identities();
    if identities.is_empty() {
        return Json(serde_json::json!({"configured": false}));
    }
    let primary = identities.iter().find(|i| i.primary).unwrap_or(&identities[0]);

    // Flatten backend-specific fields into the JSON shape the web UI still
    // expects (smtp_server/smtp_port/username). Graph identities report
    // empty strings for those — the UI treats empty smtp_server as
    // "non-SMTP identity". This is a pragmatic shim for the admin dashboard
    // until that UI is updated to speak the new shape directly.
    let identity_rows: Vec<serde_json::Value> = identities.iter().map(|i| {
        let (smtp_server, smtp_port, username) = match &i.config.backend {
            BackendConfig::Smtp(cfg) => {
                (cfg.host.clone(), cfg.port, cfg.username.clone())
            }
            BackendConfig::Graph(_) => (String::new(), 0u16, String::new()),
        };
        serde_json::json!({
            "label": i.label,
            "from_email": i.from_email,
            "from_name": i.from_name,
            "smtp_server": smtp_server,
            "smtp_port": smtp_port,
            "username": username,
            "primary": i.primary,
        })
    }).collect();

    Json(serde_json::json!({
        "configured": true,
        "from_email": primary.from_email,
        "from_name": primary.from_name,
        "identities": identity_rows,
    }))
}

#[derive(Deserialize)]
pub struct IdentitySetupEntry {
    pub label: String,
    pub from_email: String,
    pub from_name: String,
    pub smtp_server: String,
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    pub username: String,
    /// Password — required on first setup; omit (or empty) to keep existing.
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub primary: bool,
}

fn default_smtp_port() -> u16 { 465 }

#[derive(Deserialize)]
pub struct EmailSetupRequest {
    pub identities: Vec<IdentitySetupEntry>,
}

/// POST /api/email/setup — save all email identities to config.toml + passwords to secrets.toml.
pub async fn email_setup(
    Json(req): Json<EmailSetupRequest>,
) -> Json<serde_json::Value> {
    if req.identities.is_empty() {
        return Json(serde_json::json!({"ok": false, "error": "At least one identity required"}));
    }

    let config_path = crate::config::config_dir().join("config.toml");
    if let Err(e) = std::fs::create_dir_all(crate::config::config_dir()) {
        return Json(serde_json::json!({"ok": false, "error": e.to_string()}));
    }

    // Read existing config and strip out all email-related sections
    let existing = std::fs::read_to_string(&config_path).unwrap_or_default();
    let stripped = strip_email_sections(&existing);

    // Build new [[email.identities]] blocks
    let mut new_email = String::new();
    for (i, id) in req.identities.iter().enumerate() {
        let primary = id.primary || (i == 0 && !req.identities.iter().any(|x| x.primary));
        new_email.push_str(&format!(
            "\n[[email.identities]]\nlabel = \"{}\"\nfrom_email = \"{}\"\nfrom_name = \"{}\"\nsmtp_server = \"{}\"\nsmtp_port = {}\nusername = \"{}\"\nprimary = {}\n",
            esc_toml(&id.label), esc_toml(&id.from_email), esc_toml(&id.from_name),
            esc_toml(&id.smtp_server), id.smtp_port, esc_toml(&id.username), primary
        ));
    }

    if let Err(e) = std::fs::write(&config_path, format!("{}{}", stripped.trim_end(), new_email)) {
        return Json(serde_json::json!({"ok": false, "error": e.to_string()}));
    }

    // Store passwords in secrets.toml (only for identities with a non-empty password)
    let mut secrets = match crate::billing::secrets::BillingSecrets::load() {
        Ok(s) => s,
        Err(e) => return Json(serde_json::json!({"ok": false, "error": e.to_string()})),
    };
    for id in &req.identities {
        if !id.password.is_empty() {
            secrets.set_email_password(&id.username, &id.password);
        }
    }
    if let Err(e) = secrets.save() {
        return Json(serde_json::json!({"ok": false, "error": e.to_string()}));
    }

    Json(serde_json::json!({"ok": true, "identities": req.identities.len()}))
}

// ---------------------------------------------------------------------------
// M365 OAuth device-flow — colleague-friendly "click a button" path.
//
// Pairs with the "Add Microsoft 365 account" affordance in admin.html:
//   1. UI clicks /api/email/m365/begin → dashboard calls Microsoft's
//      /devicecode endpoint, returns the user_code + verification URL.
//   2. UI shows a modal with the code + URL; user authenticates in browser.
//   3. UI polls /api/email/m365/poll every few seconds; when Microsoft
//      issues tokens, they land in the keychain.
//   4. UI posts /api/email/m365/setup to save the identity block with
//      backend=graph, auth=oauth2_command pointing at the cohs-oauth-graph
//      helper (which reads those same keychain entries).
// ---------------------------------------------------------------------------

/// POST /api/email/m365/begin — start a device-code flow.
pub async fn email_m365_begin() -> Json<serde_json::Value> {
    // Run blocking HTTP in a blocking task so we don't stall the runtime.
    let result = tokio::task::spawn_blocking(crate::email::m365_oauth::begin).await;
    match result {
        Ok(Ok(start)) => Json(serde_json::json!({
            "ok": true,
            "user_code": start.user_code,
            "verification_uri": start.verification_uri,
            "device_code": start.device_code,
            "expires_in": start.expires_in,
            "interval": start.interval,
            "message": start.message,
        })),
        Ok(Err(e)) => Json(serde_json::json!({"ok": false, "error": e.to_string()})),
        Err(e) => Json(serde_json::json!({"ok": false, "error": format!("join error: {e}")})),
    }
}

#[derive(Deserialize)]
pub struct M365PollRequest {
    pub device_code: String,
}

/// POST /api/email/m365/poll — check whether the device flow has completed.
pub async fn email_m365_poll(
    Json(req): Json<M365PollRequest>,
) -> Json<serde_json::Value> {
    let code = req.device_code.clone();
    let result = tokio::task::spawn_blocking(move || {
        crate::email::m365_oauth::poll(&code)
    })
    .await;
    match result {
        Ok(Ok(poll_result)) => Json(serde_json::to_value(&poll_result).unwrap_or_else(|e| {
            serde_json::json!({"status": "error", "message": e.to_string()})
        })),
        Ok(Err(e)) => Json(serde_json::json!({"status": "error", "message": e.to_string()})),
        Err(e) => Json(serde_json::json!({"status": "error", "message": format!("join error: {e}")})),
    }
}

#[derive(Deserialize)]
pub struct M365SetupRequest {
    pub label: String,
    pub from_email: String,
    pub from_name: String,
    #[serde(default)]
    pub primary: bool,
}

/// POST /api/email/m365/setup — write a Graph-backed identity to config.toml.
/// Prerequisites: the OAuth device flow completed successfully (tokens in
/// keychain). No password to store — Graph uses the OAuth2 token command.
pub async fn email_m365_setup(
    Json(req): Json<M365SetupRequest>,
) -> Json<serde_json::Value> {
    if req.from_email.trim().is_empty() {
        return Json(serde_json::json!({"ok": false, "error": "from_email is required"}));
    }

    let config_path = crate::config::config_dir().join("config.toml");
    if let Err(e) = std::fs::create_dir_all(crate::config::config_dir()) {
        return Json(serde_json::json!({"ok": false, "error": e.to_string()}));
    }
    let existing = std::fs::read_to_string(&config_path).unwrap_or_default();

    // Build a new identity entry using the tagged shape that the Phase 3a
    // config loader understands.
    let entry = crate::email::wizard::IdentityEntry {
        label: req.label.clone(),
        from_email: req.from_email.clone(),
        from_name: if req.from_name.is_empty() { req.from_email.clone() } else { req.from_name.clone() },
        primary: req.primary,
        backend: crate::email::backends::BackendConfig::Graph(
            crate::email::backends::GraphConfig::default(),
        ),
        // In-Rust refresh-on-read. Replaces the prior shell-out to
        // `cohs-oauth-graph show`; works on Windows (no Python required)
        // and saves the per-send subprocess hop on Mac/Linux. Old configs
        // that still carry `OAuth2Command { command: "cohs-oauth-graph
        // show" }` continue to dispatch through the legacy shim.
        auth: crate::email::backends::AuthConfig::KeychainM365,
    };
    // Note: we delegate to the wizard's `append_identity` which already
    // handles legacy-flat promotion, primary flag juggling, and pretty
    // serde output. This is the only web caller that writes tagged-shape
    // identities today; the SMTP setup endpoint above stays on the
    // flat-shape writer.
    let updated = match crate::email::wizard::append_identity(&existing, &entry) {
        Ok(s) => s,
        Err(e) => return Json(serde_json::json!({"ok": false, "error": e.to_string()})),
    };
    if let Err(e) = std::fs::write(&config_path, updated) {
        return Json(serde_json::json!({"ok": false, "error": e.to_string()}));
    }

    Json(serde_json::json!({
        "ok": true,
        "from_email": req.from_email,
        "label": req.label,
    }))
}

// ---------------------------------------------------------------------------
// Gmail OAuth auth-code flow — colleague-friendly "Add Gmail account" path.
//
// Mirror of the M365 endpoints above, with a different flow shape:
// Google's Desktop OAuth client doesn't support device-code, so we use
// auth-code with a loopback redirect back to this dashboard. Flow:
//
//   1. Frontend POSTs /api/email/gmail/begin → dashboard returns an
//      auth URL + state. Frontend opens the URL in a new browser tab.
//   2. User signs in at Google, consents. Google redirects back to
//      /api/email/gmail/callback?code=...&state=... on THIS dashboard
//      (loopback redirect requires dashboard be on the user's machine
//      or reachable at the configured redirect URI).
//   3. Callback handler exchanges code for tokens, stores in keychain,
//      serves a tiny HTML page that auto-closes the tab.
//   4. Frontend polls /api/email/gmail/poll?state=... for completion.
//   5. Once complete, user fills in label/display name; frontend posts
//      /api/email/gmail/setup which writes the SMTP+XOAUTH2 identity.
// ---------------------------------------------------------------------------

/// Redirect URI used for Google's auth-code callback. Hard-coded to the
/// admin-dashboard default port because that's what launchd serves. If
/// the dashboard runs on a different port, callers need a way to override
/// this (future work).
const GMAIL_REDIRECT_URI: &str = "http://127.0.0.1:3457/api/email/gmail/callback";

/// POST /api/email/gmail/begin — start the auth-code flow.
pub async fn email_gmail_begin() -> Json<serde_json::Value> {
    let result = tokio::task::spawn_blocking(|| {
        crate::email::gmail_oauth::begin(GMAIL_REDIRECT_URI)
    })
    .await;
    match result {
        Ok(Ok(start)) => Json(serde_json::json!({
            "ok": true,
            "auth_url": start.auth_url,
            "state": start.state,
            "redirect_uri": start.redirect_uri,
        })),
        Ok(Err(e)) => Json(serde_json::json!({"ok": false, "error": e.to_string()})),
        Err(e) => Json(serde_json::json!({"ok": false, "error": format!("join error: {e}")})),
    }
}

#[derive(Deserialize)]
pub struct GmailCallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

/// GET /api/email/gmail/callback — Google redirects here after user signs
/// in. Completes the token exchange and serves a tiny auto-closing HTML
/// page so the browser tab cleans itself up.
pub async fn email_gmail_callback(
    axum::extract::Query(q): axum::extract::Query<GmailCallbackQuery>,
) -> axum::response::Html<String> {
    let body = if let Some(err) = q.error {
        format!(
            "<!doctype html><html><body style='font-family:sans-serif;padding:20px'>\
             <h2 style='color:#c44'>Sign-in failed</h2>\
             <pre style='background:#222;color:#eee;padding:10px;border-radius:4px'>{}</pre>\
             <p>You can close this tab and return to PracticeForge.</p>\
             </body></html>",
            html_escape(&err)
        )
    } else if let (Some(code), Some(state)) = (q.code, q.state) {
        let outcome = tokio::task::spawn_blocking(move || {
            crate::email::gmail_oauth::handle_callback(&state, &code)
        })
        .await;
        match outcome {
            Ok(Ok(())) => String::from(
                "<!doctype html><html><body style='font-family:sans-serif;padding:20px'>\
                 <h2 style='color:#4a4'>Signed in</h2>\
                 <p>You can close this tab and return to PracticeForge.</p>\
                 <script>setTimeout(function(){window.close();},2000);</script>\
                 </body></html>",
            ),
            Ok(Err(e)) => format!(
                "<!doctype html><html><body style='font-family:sans-serif;padding:20px'>\
                 <h2 style='color:#c44'>Token exchange failed</h2>\
                 <pre style='background:#222;color:#eee;padding:10px;border-radius:4px'>{}</pre>\
                 </body></html>",
                html_escape(&e.to_string())
            ),
            Err(e) => format!(
                "<!doctype html><html><body style='font-family:sans-serif;padding:20px'>\
                 <h2 style='color:#c44'>Internal error</h2><pre>{}</pre></body></html>",
                html_escape(&e.to_string())
            ),
        }
    } else {
        String::from(
            "<!doctype html><html><body><h2>Missing code or state in callback</h2></body></html>",
        )
    };
    axum::response::Html(body)
}

#[derive(Deserialize)]
pub struct GmailPollRequest {
    pub state: String,
}

/// POST /api/email/gmail/poll — frontend checks whether the callback has
/// fired and stored tokens yet.
pub async fn email_gmail_poll(
    Json(req): Json<GmailPollRequest>,
) -> Json<serde_json::Value> {
    let status = tokio::task::spawn_blocking(move || {
        crate::email::gmail_oauth::poll_status(&req.state)
    })
    .await
    .unwrap_or(crate::email::gmail_oauth::FlowStatus::Error {
        message: "join error".to_string(),
    });
    Json(serde_json::to_value(&status).unwrap_or_else(|_| serde_json::json!({"status": "error"})))
}

#[derive(Deserialize)]
pub struct GmailSetupRequest {
    pub label: String,
    pub from_email: String,
    pub from_name: String,
    #[serde(default)]
    pub primary: bool,
}

/// POST /api/email/gmail/setup — write a Gmail SMTP+XOAUTH2 identity to
/// config.toml after a successful OAuth flow. Auth-command points at
/// `practiceforge email gmail-show`, a subcommand that refreshes + prints
/// the current access token (see src/email/gmail_oauth.rs::show()).
pub async fn email_gmail_setup(
    Json(req): Json<GmailSetupRequest>,
) -> Json<serde_json::Value> {
    if req.from_email.trim().is_empty() {
        return Json(serde_json::json!({"ok": false, "error": "from_email is required"}));
    }

    let config_path = crate::config::config_dir().join("config.toml");
    if let Err(e) = std::fs::create_dir_all(crate::config::config_dir()) {
        return Json(serde_json::json!({"ok": false, "error": e.to_string()}));
    }
    let existing = std::fs::read_to_string(&config_path).unwrap_or_default();

    let entry = crate::email::wizard::IdentityEntry {
        label: req.label.clone(),
        from_email: req.from_email.clone(),
        from_name: if req.from_name.is_empty() {
            req.from_email.clone()
        } else {
            req.from_name.clone()
        },
        primary: req.primary,
        backend: crate::email::backends::BackendConfig::Smtp(
            crate::email::backends::SmtpConfig {
                host: "smtp.gmail.com".to_string(),
                port: 465,
                encryption: crate::email::backends::Encryption::Tls,
                username: req.from_email.clone(),
                auth_mode: crate::email::backends::AuthMode::XOAuth2,
            },
        ),
        auth: crate::email::backends::AuthConfig::OAuth2Command {
            command: "practiceforge email gmail-show".to_string(),
        },
    };
    let updated = match crate::email::wizard::append_identity(&existing, &entry) {
        Ok(s) => s,
        Err(e) => return Json(serde_json::json!({"ok": false, "error": e.to_string()})),
    };
    if let Err(e) = std::fs::write(&config_path, updated) {
        return Json(serde_json::json!({"ok": false, "error": e.to_string()}));
    }

    Json(serde_json::json!({
        "ok": true,
        "from_email": req.from_email,
        "label": req.label,
    }))
}

// ---------------------------------------------------------------------------
// SMTP identity — single-identity add endpoint (matches M365/Gmail shape).
//
// The older `/api/email/setup` takes a full array and overwrites. This one
// appends a single SMTP identity, so the "add identity" UX can be uniform
// across SMTP, M365, and Gmail: always open a modal, always immediate save.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SmtpSetupRequest {
    pub label: String,
    pub from_email: String,
    pub from_name: String,
    pub smtp_server: String,
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    pub username: String,
    /// Password for keychain. Required for new SMTP identities.
    pub password: String,
    #[serde(default)]
    pub primary: bool,
}

/// POST /api/email/smtp/setup — append one SMTP+password identity and
/// store its password in the shared `clinical-email` keychain service.
pub async fn email_smtp_setup(
    Json(req): Json<SmtpSetupRequest>,
) -> Json<serde_json::Value> {
    if req.from_email.trim().is_empty() {
        return Json(serde_json::json!({"ok": false, "error": "from_email is required"}));
    }
    if req.password.is_empty() {
        return Json(serde_json::json!({"ok": false, "error": "password is required"}));
    }

    let encryption = if req.smtp_port == 465 {
        crate::email::backends::Encryption::Tls
    } else {
        crate::email::backends::Encryption::StartTls
    };

    let entry = crate::email::wizard::IdentityEntry {
        label: req.label.clone(),
        from_email: req.from_email.clone(),
        from_name: if req.from_name.is_empty() {
            req.from_email.clone()
        } else {
            req.from_name.clone()
        },
        primary: req.primary,
        backend: crate::email::backends::BackendConfig::Smtp(
            crate::email::backends::SmtpConfig {
                host: req.smtp_server.clone(),
                port: req.smtp_port,
                encryption,
                username: req.username.clone(),
                auth_mode: crate::email::backends::AuthMode::Password,
            },
        ),
        auth: crate::email::backends::AuthConfig::Password {
            keyring_service: "clinical-email".to_string(),
            keyring_account: req.username.clone(),
        },
    };

    // Store password in the secrets.toml used by legacy-path callers, so
    // anything still using the old `secrets.email_password()` lookup keeps
    // working. New callers via `KeychainPasswordSource` look it up under
    // the "clinical-email" keychain service; we write there too for
    // consistency with the wizard.
    let mut secrets = match crate::billing::secrets::BillingSecrets::load() {
        Ok(s) => s,
        Err(e) => return Json(serde_json::json!({"ok": false, "error": e.to_string()})),
    };
    secrets.set_email_password(&req.username, &req.password);
    if let Err(e) = secrets.save() {
        return Json(serde_json::json!({"ok": false, "error": e.to_string()}));
    }

    // Also write to macOS/Linux keychain for the new `KeychainPasswordSource`
    // lookup path. Delegated to a small helper that mirrors what the wizard
    // does in `store_password`.
    if let Err(e) = store_keychain_password("clinical-email", &req.username, &req.password) {
        return Json(serde_json::json!({"ok": false, "error": format!("keychain write failed: {e}")}));
    }

    let config_path = crate::config::config_dir().join("config.toml");
    if let Err(e) = std::fs::create_dir_all(crate::config::config_dir()) {
        return Json(serde_json::json!({"ok": false, "error": e.to_string()}));
    }
    let existing = std::fs::read_to_string(&config_path).unwrap_or_default();
    let updated = match crate::email::wizard::append_identity(&existing, &entry) {
        Ok(s) => s,
        Err(e) => return Json(serde_json::json!({"ok": false, "error": e.to_string()})),
    };
    if let Err(e) = std::fs::write(&config_path, updated) {
        return Json(serde_json::json!({"ok": false, "error": e.to_string()}));
    }

    Json(serde_json::json!({"ok": true, "from_email": req.from_email, "label": req.label}))
}

/// Write a password into the OS keystore under service/account.
fn store_keychain_password(service: &str, account: &str, password: &str) -> Result<(), String> {
    crate::keystore::set(service, account, password).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Delete an identity by from_email.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct IdentityDeleteRequest {
    pub from_email: String,
}

/// POST /api/email/identity/delete — remove one `[[email.identities]]`
/// block from config.toml by `from_email`. Doesn't touch keychain entries
/// or OAuth tokens (those may be shared / useful for re-adding later).
pub async fn email_identity_delete(
    Json(req): Json<IdentityDeleteRequest>,
) -> Json<serde_json::Value> {
    let config_path = crate::config::config_dir().join("config.toml");
    let existing = match std::fs::read_to_string(&config_path) {
        Ok(s) => s,
        Err(e) => return Json(serde_json::json!({"ok": false, "error": e.to_string()})),
    };

    // Parse, filter, reserialize. We use the same Identity/IdentityEntry
    // round-trip the wizard uses, so the output keeps the canonical tagged
    // shape.
    let parsed: toml::Value = match existing.parse() {
        Ok(v) => v,
        Err(e) => return Json(serde_json::json!({"ok": false, "error": e.to_string()})),
    };
    let mut root = match parsed.as_table() {
        Some(t) => t.clone(),
        None => return Json(serde_json::json!({"ok": false, "error": "config.toml top level is not a table"})),
    };

    if let Some(toml::Value::Table(email_tbl)) = root.get_mut("email") {
        if let Some(toml::Value::Array(arr)) = email_tbl.get_mut("identities") {
            let before = arr.len();
            arr.retain(|item| {
                item.as_table()
                    .and_then(|t| t.get("from_email"))
                    .and_then(|v| v.as_str())
                    .map(|addr| addr != req.from_email)
                    .unwrap_or(true)
            });
            if arr.len() == before {
                return Json(serde_json::json!({"ok": false, "error": format!("no identity found with from_email={}", req.from_email)}));
            }
        }
    }

    let rewritten = match toml::to_string(&root) {
        Ok(s) => s,
        Err(e) => return Json(serde_json::json!({"ok": false, "error": e.to_string()})),
    };
    if let Err(e) = std::fs::write(&config_path, rewritten) {
        return Json(serde_json::json!({"ok": false, "error": e.to_string()}));
    }
    Json(serde_json::json!({"ok": true}))
}

// ---------------------------------------------------------------------------
// Change which identity is primary.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct IdentityPrimaryRequest {
    pub from_email: String,
}

/// POST /api/email/identity/set-primary — mark one identity as primary
/// and demote all others. Handles the case where no identity was ever
/// explicitly primary (load-time "first one wins" fallback becomes an
/// explicit `primary = true` flag after this call).
pub async fn email_identity_set_primary(
    Json(req): Json<IdentityPrimaryRequest>,
) -> Json<serde_json::Value> {
    let config_path = crate::config::config_dir().join("config.toml");
    let existing = match std::fs::read_to_string(&config_path) {
        Ok(s) => s,
        Err(e) => return Json(serde_json::json!({"ok": false, "error": e.to_string()})),
    };
    let parsed: toml::Value = match existing.parse() {
        Ok(v) => v,
        Err(e) => return Json(serde_json::json!({"ok": false, "error": e.to_string()})),
    };
    let mut root = match parsed.as_table() {
        Some(t) => t.clone(),
        None => return Json(serde_json::json!({"ok": false, "error": "config.toml top level is not a table"})),
    };

    let mut found = false;
    if let Some(toml::Value::Table(email_tbl)) = root.get_mut("email") {
        if let Some(toml::Value::Array(arr)) = email_tbl.get_mut("identities") {
            for item in arr.iter_mut() {
                if let Some(t) = item.as_table_mut() {
                    let addr = t.get("from_email").and_then(|v| v.as_str()).unwrap_or("");
                    let should_be_primary = addr == req.from_email;
                    if should_be_primary {
                        found = true;
                    }
                    t.insert("primary".to_string(), toml::Value::Boolean(should_be_primary));
                }
            }
        }
    }

    if !found {
        return Json(serde_json::json!({"ok": false, "error": format!("no identity found with from_email={}", req.from_email)}));
    }

    let rewritten = match toml::to_string(&root) {
        Ok(s) => s,
        Err(e) => return Json(serde_json::json!({"ok": false, "error": e.to_string()})),
    };
    if let Err(e) = std::fs::write(&config_path, rewritten) {
        return Json(serde_json::json!({"ok": false, "error": e.to_string()}));
    }
    Json(serde_json::json!({"ok": true}))
}

/// Minimal HTML escape for error-message display in the callback page.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn esc_toml(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Remove all [email] and [[email.identities]] sections from a config string.
fn strip_email_sections(config: &str) -> String {
    let mut out = String::new();
    let mut skip = false;
    for line in config.lines() {
        let trimmed = line.trim();
        if trimmed == "[email]" || trimmed.starts_with("[email.") || trimmed == "[[email.identities]]" {
            skip = true;
        } else if skip && trimmed.starts_with('[') {
            skip = false;
        }
        if !skip {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// POST /api/email/test — send a test email to the primary (or specified) identity.
pub async fn email_test(
    body: Option<Json<serde_json::Value>>,
) -> Json<serde_json::Value> {
    let from_email = body
        .as_ref()
        .and_then(|b| b.get("from_email"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    use crate::email::backends::BackendConfig;

    let identities = crate::email::load_identities();
    if identities.is_empty() {
        return Json(serde_json::json!({"ok": false, "error": "Email not configured"}));
    }

    let identity = if let Some(ref fe) = from_email {
        match identities.iter().find(|i| &i.from_email == fe) {
            Some(id) => id,
            None => return Json(serde_json::json!({"ok": false, "error": format!("No identity for {}", fe)})),
        }
    } else {
        identities.iter().find(|i| i.primary).unwrap_or(&identities[0])
    };

    // Describe the transport for the test-email body — Graph has no host/port.
    let transport_desc = match &identity.config.backend {
        BackendConfig::Smtp(cfg) => format!("SMTP {}:{}", cfg.host, cfg.port),
        BackendConfig::Graph(_) => "Microsoft Graph".to_string(),
    };

    match crate::email::send_as(
        &identity.from_email,
        &identity.from_email,
        &identity.from_name,
        "PracticeForge — Email Test",
        crate::email::Body::Text(format!(
            "This is a test email from PracticeForge.\n\nIdentity: {}\nTransport: {}\nFrom: {} <{}>",
            identity.label, transport_desc,
            identity.from_name, identity.from_email
        )),
        None,
        None,
    ) {
        Ok(_) => Json(serde_json::json!({"ok": true, "sent_to": identity.from_email})),
        Err(e) => Json(serde_json::json!({"ok": false, "error": format!("Test failed: {}", e)})),
    }
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

