//! scroll-server library. See `main.rs` for the binary entry point and
//! ~/Assistants/shared/scroll-server-design.md for the full design spec.

pub mod audit;
pub mod config;
pub mod handler;
pub mod mcp;
pub mod ratelimit;

use anyhow::Result;
use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;

use crate::audit::AuditLog;
use crate::config::Config;
use crate::handler::{fallback, healthz, scroll, AppState};
use crate::ratelimit::RateLimiter;

/// Construct the axum router from a fully-validated `Config`. Returns the
/// router so callers (binary + tests) can either run it or bind it to a
/// specific listener.
pub fn build_app(cfg: &Config, audit_log: Arc<AuditLog>) -> Router {
    let state = AppState {
        scroll_dir: cfg.scroll_dir.clone(),
        rate_limiter: Arc::new(RateLimiter::new()),
        audit_log,
        mcp_token: cfg.mcp_token.clone(),
    };

    let mut router = Router::new().route("/healthz", get(healthz));

    // Registered only when a token is configured — an unset token must not
    // leave an unauthenticated MCP endpoint reachable.
    if cfg.mcp_token.is_some() {
        router = router
            .route("/mcp/{token}", post(crate::mcp::mcp_post).get(crate::mcp::mcp_get));
    }

    router
        .route("/{slug}", get(scroll))
        .fallback(fallback)
        .with_state(state)
}

/// Run the server until Ctrl-C. Used by `main`; the integration tests bind
/// their own listener so they can pick an ephemeral port.
pub async fn serve(cfg: Config) -> Result<()> {
    let audit_log = Arc::new(AuditLog::new(cfg.audit_log.clone()));
    audit_log.purge_old().await?;
    audit_log.clone().spawn_purge_task();

    let app = build_app(&cfg, audit_log);

    let listener = tokio::net::TcpListener::bind(cfg.bind).await?;
    eprintln!("scroll-server listening on {}", cfg.bind);
    axum::serve(listener, app).await?;
    Ok(())
}
