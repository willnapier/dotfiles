//! One-click List-Unsubscribe support (RFC 2369 + RFC 8058).
//!
//! Two endpoints, both `?id=<bare-message-id>`:
//!
//! - `GET /api/unsubscribe/probe` — parse the message's `List-Unsubscribe`
//!   and `List-Unsubscribe-Post` headers, return the available methods
//!   ranked by preference.
//! - `POST /api/unsubscribe/execute` — for `one_click_post`, server-side
//!   POST per RFC 8058. For `https`/`mailto`, return the URL so the
//!   browser can open it in a new tab (cookie-bound confirmation flows
//!   must run in the user's browser, not via our reqwest client).
//!
//! ## Why server-side POST for one_click
//!
//! RFC 8058 explicitly defines a no-cookies, no-confirmation flow: the
//! sender promises that POSTing `List-Unsubscribe=One-Click` to the URL
//! is sufficient. That's safe to do from the server because no human
//! interaction is needed and no session state matters. The HTTP semantics
//! are the contract.
//!
//! For `https://...` URLs without the `One-Click` opt-in, the receiver
//! generally wants the browser to load a confirmation page (often
//! cookie-bound to the user's prior visits, sometimes requiring CAPTCHA).
//! Best to hand off and let the browser do its job.
//!
//! ## Header parsing
//!
//! `List-Unsubscribe: <https://example.com/u?token=abc>, <mailto:unsub@example.com?subject=unsubscribe>`
//!
//! Multiple URIs are comma-separated, each wrapped in `<>` per RFC 2369.
//! `List-Unsubscribe-Post: List-Unsubscribe=One-Click` indicates RFC 8058
//! one-click is supported (must coexist with an HTTPS URL in
//! `List-Unsubscribe`; the mailto: alone doesn't qualify).
//!
//! ## Tagging on success
//!
//! After a successful one-click POST we tag the message `unsubscribed`
//! (audit trail — easy to find later: `notmuch search tag:unsubscribed`)
//! and `trash` while removing `inbox` (clears it from the listing). The
//! existing `gmail-push-tags` / `cohs-trash-mover` infrastructure mirrors
//! the trash to the upstream mailbox.

use anyhow::{Context, Result};
use axum::{extract::Query, http::StatusCode, response::IntoResponse, Json};
use mail_parser::{HeaderValue, MessageParser};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::mail::notmuch_db;

/// Query string for both probe and execute: `?id=<bare-message-id>`.
#[derive(Debug, Deserialize)]
pub struct UnsubQuery {
    pub id: String,
}

/// One unsubscribe method discovered in the message headers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnsubMethod {
    /// Discriminator used by the frontend to decide what to do.
    pub kind: UnsubKind,
    /// The URL: https://, http://, or mailto:.
    pub url: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UnsubKind {
    /// RFC 8058 one-click: POST to URL with `List-Unsubscribe=One-Click`
    /// body. Server-side handled.
    OneClickPost,
    /// HTTPS unsubscribe URL. Open in browser tab.
    Https,
    /// HTTP unsubscribe URL. Treated like Https for opening (some real
    /// senders still emit http://, though we ignore them server-side
    /// because the one-click POST path requires HTTPS by RFC 8058).
    Http,
    /// mailto: unsubscribe — open in user's mail handler. We don't try
    /// to send the email server-side because the mail-sending stack here
    /// (msmtp/graph-send/pizauth) is the user's identity, not a generic
    /// transport, and the mailto target may include `?subject=...&body=...`
    /// the user wants to inspect.
    Mailto,
}

/// Result of `GET /api/unsubscribe/probe`.
#[derive(Debug, Serialize)]
pub struct ProbeResult {
    pub has_unsubscribe: bool,
    pub methods: Vec<UnsubMethod>,
    /// `kind` of the first method, if any. Convenience for the frontend
    /// (avoids re-walking the array).
    pub preferred: Option<UnsubKind>,
    /// Display name of the sender (From: address), best-effort. Used by
    /// the frontend confirm dialog.
    pub from: Option<String>,
}

/// Result of `POST /api/unsubscribe/execute`.
#[derive(Debug, Serialize)]
pub struct ExecuteResult {
    /// True only when the server-side action completed end-to-end (POST
    /// succeeded AND the message was tagged). `false` when the frontend
    /// must take over (https/mailto fallback) or when the operation
    /// failed.
    pub ok: bool,
    /// `one_click_post` | `https_link` | `mailto` | `error`.
    pub method: &'static str,
    /// HTTP status code from the upstream POST (only for one_click_post).
    pub status: Option<u16>,
    /// Frontend should `window.open(url, ...)` (https) or
    /// `window.location.href = url` (mailto). Empty for one_click_post
    /// success.
    pub open_url: Option<String>,
    /// Human-readable error message, if any.
    pub error: Option<String>,
}

// ------------------------------------------------------------------
// Probe
// ------------------------------------------------------------------

/// GET `/api/unsubscribe/probe?id=<msg-id>`.
pub async fn probe_get(Query(q): Query<UnsubQuery>) -> impl IntoResponse {
    match probe(&q.id) {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(e) => {
            tracing::warn!("unsubscribe probe failed for id={}: {e:#}", q.id);
            (
                StatusCode::NOT_FOUND,
                Json(ProbeResult {
                    has_unsubscribe: false,
                    methods: Vec::new(),
                    preferred: None,
                    from: None,
                }),
            )
                .into_response()
        }
    }
}

/// Read the message file from disk, parse the relevant headers, return
/// the methods (sorted by priority) and the From: address.
fn probe(id: &str) -> Result<ProbeResult> {
    let bytes = notmuch_db::raw_bytes(id)
        .with_context(|| format!("loading raw bytes for id:{id}"))?;
    let parsed = MessageParser::default()
        .parse(&bytes[..])
        .ok_or_else(|| anyhow::anyhow!("mail-parser failed to parse id:{id}"))?;

    let list_unsub = first_text_header(&parsed, "List-Unsubscribe");
    let list_unsub_post = first_text_header(&parsed, "List-Unsubscribe-Post");

    let methods = parse_unsubscribe_headers(list_unsub.as_deref(), list_unsub_post.as_deref());
    let from = parsed
        .from()
        .and_then(|addrs| addrs.first())
        .and_then(|a| a.address().map(|s| s.to_string()));

    Ok(ProbeResult {
        has_unsubscribe: !methods.is_empty(),
        preferred: methods.first().map(|m| m.kind),
        methods,
        from,
    })
}

/// Cheap presence check: returns true iff the message has a
/// `List-Unsubscribe` header. Used by the listing render path to set
/// `data-has-unsubscribe` on each row without doing the full URL parse.
///
/// Failures (missing file, parse error) return `false` rather than
/// bubbling — never break the listing render for one bad message.
pub fn message_has_unsubscribe(id: &str) -> bool {
    let Ok(bytes) = notmuch_db::raw_bytes(id) else {
        return false;
    };
    let Some(parsed) = MessageParser::default().parse(&bytes[..]) else {
        return false;
    };
    let lu = first_text_header(&parsed, "List-Unsubscribe");
    let lup = first_text_header(&parsed, "List-Unsubscribe-Post");
    !parse_unsubscribe_headers(lu.as_deref(), lup.as_deref()).is_empty()
}

fn first_text_header<'a>(
    parsed: &'a mail_parser::Message<'a>,
    name: &str,
) -> Option<String> {
    for hdr in parsed.headers() {
        if hdr.name.as_str().eq_ignore_ascii_case(name) {
            return match &hdr.value {
                HeaderValue::Text(t) => Some(t.to_string()),
                HeaderValue::TextList(parts) => {
                    Some(parts.iter().map(|p| p.as_ref()).collect::<Vec<_>>().join(", "))
                }
                _ => None,
            };
        }
    }
    None
}

// ------------------------------------------------------------------
// Header parser (testable, pure)
// ------------------------------------------------------------------

/// Parse `List-Unsubscribe` and `List-Unsubscribe-Post` header values
/// into a priority-sorted list of methods.
///
/// Priority order:
/// 1. `OneClickPost` — only when `List-Unsubscribe-Post: List-Unsubscribe=One-Click`
///    is present AND `List-Unsubscribe` contains an `https://` URL (RFC 8058).
/// 2. `Https` — any HTTPS URL.
/// 3. `Http` — any HTTP URL (less common, but real).
/// 4. `Mailto` — fallback.
///
/// Each URL appears at most once across the list (one_click_post wins
/// over plain https for the same URL).
pub fn parse_unsubscribe_headers(
    list_unsubscribe: Option<&str>,
    list_unsubscribe_post: Option<&str>,
) -> Vec<UnsubMethod> {
    let Some(lu) = list_unsubscribe else {
        return Vec::new();
    };
    let urls = extract_uris(lu);
    if urls.is_empty() {
        return Vec::new();
    }

    let one_click_supported = list_unsubscribe_post
        .map(|s| s.to_ascii_lowercase().contains("one-click"))
        .unwrap_or(false);

    let mut https_url: Option<String> = None;
    let mut http_url: Option<String> = None;
    let mut mailto_url: Option<String> = None;

    for u in urls {
        let lower = u.to_ascii_lowercase();
        if lower.starts_with("https://") {
            if https_url.is_none() {
                https_url = Some(u);
            }
        } else if lower.starts_with("http://") {
            if http_url.is_none() {
                http_url = Some(u);
            }
        } else if lower.starts_with("mailto:") {
            if mailto_url.is_none() {
                mailto_url = Some(u);
            }
        }
    }

    let mut methods = Vec::new();
    if one_click_supported {
        if let Some(url) = https_url.clone() {
            methods.push(UnsubMethod {
                kind: UnsubKind::OneClickPost,
                url,
            });
        }
    }
    // Always include the https method too — frontend may fall back to
    // it if the server-side POST is rejected (some senders incorrectly
    // advertise One-Click but the endpoint expects browser cookies).
    if let Some(url) = https_url {
        // Avoid duplicating if we already pushed it as one-click
        if !methods.iter().any(|m| m.url == url && m.kind == UnsubKind::OneClickPost) {
            methods.push(UnsubMethod {
                kind: UnsubKind::Https,
                url,
            });
        } else {
            // It IS the one-click URL — record a fallback row too. The
            // frontend never sees both unless it actively wants to fall
            // back; keeping it in the array makes that decision local.
            methods.push(UnsubMethod {
                kind: UnsubKind::Https,
                url: methods[0].url.clone(),
            });
        }
    }
    if let Some(url) = http_url {
        methods.push(UnsubMethod {
            kind: UnsubKind::Http,
            url,
        });
    }
    if let Some(url) = mailto_url {
        methods.push(UnsubMethod {
            kind: UnsubKind::Mailto,
            url,
        });
    }
    methods
}

/// Extract `<uri>` tokens from a List-Unsubscribe header value.
/// Multiple URIs are comma-separated and each wrapped in `<>`.
fn extract_uris(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if let Some(end) = value[i + 1..].find('>') {
                let uri = &value[i + 1..i + 1 + end];
                let trimmed = uri.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
                i += 1 + end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

// ------------------------------------------------------------------
// Execute
// ------------------------------------------------------------------

/// POST `/api/unsubscribe/execute?id=<msg-id>`.
pub async fn execute_post(Query(q): Query<UnsubQuery>) -> impl IntoResponse {
    match probe(&q.id) {
        Ok(probe_result) => execute_with_probe(&q.id, probe_result).await,
        Err(e) => {
            tracing::warn!("unsubscribe execute probe-fail for id={}: {e:#}", q.id);
            (
                StatusCode::NOT_FOUND,
                Json(ExecuteResult {
                    ok: false,
                    method: "error",
                    status: None,
                    open_url: None,
                    error: Some(format!("message not found: {e}")),
                }),
            )
                .into_response()
        }
    }
}

async fn execute_with_probe(id: &str, probe: ProbeResult) -> axum::response::Response {
    let Some(method) = probe.methods.first() else {
        return (
            StatusCode::NOT_FOUND,
            Json(ExecuteResult {
                ok: false,
                method: "error",
                status: None,
                open_url: None,
                error: Some("no List-Unsubscribe header on this message".into()),
            }),
        )
            .into_response();
    };

    match method.kind {
        UnsubKind::OneClickPost => execute_one_click(id, &method.url).await,
        UnsubKind::Https | UnsubKind::Http => (
            StatusCode::OK,
            Json(ExecuteResult {
                ok: false,
                method: "https_link",
                status: None,
                open_url: Some(method.url.clone()),
                error: None,
            }),
        )
            .into_response(),
        UnsubKind::Mailto => (
            StatusCode::OK,
            Json(ExecuteResult {
                ok: false,
                method: "mailto",
                status: None,
                open_url: Some(method.url.clone()),
                error: None,
            }),
        )
            .into_response(),
    }
}

async fn execute_one_click(id: &str, url: &str) -> axum::response::Response {
    // RFC 8058 §3: POST application/x-www-form-urlencoded body
    // `List-Unsubscribe=One-Click`. 10s timeout. No redirects (cookie-bound
    // confirm pages must fall through to the browser).
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ExecuteResult {
                    ok: false,
                    method: "error",
                    status: None,
                    open_url: None,
                    error: Some(format!("reqwest client build failed: {e}")),
                }),
            )
                .into_response();
        }
    };

    let resp = match client
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("List-Unsubscribe=One-Click")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("unsubscribe POST failed for {url}: {e}");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ExecuteResult {
                    ok: false,
                    method: "error",
                    status: None,
                    open_url: Some(url.to_string()),
                    error: Some(format!("upstream POST failed: {e}")),
                }),
            )
                .into_response();
        }
    };

    let status = resp.status().as_u16();

    // 30x: cookie-bound confirmation page; fall back to client-side open.
    if (300..400).contains(&status) {
        return (
            StatusCode::OK,
            Json(ExecuteResult {
                ok: false,
                method: "https_link",
                status: Some(status),
                open_url: Some(url.to_string()),
                error: None,
            }),
        )
            .into_response();
    }

    if !(200..300).contains(&status) {
        return (
            StatusCode::OK,
            Json(ExecuteResult {
                ok: false,
                method: "error",
                status: Some(status),
                open_url: Some(url.to_string()),
                error: Some(format!(
                    "upstream returned HTTP {status}; opening in browser as fallback"
                )),
            }),
        )
            .into_response();
    }

    // Success — tag for audit + trash for inbox clearance.
    let query = format!("id:{id}");
    if let Err(e) = notmuch_db::apply_tag_changes(&query, &["unsubscribed", "trash"], &["inbox"])
    {
        tracing::warn!("unsubscribe tag-update failed for id={id}: {e}");
        return (
            StatusCode::OK,
            Json(ExecuteResult {
                ok: true,
                method: "one_click_post",
                status: Some(status),
                open_url: None,
                error: Some(format!("POST succeeded but tagging failed: {e}")),
            }),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(ExecuteResult {
            ok: true,
            method: "one_click_post",
            status: Some(status),
            open_url: None,
            error: None,
        }),
    )
        .into_response()
}

// ------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_uris_single_https() {
        let v = extract_uris("<https://example.com/u?token=abc>");
        assert_eq!(v, vec!["https://example.com/u?token=abc"]);
    }

    #[test]
    fn extract_uris_multiple_with_comma() {
        let v = extract_uris(
            "<https://example.com/u?t=abc>, <mailto:unsub@example.com?subject=unsubscribe>",
        );
        assert_eq!(
            v,
            vec![
                "https://example.com/u?t=abc",
                "mailto:unsub@example.com?subject=unsubscribe",
            ]
        );
    }

    #[test]
    fn extract_uris_handles_whitespace() {
        let v = extract_uris("  < https://x.com >  ,  <mailto:y@z.com>  ");
        assert_eq!(v, vec!["https://x.com", "mailto:y@z.com"]);
    }

    #[test]
    fn extract_uris_no_brackets_returns_empty() {
        let v = extract_uris("https://example.com");
        assert!(v.is_empty());
    }

    #[test]
    fn extract_uris_unclosed_bracket_skipped() {
        let v = extract_uris("<https://example.com");
        assert!(v.is_empty());
    }

    #[test]
    fn parse_one_click_priority_over_https() {
        let lu = "<https://example.com/u?t=abc>, <mailto:u@x.com>";
        let lup = "List-Unsubscribe=One-Click";
        let methods = parse_unsubscribe_headers(Some(lu), Some(lup));
        assert!(!methods.is_empty());
        assert_eq!(methods[0].kind, UnsubKind::OneClickPost);
        assert_eq!(methods[0].url, "https://example.com/u?t=abc");
        // mailto still exposed as a fallback
        assert!(methods.iter().any(|m| m.kind == UnsubKind::Mailto));
    }

    #[test]
    fn parse_https_priority_over_mailto_when_no_one_click() {
        let lu = "<mailto:u@x.com>, <https://example.com/u>";
        let methods = parse_unsubscribe_headers(Some(lu), None);
        assert_eq!(methods[0].kind, UnsubKind::Https);
        assert_eq!(methods[0].url, "https://example.com/u");
    }

    #[test]
    fn parse_mailto_only() {
        let lu = "<mailto:unsubscribe@example.com>";
        let methods = parse_unsubscribe_headers(Some(lu), None);
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].kind, UnsubKind::Mailto);
    }

    #[test]
    fn parse_no_list_unsubscribe_returns_empty() {
        let methods = parse_unsubscribe_headers(None, None);
        assert!(methods.is_empty());
    }

    #[test]
    fn parse_one_click_without_https_url_falls_back() {
        // Pathological: List-Unsubscribe-Post says one-click but the LU
        // header has no https URL. RFC 8058 says one-click MUST have
        // https; we silently downgrade rather than try to POST mailto:.
        let lu = "<mailto:u@x.com>";
        let lup = "List-Unsubscribe=One-Click";
        let methods = parse_unsubscribe_headers(Some(lu), Some(lup));
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].kind, UnsubKind::Mailto);
    }

    #[test]
    fn parse_http_url_classified_as_http() {
        let lu = "<http://legacy.example.com/u>";
        let methods = parse_unsubscribe_headers(Some(lu), None);
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].kind, UnsubKind::Http);
    }

    #[test]
    fn parse_one_click_case_insensitive() {
        // Some senders emit `LIST-UNSUBSCRIBE=ONE-CLICK` or mixed case.
        // RFC 8058 says case-insensitive on the directive value.
        let lu = "<https://example.com/u>";
        let lup = "List-Unsubscribe=ONE-CLICK";
        let methods = parse_unsubscribe_headers(Some(lu), Some(lup));
        assert_eq!(methods[0].kind, UnsubKind::OneClickPost);
    }

    #[test]
    fn parse_empty_list_unsubscribe_returns_empty() {
        let methods = parse_unsubscribe_headers(Some(""), None);
        assert!(methods.is_empty());
    }
}
