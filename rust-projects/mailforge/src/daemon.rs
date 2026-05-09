use anyhow::{Context, Result};
use axum::{
    Router,
    body::Body,
    extract::{Path as AxPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

/// Strict CSP for body iframe: no scripts, only same-origin styles + images,
/// `data:` allowed for inline images, no external network at all. Keeps
/// tracker pixels and beacons inert until the user explicitly opts in.
const CSP_STRICT: &str = "default-src 'self'; img-src 'self' data:; \
    style-src 'self' 'unsafe-inline'; script-src 'none'; \
    frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'";

/// Relaxed CSP allowing external images for emails the user has decided
/// are worth rendering. Both HTTP and HTTPS are allowed because real-world
/// email image URLs use both schemes — sendgrid's CDN serves over HTTP for
/// example. The user already trusts the sender enough to escalate to
/// mailforge; demanding HTTPS-only here just produces broken images on
/// otherwise-fine emails. Scripts, frames, and form submission stay blocked
/// to keep the dangerous categories inert.
const CSP_RELAXED: &str = "default-src 'self'; img-src 'self' data: http: https:; \
    style-src 'self' 'unsafe-inline'; script-src 'none'; \
    frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'";

#[derive(Debug, Default, Deserialize)]
struct ViewQuery {
    /// `?images=0` blocks external images for this view; default is to load
    /// them. The trust posture: by the time the user pressed `2m` to escalate
    /// an email to mailforge, they've already decided the sender is benign at
    /// the listing-level triage step. Forcing them to click-to-allow images
    /// for every render adds friction without meaningful safety benefit.
    /// The block-images toggle remains available for the rare suspicious case.
    #[serde(default)]
    images: Option<u8>,
}

impl ViewQuery {
    /// External images are loaded by default; only `?images=0` blocks them.
    fn images_allowed(&self) -> bool {
        self.images.unwrap_or(1) != 0
    }
}

use crate::manifest;

#[derive(Clone)]
struct AppState {
    cache: Arc<PathBuf>,
}

pub async fn run(port: u16) -> Result<()> {
    let cache = manifest::cache_root()?;
    std::fs::create_dir_all(&cache)
        .with_context(|| format!("mkdir {}", cache.display()))?;
    let state = AppState { cache: Arc::new(cache) };

    // Background refresh task for the compose-form address book. The
    // initial build is lazy (first request to /api/addresses pays the
    // cost), so spawning here only arms the 10-minute refresh loop —
    // it does not block startup.
    crate::mail::addresses::spawn_refresh_task();

    // mailforge subrouter: the browser-native MUA UI (listing, message,
    // compose, send, tag, search). Lives under /mail/* and /api/* — no
    // overlap with the /v/* asset routes. The mail subrouter is stateless
    // (notmuch CLI subprocesses, on-disk drafts), so it merges cleanly into
    // the stateful asset Router. See `~/Assistants/shared/mailforge-design.md`.
    //
    // The `/v/<uuid>` "wrapper" route used to render an HTML page containing
    // a second iframe pointing at body.html — a nested-iframe shape that
    // intermittently failed to load on first navigation. As of 2026-05-02
    // mailforge's message-view points its single iframe directly at
    // /v/<uuid>/body.html; the wrapper, render handler, and ViewQuery on
    // the wrapper route have all been deleted. body.html, raw.pdf, and the
    // cid/* asset routes remain — they're called by the new single-iframe
    // architecture. The meli `:pipe-message` standalone path (src/pipe.rs)
    // still works in degraded form (no header chrome, no image toggle); not
    // load-bearing, mailforge is the primary UI now.
    // User-facing routes — any request here counts as "practitioner is at
    // this machine" and bumps the activity record for mailcurator leadership.
    // Excludes /healthz (which may be polled by launchd / systemd / external
    // monitoring and would falsely keep the leader from rotating). See
    // `src/activity.rs` for the protocol and `mailcurator::leader` for the
    // read-and-decide side.
    let user_facing = Router::new()
        .route("/v/:id/body.html", get(serve_body))
        .route("/v/:id/raw.pdf", get(serve_pdf))
        .route("/v/:id/cid/:filename", get(serve_asset))
        .with_state(state)
        .merge(crate::mail::router())
        // Static assets for the mailforge UI (CSS + JS). ServeDir is
        // resolved relative to CARGO_MANIFEST_DIR at build time so the
        // installed binary still finds its assets — for a release
        // install the user must `cp -r static/ ~/.local/share/mailforge/`
        // and set MAILFORGE_STATIC_DIR=~/.local/share/mailforge/static
        // (or the impl agent introduces include_dir!-based embedding,
        // matching practiceforge's admin_dashboard_assets pattern).
        .nest_service(
            "/static",
            tower::ServiceBuilder::new()
                // Local-only dev daemon: never let the browser cache static
                // assets. Without this, edits to mailforge.css/keys.js
                // require manual cache-busting (DevTools "Disable cache" or
                // a Cmd+Shift+R hard reload that sometimes still misses
                // because Chrome cached the previous 404).
                .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-cache, no-store, must-revalidate"),
                ))
                .service(tower_http::services::ServeDir::new(
                    std::env::var("MAILFORGE_STATIC_DIR")
                        .unwrap_or_else(|_| {
                            format!("{}/static", env!("CARGO_MANIFEST_DIR"))
                        }),
                )),
        )
        .layer(axum::middleware::from_fn(crate::activity::middleware));

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .merge(user_facing)
        // Global no-store cache headers for /mail/* and /v/* dynamic routes
        // (the static branch above already has its own no-store layer
        // and is unaffected). Without this, the browser uses heuristic
        // caching on responses that lack explicit Cache-Control, which
        // produces intermittent "iframe loaded blank, refresh fixes it"
        // failures when a freshly-issued /v/<uuid>/body.html request
        // collides with a cached 404 from a prior UUID. For a
        // localhost-only single-user mail UI, re-fetching everything is
        // both correct and cheap (notmuch + local cache are fast).
        // Discovered 2026-05-04 — Sadhana Mala emails in particular
        // surfaced the failure mode.
        .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-cache, no-store, must-revalidate"),
        ));

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    tracing::info!("mailforge listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn entry_dir(state: &AppState, id: &str) -> Option<PathBuf> {
    if id.contains('/') || id.contains("..") {
        return None;
    }
    let p = state.cache.join(id);
    if p.is_dir() { Some(p) } else { None }
}

async fn serve_body(
    AxPath(id): AxPath<String>,
    Query(q): Query<ViewQuery>,
    State(state): State<AppState>,
) -> Response {
    let Some(dir) = entry_dir(&state, &id) else {
        return (StatusCode::NOT_FOUND, "no such view").into_response();
    };
    let csp = if q.images_allowed() { CSP_RELAXED } else { CSP_STRICT };
    serve_file_with_csp(&dir.join("body.html"), "text/html; charset=utf-8", Some(csp))
}

async fn serve_pdf(
    AxPath(id): AxPath<String>,
    State(state): State<AppState>,
) -> Response {
    let Some(dir) = entry_dir(&state, &id) else {
        return (StatusCode::NOT_FOUND, "no such view").into_response();
    };
    serve_file(&dir.join("doc.pdf"), "application/pdf")
}

async fn serve_asset(
    AxPath((id, filename)): AxPath<(String, String)>,
    State(state): State<AppState>,
) -> Response {
    let Some(dir) = entry_dir(&state, &id) else {
        return (StatusCode::NOT_FOUND, "no such view").into_response();
    };
    if filename.contains('/') || filename.contains("..") {
        return (StatusCode::BAD_REQUEST, "bad filename").into_response();
    }
    let path = dir.join("cid").join(&filename);
    let mime = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .to_string();
    serve_file(&path, &mime)
}

fn serve_file(path: &std::path::Path, content_type: &str) -> Response {
    serve_file_with_csp(path, content_type, None)
}

fn serve_file_with_csp(
    path: &std::path::Path,
    content_type: &str,
    csp: Option<&str>,
) -> Response {
    match std::fs::read(path) {
        Ok(bytes) => {
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_TYPE, content_type.parse().unwrap());
            if let Some(csp) = csp {
                if let Ok(value) = HeaderValue::from_str(csp) {
                    headers.insert(header::CONTENT_SECURITY_POLICY, value);
                }
            }
            (headers, Body::from(bytes)).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "file not found").into_response(),
    }
}
