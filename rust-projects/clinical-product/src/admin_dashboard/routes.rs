//! Route definitions for the admin dashboard.

use axum::{
    response::Html,
    routing::get,
    Router,
};

use super::handlers;

const ADMIN_HTML: &str = include_str!("../admin_dashboard_assets/admin.html");

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
}

async fn index_page() -> Html<&'static str> {
    Html(ADMIN_HTML)
}
