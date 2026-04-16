//! Route definitions for the admin dashboard.

use axum::{
    response::Html,
    routing::{get, post, put},
    Router,
};

use super::handlers;

const ADMIN_HTML: &str = include_str!("../admin_dashboard_assets/admin.html");

/// Path to the admin HTML file for dev mode (live reload without recompile).
const DEV_HTML_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/admin_dashboard_assets/admin.html");

/// Build the complete router for the admin dashboard.
pub fn build_router() -> Router {
    Router::new()
        // HTML UI
        .route("/", get(index_page))
        // Client management
        .route("/api/clients", get(handlers::list_clients))
        .route("/api/clients/{id}", get(handlers::get_client).post(handlers::update_client))
        .route("/api/clients/{id}/assignments", get(handlers::get_assignments))
        // Calendar
        .route("/api/calendar", get(handlers::calendar))
        // Search
        .route("/api/search", get(handlers::search))
        // Billing
        .route("/api/billing/status", get(handlers::billing_status))
        .route("/api/billing/summary", get(handlers::billing_summary))
        // Practice info
        .route("/api/practice", get(handlers::practice_info))
        .route("/api/practitioners", get(handlers::practitioners))
        // Clinic workflow
        .route("/api/session", get(handlers::get_session).put(handlers::save_session))
        .route("/api/generate", post(handlers::generate_note))
        .route("/api/save-note", post(handlers::save_note))
        .route("/api/client/{id}/notes", get(handlers::get_client_notes))
        .route("/api/client/{id}/metadata", get(handlers::get_client_metadata))
        .route("/api/inference/status", get(handlers::inference_status))
        .route("/api/end-clinic", post(handlers::end_clinic))
}

/// Serves admin.html. In dev mode (PF_DEV=1), reads from disk on every
/// request — edit the HTML, refresh the browser, no recompile needed.
/// In production, serves the compile-time embedded copy.
async fn index_page() -> Html<String> {
    if std::env::var("PF_DEV").is_ok() {
        if let Ok(content) = std::fs::read_to_string(DEV_HTML_PATH) {
            return Html(content);
        }
    }
    Html(ADMIN_HTML.to_string())
}
