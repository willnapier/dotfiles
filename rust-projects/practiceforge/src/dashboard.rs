//! Product Dashboard — local web UI for clinical note-writing.
//!
//! Serves a single-page app on 127.0.0.1 that wraps the `clinical` CLI
//! tools in a browser interface: browse today's appointments, select a
//! client, dictate/type an observation, stream a generated note, then
//! accept or reject it.
//!
//! All static assets are embedded in the binary via `include_str!`.

use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;

// ---------------------------------------------------------------------------
// Embedded assets
// ---------------------------------------------------------------------------

const INDEX_HTML: &str = include_str!("dashboard_assets/index.html");
const STYLE_CSS: &str = include_str!("dashboard_assets/style.css");
const APP_JS: &str = include_str!("dashboard_assets/app.js");

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

fn clients_dir() -> PathBuf {
    crate::config::clients_dir()
}

fn attendance_dir() -> PathBuf {
    crate::config::attendance_dir()
}

// ---------------------------------------------------------------------------
// JSON types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct Appointment {
    client_id: String,
    time: String,
    name: String,
}

#[derive(Serialize)]
struct ClientSummary {
    id: String,
    has_identity: bool,
}

#[derive(Serialize)]
struct ClientInfo {
    id: String,
    referrer: Option<String>,
    funding: Option<String>,
    session_count: Option<u64>,
    modality: Option<String>,
}

#[derive(Deserialize)]
struct NoteRequest {
    client_id: String,
    observation: String,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize)]
struct SaveRequest {
    client_id: String,
    note: String,
}

#[derive(Serialize)]
struct SaveResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Payload from the dashboard's compare view. Contains both variants
/// plus the user's accepted choice (or None if rejected).
#[derive(Deserialize)]
struct LogPairRequest {
    client_id: String,
    observation: String,
    q4_note: String,
    q4_generation_secs: f64,
    q8_note: String,
    q8_generation_secs: f64,
    /// "q4", "q8", or null (rejected)
    #[serde(default)]
    accepted: Option<String>,
}

/// One row appended to ~/Clinical/comparisons.jsonl.
/// Mirrors `clinical::faithfulness::ComparisonEntry` schema so downstream
/// analysis tools can consume both CLI-generated and dashboard-generated rows.
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
}

// ---------------------------------------------------------------------------
// Server entry point
// ---------------------------------------------------------------------------

/// Start the dashboard on `127.0.0.1:{port}`.
///
/// If `open_browser` is true, attempt to open the URL in the default browser.
pub async fn serve(port: u16, open_browser: bool) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/", get(index_page))
        .route("/api/today", get(todays_appointments))
        .route("/api/clients", get(list_clients))
        .route("/api/client/{id}", get(client_info))
        .route("/api/note", post(generate_note))
        .route("/api/note/save", post(save_note))
        .route("/api/note/log-pair", post(log_pair))
        // Billing API — all routes return 404 if billing is disabled
        .route("/api/billing/config", get(billing_config))
        .route("/api/billing/invoices", get(billing_invoices))
        .route("/api/billing/invoice/{id}", post(billing_create_invoice))
        .route("/api/billing/paid", post(billing_mark_paid))
        .route("/api/billing/cancel", post(billing_cancel))
        .route("/api/billing/reminders", get(billing_reminders))
        .route("/style.css", get(serve_css))
        .route("/app.js", get(serve_js));

    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("Dashboard running at http://{addr}");

    if open_browser {
        // Best-effort; ignore errors.
        let _ = std::process::Command::new("xdg-open")
            .arg(format!("http://{addr}"))
            .spawn();
    }

    axum::serve(listener, app).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Static asset handlers
// ---------------------------------------------------------------------------

async fn index_page() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn serve_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css")], STYLE_CSS)
}

async fn serve_js() -> impl IntoResponse {
    ([
        (header::CONTENT_TYPE, "application/javascript"),
    ], APP_JS)
}

// ---------------------------------------------------------------------------
// GET /api/today
// ---------------------------------------------------------------------------

async fn todays_appointments() -> Result<Json<Vec<Appointment>>, (StatusCode, String)> {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let att_dir = attendance_dir();

    // Try to find today's attendance file (e.g. 2026-04-12.yaml, 2026-04-12.txt, etc.)
    if att_dir.is_dir() {
        let mut appointments = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&att_dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname.contains(&today) {
                    // Try to parse lines as "HH:MM CLIENT_ID" or similar
                    if let Ok(contents) = std::fs::read_to_string(entry.path()) {
                        for line in contents.lines() {
                            let line = line.trim();
                            if line.is_empty() || line.starts_with('#') {
                                continue;
                            }
                            // Expect formats like "10:00 AB12" or "AB12 10:00" or just "AB12"
                            let parts: Vec<&str> = line.split_whitespace().collect();
                            match parts.len() {
                                0 => continue,
                                1 => {
                                    appointments.push(Appointment {
                                        client_id: parts[0].to_string(),
                                        time: String::new(),
                                        name: format!("Client {}", parts[0]),
                                    });
                                }
                                _ => {
                                    // First part that looks like a time, rest is client ID
                                    let (time, id) =
                                        if parts[0].contains(':') && parts[0].len() <= 5 {
                                            (parts[0].to_string(), parts[1].to_string())
                                        } else if parts[1].contains(':') && parts[1].len() <= 5 {
                                            (parts[1].to_string(), parts[0].to_string())
                                        } else {
                                            (String::new(), parts[0].to_string())
                                        };
                                    appointments.push(Appointment {
                                        client_id: id.clone(),
                                        time,
                                        name: format!("Client {id}"),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        if !appointments.is_empty() {
            // Sort by time so the schedule is chronological.
            appointments.sort_by(|a, b| a.time.cmp(&b.time));
            return Ok(Json(appointments));
        }
    }

    // Fallback: list all client directories as a picker
    let appointments = list_client_dirs()
        .into_iter()
        .map(|id| Appointment {
            name: format!("Client {id}"),
            client_id: id,
            time: String::new(),
        })
        .collect();

    Ok(Json(appointments))
}

// ---------------------------------------------------------------------------
// GET /api/clients
// ---------------------------------------------------------------------------

async fn list_clients() -> Result<Json<Vec<ClientSummary>>, (StatusCode, String)> {
    let summaries: Vec<ClientSummary> = list_client_dirs()
        .into_iter()
        .map(|id| {
            // Auto-detect layout: Route C has identity.yaml at root, Route A under private/
            let has_identity = clients_dir().join(&id).join("identity.yaml").exists()
                || clients_dir().join(&id).join("private").join("identity.yaml").exists();
            ClientSummary { id, has_identity }
        })
        .collect();
    Ok(Json(summaries))
}

// ---------------------------------------------------------------------------
// GET /api/client/{id}
// ---------------------------------------------------------------------------

async fn client_info(Path(id): Path<String>) -> Result<Json<ClientInfo>, (StatusCode, String)> {
    // Sanitise: only allow alphanumeric + hyphen + underscore
    if !id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '+') {
        return Err((StatusCode::BAD_REQUEST, "Invalid client ID".to_string()));
    }

    // Auto-detect layout: Route C has identity.yaml at root, Route A under private/
    let root_identity = clients_dir().join(&id).join("identity.yaml");
    let private_identity = clients_dir().join(&id).join("private").join("identity.yaml");
    let identity_path = if root_identity.exists() { root_identity } else { private_identity };
    let mut info = ClientInfo {
        id: id.clone(),
        referrer: None,
        funding: None,
        session_count: None,
        modality: None,
    };

    if identity_path.exists() {
        if let Ok(contents) = std::fs::read_to_string(&identity_path) {
            // Light YAML parsing — avoid pulling in a full YAML crate.
            // identity.yaml is a flat key: value file.
            for line in contents.lines() {
                let line = line.trim();
                if let Some((key, val)) = line.split_once(':') {
                    let key = key.trim().to_lowercase();
                    let val = val.trim().to_string();
                    if val.is_empty() {
                        continue;
                    }
                    match key.as_str() {
                        "referrer" | "referred_by" => info.referrer = Some(val),
                        "funding" | "funder" => info.funding = Some(val),
                        "sessions" | "session_count" => {
                            info.session_count = val.parse().ok();
                        }
                        "modality" => info.modality = Some(val),
                        // Deliberately skip name / dob / any PII fields
                        _ => {}
                    }
                }
            }
        }
    }

    // Try to count sessions from the notes file if not set from identity
    if info.session_count.is_none() {
        // Route C: notes.md, Route A: <id>.md
        let notes_file = clients_dir().join(&id).join("notes.md");
        let notes_file = if notes_file.exists() {
            notes_file
        } else {
            clients_dir().join(&id).join(format!("{}.md", id))
        };
        if let Ok(content) = std::fs::read_to_string(&notes_file) {
            let count = content.lines().filter(|l| l.starts_with("### ")).count();
            if count > 0 {
                info.session_count = Some(count as u64);
            }
        }
    }

    Ok(Json(info))
}

// ---------------------------------------------------------------------------
// POST /api/note — streaming note generation
// ---------------------------------------------------------------------------

async fn generate_note(
    Json(req): Json<NoteRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Validate client ID
    if !req
        .client_id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '+')
    {
        return Err((StatusCode::BAD_REQUEST, "Invalid client ID".to_string()));
    }
    if req.observation.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Observation cannot be empty".to_string(),
        ));
    }

    // Spawn `clinical note <id> "<observation>" --no-save --yes [--model-override MODEL]`
    let mut cmd = Command::new("clinical");
    cmd.arg("note")
        .arg(&req.client_id)
        .arg(&req.observation)
        .arg("--no-save")
        .arg("--yes");
    if let Some(ref model) = req.model {
        if !model.is_empty() {
            cmd.arg("--model-override").arg(model);
        }
    }
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to spawn clinical: {e}"),
            )
        })?;

    let stdout = child.stdout.take().ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        "No stdout from child".to_string(),
    ))?;

    // Stream stdout line-by-line via a channel to an SSE-like streaming body.
    let stream = async_stream::stream! {
        let reader = tokio::io::BufReader::new(stdout);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            // Each chunk is just the line text + newline.
            yield Ok::<_, std::convert::Infallible>(format!("{line}\n"));
        }

        // Wait for process to finish and capture any stderr on failure.
        match child.wait().await {
            Ok(status) if !status.success() => {
                yield Ok(format!("\n[error: clinical exited with {status}]\n"));
            }
            Err(e) => {
                yield Ok(format!("\n[error: {e}]\n"));
            }
            _ => {}
        }
    };

    let body = axum::body::Body::from_stream(stream);

    Ok((
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    ))
}

// ---------------------------------------------------------------------------
// POST /api/note/save
// ---------------------------------------------------------------------------

async fn save_note(
    Json(req): Json<SaveRequest>,
) -> Result<Json<SaveResponse>, (StatusCode, String)> {
    if !req
        .client_id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '+')
    {
        return Err((StatusCode::BAD_REQUEST, "Invalid client ID".to_string()));
    }

    // Pipe note text to `clinical note-save <id>` via stdin
    let mut child = Command::new("clinical")
        .arg("note-save")
        .arg(&req.client_id)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to spawn clinical note-save: {e}"),
            )
        })?;

    // Write the note to stdin
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin
            .write_all(req.note.as_bytes())
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("stdin write: {e}")))?;
        // Drop stdin to signal EOF
    }

    let output = child.wait_with_output().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("process error: {e}"),
        )
    })?;

    if output.status.success() {
        Ok(Json(SaveResponse {
            ok: true,
            error: None,
        }))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Ok(Json(SaveResponse {
            ok: false,
            error: Some(if stderr.is_empty() {
                format!("clinical note-save exited with {}", output.status)
            } else {
                stderr
            }),
        }))
    }
}

// ---------------------------------------------------------------------------
// POST /api/note/log-pair — record a Q4/Q8 comparison from the dashboard
// ---------------------------------------------------------------------------
//
// The dashboard streams two variants in parallel (see app.js handleCompare).
// After the user accepts one (or rejects both), this endpoint appends two
// rows to ~/Clinical/comparisons.jsonl in the same schema the CLI writes.
//
// Faithfulness stats (hard_failures / soft_flags / attempts / regen_reasons)
// are not tracked client-side — we pass zeroes/empties. Analysis tools that
// want those can re-check offline against the stored note + observation.

async fn log_pair(
    Json(req): Json<LogPairRequest>,
) -> Result<Json<SaveResponse>, (StatusCode, String)> {
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

    Ok(Json(SaveResponse { ok: true, error: None }))
}

// ---------------------------------------------------------------------------
// Billing API
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct BillingConfigResponse {
    enabled: bool,
    currency: String,
    payment_terms_days: i64,
    provider: String,
    payment_provider: String,
}

#[derive(Serialize)]
struct InvoiceSummaryResponse {
    reference: String,
    client_id: String,
    client_name: String,
    bill_to: String,
    total: f64,
    currency: String,
    issue_date: String,
    due_date: String,
    state: String,
    days_overdue: i64,
}

#[derive(Serialize)]
struct ReminderResponse {
    invoice_reference: String,
    client_name: String,
    tone: String,
    subject: String,
    body: String,
    to_name: String,
}

#[derive(Deserialize)]
struct PaidRequest {
    reference: String,
    date: Option<String>,
}

#[derive(Deserialize)]
struct CancelRequest {
    reference: String,
    reason: Option<String>,
}

async fn billing_config() -> Result<Json<BillingConfigResponse>, (StatusCode, String)> {
    let config = crate::billing::config::BillingConfig::load()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(BillingConfigResponse {
        enabled: config.enabled,
        currency: config.currency,
        payment_terms_days: config.payment_terms_days,
        provider: config.provider,
        payment_provider: config.payment_provider,
    }))
}

async fn billing_invoices() -> Result<Json<Vec<InvoiceSummaryResponse>>, (StatusCode, String)> {
    let config = crate::billing::config::BillingConfig::load()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !config.enabled {
        return Ok(Json(Vec::new()));
    }

    let provider = crate::billing::ManualProvider::new(&config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    use crate::billing::traits::{AccountingProvider, InvoiceFilter};
    let invoices = provider
        .list_invoices(InvoiceFilter::default())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response: Vec<InvoiceSummaryResponse> = invoices
        .into_iter()
        .filter(|i| {
            i.state != crate::billing::invoice::InvoiceState::Cancelled
                && i.state != crate::billing::invoice::InvoiceState::Paid
        })
        .map(|i| InvoiceSummaryResponse {
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

async fn billing_create_invoice(
    Path(client_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let config = crate::billing::config::BillingConfig::load()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !config.enabled {
        return Err((StatusCode::BAD_REQUEST, "Billing not enabled".to_string()));
    }

    let provider = crate::billing::ManualProvider::new(&config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let clients_dir = crate::config::clients_dir();
    let client_dir = clients_dir.join(&client_id);
    let identity_path = client_dir.join("identity.yaml");
    let notes_path = client_dir.join("notes.md");

    if !identity_path.exists() {
        return Err((StatusCode::NOT_FOUND, format!("No identity.yaml for {}", client_id)));
    }
    if !notes_path.exists() {
        return Err((StatusCode::NOT_FOUND, format!("No notes.md for {}", client_id)));
    }

    let notes = std::fs::read_to_string(&notes_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let all_sessions = crate::billing::invoice::extract_session_dates(&notes);
    let already_invoiced = provider
        .invoiced_dates_for_client(&client_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let uninvoiced =
        crate::billing::invoice::uninvoiced_sessions(&all_sessions, &already_invoiced);

    if uninvoiced.is_empty() {
        return Ok(Json(serde_json::json!({
            "ok": true,
            "created": false,
            "message": "No uninvoiced sessions"
        })));
    }

    use crate::billing::traits::AccountingProvider;
    let reference = provider
        .next_invoice_number()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let invoice = crate::billing::invoice::build_invoice(
        reference,
        &client_id,
        &identity_path,
        &uninvoiced,
        config.payment_terms_days,
        &config.currency,
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let result = provider
        .create_invoice(&invoice)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "ok": true,
        "created": true,
        "reference": result.reference,
        "total": invoice.total(),
        "sessions": uninvoiced.len(),
    })))
}

async fn billing_mark_paid(
    Json(req): Json<PaidRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let config = crate::billing::config::BillingConfig::load()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !config.enabled {
        return Err((StatusCode::BAD_REQUEST, "Billing not enabled".to_string()));
    }

    let provider = crate::billing::ManualProvider::new(&config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let date = req
        .date
        .unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string());

    use crate::billing::traits::AccountingProvider;
    provider
        .mark_paid(&req.reference, &date, None)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({"ok": true})))
}

async fn billing_cancel(
    Json(req): Json<CancelRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let config = crate::billing::config::BillingConfig::load()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !config.enabled {
        return Err((StatusCode::BAD_REQUEST, "Billing not enabled".to_string()));
    }

    let provider = crate::billing::ManualProvider::new(&config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    use crate::billing::traits::AccountingProvider;
    provider
        .cancel_invoice(&req.reference, req.reason.as_deref().unwrap_or("Cancelled"))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({"ok": true})))
}

async fn billing_reminders() -> Result<Json<Vec<ReminderResponse>>, (StatusCode, String)> {
    let config = crate::billing::config::BillingConfig::load()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !config.enabled {
        return Ok(Json(Vec::new()));
    }

    let provider = crate::billing::ManualProvider::new(&config)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    use crate::billing::traits::{AccountingProvider, InvoiceFilter};
    let overdue = provider
        .list_invoices(InvoiceFilter {
            overdue_only: true,
            ..Default::default()
        })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let practitioner = crate::email::load_email_config()
        .map(|c| c.from_name)
        .unwrap_or_else(|_| "The Practitioner".to_string());

    let due = crate::billing::remind::due_reminders(&config, &overdue);

    let reminders: Vec<ReminderResponse> = due
        .into_iter()
        .map(|(inv, tone)| {
            let is_insurer = !inv.bill_to_name.is_empty() && inv.bill_to_name != inv.client_name;
            let reminder = if is_insurer {
                crate::billing::remind::render_insurer_reminder(&inv, &practitioner)
            } else {
                crate::billing::remind::render_client_reminder(&inv, &tone, &practitioner, "")
            };
            ReminderResponse {
                invoice_reference: inv.reference,
                client_name: inv.client_name,
                tone,
                subject: reminder.subject,
                body: reminder.body,
                to_name: reminder.to_name,
            }
        })
        .collect();

    Ok(Json(reminders))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// List client directory names under ~/Clinical/clients/, sorted.
fn list_client_dirs() -> Vec<String> {
    let dir = clients_dir();
    let mut ids = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if let Some(name) = entry.file_name().to_str() {
                    // Skip hidden directories
                    if !name.starts_with('.') {
                        ids.push(name.to_string());
                    }
                }
            }
        }
    }
    ids.sort();
    ids
}
