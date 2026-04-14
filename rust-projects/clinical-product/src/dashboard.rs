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
