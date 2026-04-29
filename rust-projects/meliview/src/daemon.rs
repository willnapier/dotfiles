use anyhow::{Context, Result};
use axum::{
    Router,
    body::Body,
    extract::{Path as AxPath, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

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

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v/:id", get(render))
        .route("/v/:id/body.html", get(serve_body))
        .route("/v/:id/raw.pdf", get(serve_pdf))
        .route("/v/:id/cid/:filename", get(serve_asset))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    tracing::info!("meliview listening on http://{addr}");
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
    State(state): State<AppState>,
) -> Response {
    let Some(dir) = entry_dir(&state, &id) else {
        return (StatusCode::NOT_FOUND, "no such view").into_response();
    };
    let m = match manifest::read(&dir) {
        Ok(m) => m,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("manifest: {e}")).into_response(),
    };
    Html(wrapper_html(&id, &m)).into_response()
}

fn wrapper_html(id: &str, m: &Manifest) -> String {
    let (subject, from, date, body) = match m {
        Manifest::Html(h) => (
            h.subject.as_deref(),
            h.from.as_deref(),
            h.date.as_deref(),
            format!(
                r#"<iframe class="body" sandbox="" src="/v/{id}/body.html"></iframe>"#
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
</style>
</head>
<body>
<header>
  <h1>{title_html}</h1>
  <div class="hdr-meta">{from_html}{date_html}</div>
</header>
{body}
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
    State(state): State<AppState>,
) -> Response {
    let Some(dir) = entry_dir(&state, &id) else {
        return (StatusCode::NOT_FOUND, "no such view").into_response();
    };
    serve_file(&dir.join("body.html"), "text/html; charset=utf-8")
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
    match std::fs::read(path) {
        Ok(bytes) => {
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_TYPE, content_type.parse().unwrap());
            (headers, Body::from(bytes)).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "file not found").into_response(),
    }
}
