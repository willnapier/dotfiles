//! Axum handlers per §5.
//!
//! Validation order per v0.4 §5: path is single segment → slug regex →
//! slug contains `-` → file exists → CF-Connecting-IP present → return content.
//! Every failure returns an identical 404 (empty body, same headers); an
//! attacker probing must not be able to distinguish failure modes.

use crate::audit::{AuditEntry, AuditLog};
use crate::ratelimit::RateLimiter;
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use std::net::IpAddr;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub scroll_dir: std::path::PathBuf,
    pub rate_limiter: Arc<RateLimiter>,
    pub audit_log: Arc<AuditLog>,
}

/// Single source of the 404 response. Every "not allowed / not found" reply
/// must go through this so the bytes are identical regardless of the cause.
fn not_found_response() -> Response {
    let mut resp = (StatusCode::NOT_FOUND, "").into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CACHE_CONTROL,
        "no-store, no-cache, must-revalidate".parse().unwrap(),
    );
    resp
}

fn too_many_response() -> Response {
    let mut resp = (StatusCode::TOO_MANY_REQUESTS, "").into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CACHE_CONTROL,
        "no-store, no-cache, must-revalidate".parse().unwrap(),
    );
    resp
}

/// `^[a-z][a-z0-9-]{2,60}$` per v0.4 §5 step 2.
///
/// First char is lowercase ASCII letter; remaining 2-60 chars are lowercase
/// ASCII letters, digits, or hyphens. Total length 3-61.
fn slug_is_valid(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    if bytes.len() < 3 || bytes.len() > 61 {
        return false;
    }
    if !bytes[0].is_ascii_lowercase() {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// Parse `CF-Connecting-IP`. Returns `None` if missing or unparseable;
/// the caller treats that as "reject" per §7.
fn client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get("cf-connecting-ip")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
}

fn user_agent(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string()
}

/// Build an audit entry for a completed request.
fn audit_entry(ip: &str, ua: &str, path: &str, status: u16, bytes: usize) -> AuditEntry {
    AuditEntry {
        ts: Utc::now().to_rfc3339(),
        ip: ip.to_string(),
        ua: ua.to_string(),
        path: path.to_string(),
        status,
        bytes,
        verified: false,
    }
}

/// `GET /healthz` per §5. No auth, no rate limiting.
pub async fn healthz() -> Response {
    (StatusCode::OK, "ok\n").into_response()
}

/// `GET /<slug>` per v0.4 §5 — main scroll endpoint.
pub async fn scroll(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Response {
    let path_for_log = format!("/{slug}");
    let ua = user_agent(&headers);

    // Step 0: rate-limit BEFORE any validation; missing CF-Connecting-IP → reject.
    let ip = match client_ip(&headers) {
        Some(ip) => ip,
        None => {
            // Reject — per §7 "reject prevents bypass". We chose 404 to keep the
            // surface uniform and undiscoverable.
            log_and_return(
                &state,
                audit_entry("", &ua, &path_for_log, 404, 0),
            )
            .await;
            return not_found_response();
        }
    };
    if !state.rate_limiter.allow(ip) {
        log_and_return(
            &state,
            audit_entry(&ip.to_string(), &ua, &path_for_log, 429, 0),
        )
        .await;
        return too_many_response();
    }

    // Step 1: path shape — axum has already matched `/{slug}` (single segment).
    // Step 2: slug regex.
    if !slug_is_valid(&slug) {
        log_404(&state, &ip.to_string(), &ua, &path_for_log).await;
        return not_found_response();
    }
    // Step 3: slug contains at least one `-` (defends against trivial enumeration).
    if !slug.contains('-') {
        log_404(&state, &ip.to_string(), &ua, &path_for_log).await;
        return not_found_response();
    }
    // Step 4: file exists and is readable.
    let file_path = state.scroll_dir.join(format!("{slug}.md"));
    // Guard against path traversal — slug_is_valid already excludes `/` `.` `\`,
    // but be defensive in case anyone widens the regex later.
    if file_path.parent() != Some(state.scroll_dir.as_path()) {
        log_404(&state, &ip.to_string(), &ua, &path_for_log).await;
        return not_found_response();
    }
    let bytes = match tokio::fs::read(&file_path).await {
        Ok(b) => b,
        Err(_) => {
            log_404(&state, &ip.to_string(), &ua, &path_for_log).await;
            return not_found_response();
        }
    };

    // Step 5: serve the file as HTML.
    //
    // We wrap the markdown in a minimal HTML document and serve `text/html`
    // (not `text/plain`/`text/markdown`) because AI browse pipelines — notably
    // ChatGPT's — frequently fail to *open* a non-HTML URL and then silently
    // fall back to a web search, returning unrelated pages. Escaping the scroll
    // into a <pre> preserves the content byte-for-byte (load-bearing for the
    // figures) while presenting a page these tools will actually fetch and read.
    let text = String::from_utf8_lossy(&bytes);
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    // `slug` is already validated to `[a-z0-9-]` so it is safe to embed in the
    // <title> without further escaping.
    let html = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"robots\" content=\"noindex\">\n<title>{slug}</title>\n</head>\n<body>\n<pre>\n{escaped}\n</pre>\n</body>\n</html>\n"
    );
    let body_len = html.len();
    let mut resp = (StatusCode::OK, html).into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        "text/html; charset=utf-8".parse().unwrap(),
    );
    h.insert(
        header::CACHE_CONTROL,
        "no-store, no-cache, must-revalidate".parse().unwrap(),
    );

    let entry = audit_entry(&ip.to_string(), &ua, &path_for_log, 200, body_len);
    if let Err(e) = state.audit_log.append(&entry).await {
        eprintln!("audit append failed: {e:#}");
    }
    resp
}

/// Fallback for any path that doesn't match the two declared routes.
/// `GET /`, `GET /foo/bar`, etc. → 404 empty body.
pub async fn fallback(State(state): State<AppState>, headers: HeaderMap, req: axum::extract::Request) -> Response {
    let path = req.uri().path().to_string();
    let ua = user_agent(&headers);
    let ip_str = client_ip(&headers)
        .map(|i| i.to_string())
        .unwrap_or_default();
    log_404(&state, &ip_str, &ua, &path).await;
    not_found_response()
}

async fn log_404(state: &AppState, ip: &str, ua: &str, path: &str) {
    let entry = audit_entry(ip, ua, path, 404, 0);
    if let Err(e) = state.audit_log.append(&entry).await {
        eprintln!("audit append failed: {e:#}");
    }
}

/// Log an audit entry from a rejected request (missing CF-IP or rate-limited).
/// Caller returns its own response separately.
async fn log_and_return(state: &AppState, entry: AuditEntry) {
    if let Err(e) = state.audit_log.append(&entry).await {
        eprintln!("audit append failed: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_regex_accepts_valid() {
        assert!(slug_is_valid("financial-copper-harbour"));
        assert!(slug_is_valid("medical-lantern-meadow"));
        assert!(slug_is_valid("a-b"));
        assert!(slug_is_valid("system-overview-x-y"));
        assert!(slug_is_valid("topic1-word-2")); // digits allowed after first char
    }

    #[test]
    fn slug_regex_rejects_invalid() {
        assert!(!slug_is_valid(""));
        assert!(!slug_is_valid("ab")); // too short (<3)
        assert!(!slug_is_valid("Financial-copper-harbour")); // uppercase
        assert!(!slug_is_valid("1financial-copper")); // starts with digit
        assert!(!slug_is_valid("-leading")); // starts with hyphen
        assert!(!slug_is_valid("with space-x"));
        assert!(!slug_is_valid("with/slash-x"));
        assert!(!slug_is_valid("with.dot-x"));
        assert!(!slug_is_valid("with_underscore-x"));
        assert!(!slug_is_valid(&format!("a{}", "x".repeat(61)))); // too long (>61)
    }

    #[test]
    fn slug_must_contain_hyphen() {
        // slug_is_valid accepts single words; the hyphen check is a separate
        // step in the handler (§5 step 3).
        assert!(slug_is_valid("financial")); // passes regex
        assert!(!"financial".contains('-')); // but fails hyphen check
    }
}
