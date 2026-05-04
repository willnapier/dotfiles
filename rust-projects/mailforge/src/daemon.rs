use anyhow::{Context, Result};
use axum::{
    Router,
    body::Body,
    extract::{Path as AxPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
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

use crate::manifest::{self, Manifest};

#[derive(Clone)]
struct AppState {
    cache: Arc<PathBuf>,
}

pub async fn run(port: u16) -> Result<()> {
    let cache = manifest::cache_root()?;
    std::fs::create_dir_all(&cache)
        .with_context(|| format!("mkdir {}", cache.display()))?;
    let state = AppState { cache: Arc::new(cache) };

    // mailforge subrouter: the browser-native MUA UI (listing, message,
    // compose, send, tag, search). Lives under /mail/* and /api/* — no
    // overlap with the existing /v/* viewer routes. The mail subrouter
    // is stateless (notmuch CLI subprocesses, on-disk drafts), so it
    // merges cleanly into the stateful viewer Router. See
    // ~/Assistants/shared/mailforge-design.md for the full design.
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v/:id", get(render))
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

async fn render(
    AxPath(id): AxPath<String>,
    Query(q): Query<ViewQuery>,
    State(state): State<AppState>,
) -> Response {
    let Some(dir) = entry_dir(&state, &id) else {
        return (StatusCode::NOT_FOUND, "no such view").into_response();
    };
    let m = match manifest::read(&dir) {
        Ok(m) => m,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("manifest: {e}")).into_response(),
    };
    Html(wrapper_html(&id, &m, q.images_allowed())).into_response()
}

fn wrapper_html(id: &str, m: &Manifest, images_allowed: bool) -> String {
    // Iframe inherits the wrapper's view-state. Default is images on; only
    // pass ?images=0 explicitly when the user has chosen to block.
    let body_query = if images_allowed { "" } else { "?images=0" };
    let (subject, from, date, body) = match m {
        Manifest::Html(h) => (
            h.subject.as_deref(),
            h.from.as_deref(),
            h.date.as_deref(),
            format!(
                r#"<iframe class="body" sandbox="allow-popups allow-popups-to-escape-sandbox" tabindex="-1" src="/v/{id}/body.html{body_query}"></iframe>"#
            ),
        ),
        Manifest::Pdf(p) => (
            p.subject.as_deref(),
            p.from.as_deref(),
            p.date.as_deref(),
            format!(
                r#"<embed class="body" src="/v/{id}/raw.pdf" type="application/pdf">"#
            ),
        ),
    };
    // For HTML mode, show a small banner with the toggle. PDFs don't load
    // external images so the toggle is meaningless there. Default is
    // images-allowed (you've already vetted the sender by escalating to
    // mailforge); the toggle blocks them for sus messages.
    let images_banner = match (m, images_allowed) {
        (Manifest::Html(_), true) => format!(
            r#"<div class="img-banner">
                External images loaded.
                <form method="get" action="/v/{id}" style="display:inline;">
                  <input type="hidden" name="images" value="0">
                  <button type="submit" class="img-toggle">Block external images</button>
                </form>
            </div>"#
        ),
        (Manifest::Html(_), false) => format!(
            r#"<div class="img-banner img-banner-active">
                External images blocked for this view.
                <form method="get" action="/v/{id}" style="display:inline;">
                  <button type="submit" class="img-toggle">Load external images</button>
                </form>
            </div>"#
        ),
        _ => String::new(),
    };
    let title = subject.unwrap_or("(no subject)");
    let from_html = from
        .map(|f| format!("<span class=hdr-field>From: {}</span>", html_escape(f)))
        .unwrap_or_default();
    let date_html = date
        .map(|d| format!("<span class=hdr-field>Date: {}</span>", html_escape(d)))
        .unwrap_or_default();
    let title_html = html_escape(title);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{title_html}</title>
<style>
  :root {{
    color-scheme: light dark;
    --bg: #fafaf7; --fg: #1a1a1a; --muted: #555; --rule: #e2e2dd;
  }}
  @media (prefers-color-scheme: dark) {{
    :root {{ --bg: #16181a; --fg: #e8e6e3; --muted: #a8a8a8; --rule: #2a2c30; }}
  }}
  html, body {{ margin: 0; padding: 0; height: 100%; background: var(--bg); color: var(--fg);
    font: 14px/1.45 -apple-system, "SF Pro Text", "Segoe UI", Roboto, system-ui, sans-serif; }}
  header {{ padding: 14px 20px; border-bottom: 1px solid var(--rule); display: flex;
    flex-direction: column; gap: 4px; }}
  header h1 {{ font-size: 16px; font-weight: 600; margin: 0; }}
  .hdr-meta {{ display: flex; gap: 18px; color: var(--muted); font-size: 12px; }}
  iframe.body, embed.body {{ width: 100%; flex: 1; border: 0; background: white; }}
  body {{ display: flex; flex-direction: column; }}
  .img-banner {{ padding: 6px 20px; font-size: 12px; color: var(--muted);
    border-bottom: 1px solid var(--rule); display: flex; gap: 12px;
    align-items: center; }}
  .img-banner.img-banner-active {{ color: var(--fg); background: rgba(255,193,7,0.06); }}
  .img-toggle {{ font: inherit; padding: 2px 10px; border: 1px solid var(--rule);
    background: transparent; color: inherit; border-radius: 4px; cursor: pointer; }}
  .img-toggle:hover {{ background: var(--rule); }}
</style>
</head>
<body>
<header>
  <h1>{title_html}</h1>
  <div class="hdr-meta">{from_html}{date_html}</div>
</header>
{images_banner}
{body}
<script>
  // Backspace / Escape return to the previous view (the message list in
  // MailForge, or the referring tab from any other entry point). The
  // sandboxed iframe below is cross-origin (NULL origin from sandbox),
  // so its keydown events can't bubble up to this script — that's fine.
  // The wrapper area (header, banner, body margins) keeps keyboard focus
  // until the user clicks INTO the iframe content, at which point this
  // handler stops firing for that focus context. Clicking back on any
  // wrapper element (or pressing Tab off the iframe) restores it.
  document.addEventListener('keydown', function (e) {{
    if (e.key === 'Backspace' || e.key === 'Escape') {{
      // Don't hijack typing inside any visible input/textarea (none today,
      // but defensive against future banner inputs).
      var t = e.target;
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) {{
        return;
      }}
      e.preventDefault();
      history.back();
    }}
  }});
</script>
</body>
</html>
"#
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
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
