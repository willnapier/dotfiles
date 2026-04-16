//! Route definitions for the admin dashboard.

use axum::{
    extract::Query,
    http::{header, StatusCode},
    response::{Html, IntoResponse, Redirect},
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;

use super::handlers;

const ADMIN_HTML: &str = include_str!("../admin_dashboard_assets/admin.html");
const LOGIN_HTML: &str = include_str!("../admin_dashboard_assets/login.html");

/// Path to the admin HTML file for dev mode (live reload without recompile).
const DEV_HTML_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/admin_dashboard_assets/admin.html");
const DEV_LOGIN_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/admin_dashboard_assets/login.html");

/// Build the complete router for the admin dashboard.
pub fn build_router() -> Router {
    Router::new()
        // HTML UI
        .route("/", get(index_page))
        // Auth
        .route("/login", get(login_page))
        .route("/api/auth/send-code", post(auth_send_code))
        .route("/api/auth/verify", post(auth_verify))
        .route("/api/auth/status", get(auth_status))
        // Client management
        .route("/api/clients", get(handlers::list_clients))
        .route("/api/clients/{id}", get(handlers::get_client).post(handlers::update_client))
        .route("/api/clients/{id}/assignments", get(handlers::get_assignments))
        // Calendar
        .route("/api/calendar", get(handlers::calendar))
        .route("/api/calendar/ics", get(handlers::calendar_ics))
        // Search
        .route("/api/search", get(handlers::search))
        // Billing
        .route("/api/billing/status", get(handlers::billing_status))
        .route("/api/billing/summary", get(handlers::billing_summary))
        .route("/api/billing/invoice", post(handlers::create_invoice))
        .route("/api/billing/invoice-batch", post(handlers::create_invoice_batch))
        .route("/api/billing/paid", post(handlers::mark_paid))
        .route("/api/billing/cancel", post(handlers::cancel_invoice))
        .route("/api/billing/reminders", get(handlers::list_reminders))
        .route("/api/billing/reminders/send", post(handlers::send_reminder))
        // Practice info
        .route("/api/practice", get(handlers::practice_info))
        .route("/api/practitioners", get(handlers::practitioners))
        // Scheduling actions
        .route("/api/schedule/create", post(handlers::schedule_create))
        .route("/api/schedule/cancel", post(handlers::schedule_cancel))
        .route("/api/schedule/move", post(handlers::schedule_move))
        .route("/api/schedule/blocks", get(handlers::schedule_blocks))
        // Email setup
        .route("/api/email/status", get(handlers::email_status))
        .route("/api/email/setup", post(handlers::email_setup))
        .route("/api/email/test", post(handlers::email_test))
        // Letter workflow
        .route("/api/letter/draft", post(handlers::letter_draft))
        .route("/api/letter/build", post(handlers::letter_build))
        .route("/api/letter/send", post(handlers::letter_send))
        // Clinic workflow
        .route("/api/session", get(handlers::get_session).put(handlers::save_session))
        .route("/api/generate", post(handlers::generate_note))
        .route("/api/generate-stream", post(handlers::generate_note_stream))
        .route("/api/save-note", post(handlers::save_note))
        .route("/api/client/{id}/notes", get(handlers::get_client_notes))
        .route("/api/client/{id}/metadata", get(handlers::get_client_metadata))
        .route("/api/inference/status", get(handlers::inference_status))
        .route("/api/end-clinic", post(handlers::end_clinic))
}

/// Serves admin.html if authenticated, otherwise redirects to /login.
async fn index_page(
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    // Check for valid session cookie
    if !is_authenticated(&headers) {
        return Redirect::to("/login").into_response();
    }

    if std::env::var("PF_DEV").is_ok() {
        if let Ok(content) = std::fs::read_to_string(DEV_HTML_PATH) {
            return Html(content).into_response();
        }
    }
    Html(ADMIN_HTML.to_string()).into_response()
}

/// Login page — clean, focused, mobile-friendly.
async fn login_page() -> Html<String> {
    if std::env::var("PF_DEV").is_ok() {
        if let Ok(content) = std::fs::read_to_string(DEV_LOGIN_PATH) {
            return Html(content);
        }
    }
    Html(LOGIN_HTML.to_string())
}

fn is_authenticated(headers: &axum::http::HeaderMap) -> bool {
    // Dev bypass: PF_NO_AUTH=1 skips auth (for local development)
    if std::env::var("PF_NO_AUTH").is_ok() {
        return true;
    }

    let cookie_header = match headers.get(header::COOKIE) {
        Some(c) => c.to_str().unwrap_or(""),
        None => return false,
    };

    // Look for pf_session=<token>
    for pair in cookie_header.split(';') {
        let pair = pair.trim();
        if let Some(token) = pair.strip_prefix("pf_session=") {
            return handlers::validate_session_token(token);
        }
    }
    false
}

#[derive(Deserialize)]
struct AuthSendCodeRequest {
    email: String,
}

async fn auth_send_code(
    Json(req): Json<AuthSendCodeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    handlers::auth_send_code_handler(&req.email).await
}

#[derive(Deserialize)]
struct AuthVerifyRequest {
    email: String,
    code: String,
}

async fn auth_verify(
    Json(req): Json<AuthVerifyRequest>,
) -> Result<axum::response::Response, (StatusCode, String)> {
    let token = handlers::auth_verify_handler(&req.email, &req.code)?;

    // Set session cookie — 30 days, HttpOnly, SameSite=Strict
    let cookie = format!(
        "pf_session={}; Path=/; Max-Age=2592000; HttpOnly; SameSite=Strict",
        token
    );

    Ok((
        [(header::SET_COOKIE, cookie)],
        Json(serde_json::json!({"ok": true})),
    ).into_response())
}

async fn auth_status(
    headers: axum::http::HeaderMap,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"authenticated": is_authenticated(&headers)}))
}
