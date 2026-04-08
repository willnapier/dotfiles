use axum::{
    extract::{Multipart, Path, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use std::path::PathBuf;

use crate::db::{self, Pool};
use crate::email;

#[derive(Clone)]
pub struct AppState {
    pub pool: Pool,
    pub storage_dir: PathBuf,
}

pub fn router(pool: Pool) -> Router {
    let storage_dir = PathBuf::from("storage");
    std::fs::create_dir_all(&storage_dir).expect("Failed to create storage directory");

    let state = AppState {
        pool,
        storage_dir,
    };

    Router::new()
        .route("/api/upload", post(upload))
        .route("/api/status", get(status))
        .route("/api/revoke/{token}", post(revoke))
        .route("/d/{token}", get(verify_page))
        .route("/d/{token}/send-code", post(send_code))
        .route("/d/{token}/verify", post(verify_code))
        .route("/d/{token}/pdf", get(serve_pdf))
        .with_state(state)
}

// --- Shared HTML templates ---

const SHARED_CSS: &str = r#"
    * { margin: 0; padding: 0; box-sizing: border-box; }
    body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
           background: #f8f9fa; color: #333; min-height: 100vh;
           display: flex; align-items: center; justify-content: center; }
    .card { background: white; border-radius: 8px; padding: 2.5rem;
             max-width: 420px; width: 90%; box-shadow: 0 2px 12px rgba(0,0,0,0.08); }
    h1 { font-size: 1.3rem; margin-bottom: 0.5rem; color: #1a1a2e; }
    p { color: #666; margin-bottom: 1.5rem; font-size: 0.95rem; line-height: 1.5; }
    .footer { text-align: center; margin-top: 1.5rem; font-size: 0.8rem; color: #999; }
    .icon { font-size: 2.5rem; margin-bottom: 1rem; }
"#;

const FOOTER_HTML: &str = r#"
    <div class="footer">
        William Napier · Chartered Counselling Psychologist<br>
        Change of Harley Street
    </div>
"#;

fn error_page(title: &str, message: &str, icon: &str) -> Html<String> {
    Html(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>{title}</title>
    <style>{SHARED_CSS}</style>
</head>
<body>
    <div class="card">
        <div class="icon">{icon}</div>
        <h1>{title}</h1>
        <p>{message}</p>
        {FOOTER_HTML}
    </div>
</body>
</html>"#
    ))
}

// --- Routes ---

/// CLI uploads a PDF and recipient details; portal stores it and emails the link
async fn upload(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let mut file_data: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;
    let mut recipient_email: Option<String> = None;
    let mut recipient_name: Option<String> = None;
    let mut client_id: Option<String> = None;
    let mut expiry_days: u32 = 14;

    while let Ok(Some(field)) = multipart.next_field().await {
        let field_name = field.name().unwrap_or_default().to_string();
        match field_name.as_str() {
            "file" => {
                filename = field.file_name().map(|s| s.to_string());
                file_data = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
                        .to_vec(),
                );
            }
            "recipient_email" => {
                recipient_email = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?,
                );
            }
            "recipient_name" => {
                recipient_name = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?,
                );
            }
            "client_id" => {
                client_id = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?,
                );
            }
            "expiry_days" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
                expiry_days = text.parse().unwrap_or(14);
            }
            _ => {}
        }
    }

    let file_data = file_data.ok_or((StatusCode::BAD_REQUEST, "No file provided".to_string()))?;
    let filename = filename.unwrap_or_else(|| "document.pdf".to_string());
    let recipient_email =
        recipient_email.ok_or((StatusCode::BAD_REQUEST, "No recipient_email".to_string()))?;
    let recipient_name = recipient_name.unwrap_or_else(|| "Colleague".to_string());

    let (doc_id, token) = db::insert_document(
        &state.pool,
        &filename,
        &recipient_email,
        &recipient_name,
        expiry_days,
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let pdf_dir = state.storage_dir.join(&doc_id);
    std::fs::create_dir_all(&pdf_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    std::fs::write(pdf_dir.join(&filename), &file_data)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let portal_url = std::env::var("CLINICAL_PORTAL_URL")
        .unwrap_or_else(|_| "http://localhost:3849".to_string());
    let link = format!("{portal_url}/d/{token}");

    email::send_link_email(&recipient_email, &recipient_name, &link, expiry_days)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Email failed: {e}")))?;

    // Notify Leigh (or whoever is configured) that a letter needs TM3 filing
    let cid = client_id.as_deref().unwrap_or("unknown");
    if let Err(e) = email::send_notify_email(&recipient_name, &recipient_email, cid, &filename) {
        eprintln!("Warning: notification email failed: {e}");
        // Non-fatal — the letter was still sent successfully
    }

    Ok(Json(serde_json::json!({
        "id": doc_id,
        "token": token,
        "link": link,
    })))
}

/// List all shared documents — called by CLI
async fn status(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let docs = db::list_documents(&state.pool)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!(docs)))
}

/// Revoke a document link — called by CLI
async fn revoke(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let doc = db::get_document_by_token(&state.pool, &token)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "Document not found".to_string()))?;

    if doc.revoked {
        return Ok(Json(serde_json::json!({"status": "already revoked"})));
    }

    db::revoke_document(&state.pool, &doc.id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    db::log_access(&state.pool, &doc.id, "", "", "revoked").ok();

    Ok(Json(serde_json::json!({"status": "revoked", "id": doc.id})))
}

/// Landing page — recipient enters their email to get a verification code
async fn verify_page(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Html<String>, Html<String>> {
    let doc = match db::get_document_by_token(&state.pool, &token) {
        Ok(Some(d)) => d,
        _ => return Err(error_page(
            "Document Not Found",
            "This link is not valid. It may have been mistyped or the document may no longer exist.",
            "🔍",
        )),
    };

    if doc.revoked {
        return Err(error_page(
            "Access Revoked",
            "The sender has revoked access to this document. If you believe this is an error, please contact the sender directly.",
            "🚫",
        ));
    }
    if doc.expires_at < Utc::now() {
        return Err(error_page(
            "Link Expired",
            "This link has expired. Please contact the sender to request a new link if you still need access to this document.",
            "⏰",
        ));
    }

    db::log_access(&state.pool, &doc.id, "", "", "page_view").ok();

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Secure Clinical Letter</title>
    <style>
        {SHARED_CSS}
        input {{ width: 100%; padding: 0.75rem; border: 1px solid #ddd; border-radius: 6px;
                 font-size: 1rem; margin-bottom: 1rem; }}
        input:focus {{ outline: none; border-color: #4a6fa5; }}
        button {{ width: 100%; padding: 0.75rem; background: #4a6fa5; color: white;
                  border: none; border-radius: 6px; font-size: 1rem; cursor: pointer; }}
        button:hover {{ background: #3d5d8a; }}
        .error {{ color: #c0392b; font-size: 0.9rem; margin-bottom: 1rem; }}
        #code-section {{ display: none; }}
    </style>
</head>
<body>
    <div class="card">
        <h1>Secure Clinical Letter</h1>
        <p>A document has been shared with you securely. Please verify your email address to access it.</p>

        <div id="email-section">
            <label for="email" style="display:block; font-size:0.85rem; color:#555; margin-bottom:0.4rem;">Your email address</label>
            <input type="email" id="email" placeholder="name@example.com" autocomplete="email">
            <button onclick="sendCode()">Send verification code</button>
            <div id="email-error" class="error"></div>
        </div>

        <div id="code-section">
            <p>A 6-digit verification code has been sent to your email. Please check your inbox.</p>
            <label for="code" style="display:block; font-size:0.85rem; color:#555; margin-bottom:0.4rem;">Verification code</label>
            <input type="text" id="code" placeholder="000000" maxlength="6" autocomplete="one-time-code" inputmode="numeric" style="text-align:center; letter-spacing:0.3em; font-size:1.2rem;">
            <button onclick="verifyCode()">Verify and access document</button>
            <div id="code-error" class="error"></div>
        </div>

        {FOOTER_HTML}
    </div>

    <script>
        const token = "{token}";
        let sendingCode = false;
        let verifyingCode = false;

        async function sendCode() {{
            if (sendingCode) return;
            const email = document.getElementById('email').value.trim();
            if (!email) return;
            if (!email.includes('@') || !email.includes('.')) {{
                document.getElementById('email-error').textContent = 'Please enter a valid email address.';
                return;
            }}
            sendingCode = true;
            document.getElementById('email-error').textContent = '';
            const resp = await fetch(`/d/${{token}}/send-code`, {{
                method: 'POST',
                headers: {{'Content-Type': 'application/json'}},
                body: JSON.stringify({{ email }})
            }});
            if (resp.ok) {{
                document.getElementById('email-section').style.display = 'none';
                document.getElementById('code-section').style.display = 'block';
                document.getElementById('code').focus();
            }} else {{
                const text = await resp.text();
                document.getElementById('email-error').textContent = text;
                sendingCode = false;
            }}
        }}

        async function verifyCode() {{
            if (verifyingCode) return;
            verifyingCode = true;
            const email = document.getElementById('email').value.trim();
            const code = document.getElementById('code').value.trim();
            const resp = await fetch(`/d/${{token}}/verify`, {{
                method: 'POST',
                headers: {{'Content-Type': 'application/json'}},
                body: JSON.stringify({{ email, code }})
            }});
            if (resp.ok) {{
                window.location.href = `/d/${{token}}/pdf`;
            }} else {{
                const text = await resp.text();
                document.getElementById('code-error').textContent = text;
                verifyingCode = false;
            }}
        }}

        document.getElementById('email').addEventListener('keypress', e => {{
            if (e.key === 'Enter') sendCode();
        }});
        document.getElementById('code').addEventListener('keypress', e => {{
            if (e.key === 'Enter') verifyCode();
        }});
    </script>
</body>
</html>"#
    );

    Ok(Html(html))
}

#[derive(Deserialize)]
struct EmailRequest {
    email: String,
}

/// Send OTP to the recipient's email (only if it matches the document's recipient)
async fn send_code(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Json(req): Json<EmailRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let doc = db::get_document_by_token(&state.pool, &token)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "Document not found".to_string()))?;

    if req.email.to_lowercase() != doc.recipient_email.to_lowercase() {
        return Ok(StatusCode::OK);
    }

    let code = format!("{:06}", rand::random::<u32>() % 1_000_000);

    db::store_otp(&state.pool, &doc.id, &req.email, &code)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    email::send_otp_email(&req.email, &code)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to send code: {e}")))?;

    db::log_access(&state.pool, &doc.id, "", "", "otp_sent").ok();

    Ok(StatusCode::OK)
}

#[derive(Deserialize)]
struct VerifyRequest {
    email: String,
    code: String,
}

/// Verify OTP, create session, set cookie, then client redirects to PDF
async fn verify_code(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Json(req): Json<VerifyRequest>,
) -> Result<Response, (StatusCode, String)> {
    let doc = db::get_document_by_token(&state.pool, &token)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::NOT_FOUND, "Document not found".to_string()))?;

    let valid = db::verify_otp(&state.pool, &doc.id, &req.email, &req.code)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if valid {
        let session_token = db::create_session(&state.pool, &doc.id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        db::log_access(&state.pool, &doc.id, "", "", "verified").ok();

        let cookie = format!(
            "clinical_session={session_token}; HttpOnly; SameSite=Strict; Path=/d/{token}; Max-Age=3600"
        );

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::SET_COOKIE, cookie)
            .body(axum::body::Body::empty())
            .unwrap())
    } else {
        Err((StatusCode::UNAUTHORIZED, "Invalid or expired code".to_string()))
    }
}

/// Serve the PDF — requires a valid session cookie from OTP verification
async fn serve_pdf(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, Html<String>> {
    let doc = match db::get_document_by_token(&state.pool, &token) {
        Ok(Some(d)) => d,
        _ => return Err(error_page("Document Not Found", "This document could not be found.", "🔍")),
    };

    if doc.revoked {
        return Err(error_page(
            "Access Revoked",
            "The sender has revoked access to this document. If you believe this is an error, please contact the sender directly.",
            "🚫",
        ));
    }
    if doc.expires_at < Utc::now() {
        return Err(error_page(
            "Link Expired",
            "This link has expired. Please contact the sender to request a new link if you still need access to this document.",
            "⏰",
        ));
    }

    let session_token = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies
                .split(';')
                .find_map(|c| c.trim().strip_prefix("clinical_session="))
        })
        .ok_or_else(|| error_page(
            "Verification Required",
            "Please verify your email address before accessing this document. Go back and enter your email to receive a verification code.",
            "🔐",
        ))?;

    let valid = db::validate_session(&state.pool, session_token, &doc.id)
        .unwrap_or(false);

    if !valid {
        return Err(error_page(
            "Session Expired",
            "Your session has expired. Please go back and verify your email address again to access this document.",
            "🔐",
        ));
    }

    let pdf_path = state.storage_dir.join(&doc.id).join(&doc.filename);
    let pdf_bytes = std::fs::read(&pdf_path)
        .map_err(|_| error_page("Document Unavailable", "This document could not be retrieved. Please contact the sender.", "⚠️"))?;

    db::log_access(&state.pool, &doc.id, "", "", "pdf_served").ok();

    let disposition = format!("inline; filename=\"{}\"", doc.filename);

    Ok((
        [
            ("content-type".to_string(), "application/pdf".to_string()),
            ("content-disposition".to_string(), disposition),
        ],
        pdf_bytes,
    ))
}
