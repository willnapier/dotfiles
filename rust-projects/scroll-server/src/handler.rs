//! Axum handlers per §5.
//!
//! Validation order per §5: path shape → topic regex → word in list → word matches
//! today → file exists. Every failure returns an identical 404 (empty body, same
//! headers); an attacker probing must not be able to distinguish failure modes.

use crate::audit::{AuditEntry, AuditLog};
use crate::day_word::{day_word, words_match};
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
    pub seed: Arc<Vec<u8>>,
    pub word_list: Arc<Vec<String>>,
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

/// `^[a-z][a-z-]{0,30}$` per §5 step 2.
fn topic_is_valid(topic: &str) -> bool {
    let bytes = topic.as_bytes();
    if bytes.is_empty() || bytes.len() > 31 {
        return false;
    }
    if !(bytes[0].is_ascii_lowercase()) {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|b| b.is_ascii_lowercase() || *b == b'-')
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

/// `GET /<topic>/<word>` per §5 — main scroll endpoint.
pub async fn scroll(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((topic, word)): Path<(String, String)>,
) -> Response {
    let path_for_log = format!("/{topic}/{word}");
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
                not_found_response(),
            )
            .await;
            return not_found_response();
        }
    };
    if !state.rate_limiter.allow(ip) {
        let resp = too_many_response();
        log_and_return(
            &state,
            audit_entry(&ip.to_string(), &ua, &path_for_log, 429, 0),
            resp,
        )
        .await;
        return too_many_response();
    }

    // Step 1: path shape — axum has already matched `/{topic}/{word}`.
    // Step 2: topic regex.
    if !topic_is_valid(&topic) {
        log_404(&state, &ip.to_string(), &ua, &path_for_log).await;
        return not_found_response();
    }
    // Step 3: word is in WORD_LIST.
    if !state.word_list.iter().any(|w| words_match(w, &word)) {
        log_404(&state, &ip.to_string(), &ua, &path_for_log).await;
        return not_found_response();
    }
    // Step 4: word matches today's.
    let today = day_word(&state.seed, &state.word_list, Utc::now());
    if !words_match(&today, &word) {
        log_404(&state, &ip.to_string(), &ua, &path_for_log).await;
        return not_found_response();
    }
    // Step 5: file exists and is readable.
    let file_path = state.scroll_dir.join(format!("{topic}.md"));
    // Guard against path traversal — topic_is_valid already excludes `/` `.` `\`,
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

    // Step 6: serve the file.
    let body_len = bytes.len();
    let mut resp = (StatusCode::OK, bytes).into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        "text/markdown; charset=utf-8".parse().unwrap(),
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
/// `GET /`, `GET /<topic>` (no word), `GET /foo/bar/baz`, etc. → 404 empty body.
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

/// Helper used by the missing-CF-IP branch: log + return the resp (caller drops
/// our return value and returns its own copy so the borrow checker is happy).
async fn log_and_return(state: &AppState, entry: AuditEntry, _resp: Response) {
    if let Err(e) = state.audit_log.append(&entry).await {
        eprintln!("audit append failed: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_regex_accepts_valid() {
        assert!(topic_is_valid("financial"));
        assert!(topic_is_valid("medical"));
        assert!(topic_is_valid("a-b-c"));
        assert!(topic_is_valid("a"));
    }

    #[test]
    fn topic_regex_rejects_invalid() {
        assert!(!topic_is_valid(""));
        assert!(!topic_is_valid("Financial")); // uppercase
        assert!(!topic_is_valid("123")); // starts with digit
        assert!(!topic_is_valid("-leading")); // starts with hyphen
        assert!(!topic_is_valid("with space"));
        assert!(!topic_is_valid("with/slash"));
        assert!(!topic_is_valid("with.dot"));
        assert!(!topic_is_valid("with_underscore"));
        assert!(!topic_is_valid(&"x".repeat(32))); // too long
    }
}
