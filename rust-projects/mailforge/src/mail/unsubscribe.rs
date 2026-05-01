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
use mail_parser::MessageParser;
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
#[derive(Debug, Serialize, Default)]
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
    /// Sender's email address (display-name stripped). Populated for
    /// successful unsubscribes; drives the post-unsub "delete N existing
    /// messages from this sender?" follow-up prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_address: Option<String>,
    /// Number of NON-TRASHED messages still on disk from the same sender
    /// (excluding the one just trashed by this unsubscribe). Frontend
    /// shows this in the "scorched earth" prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_message_count: Option<u64>,
}

/// Extract a clean `local@domain` address from a From: header value.
/// Handles bracketed (`Joe Bloggs <joe@example.com>`) and bare (`joe@example.com`)
/// forms. Returns the trimmed input if no `<...>` is found, which works
/// fine when the input was already an address.
fn extract_sender_address(from: &str) -> String {
    if let Some(start) = from.rfind('<') {
        if let Some(end_offset) = from[start..].find('>') {
            return from[start + 1..start + end_offset].trim().to_string();
        }
    }
    from.trim().to_string()
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
///
/// Why we read raw header bytes ourselves instead of using mail-parser's
/// `list_unsubscribe()` accessor: mail-parser routes `List-Unsubscribe`
/// through its address parser (because RFC 2369 piggy-backs on RFC 5322
/// address syntax for the `<URL>` shape). Real-world senders emit
/// `List-Unsubscribe: <https://...>` with multiple URIs, query strings,
/// and `;` separators that don't always survive a strict address parse.
/// Parsing the raw header bytes ourselves sidesteps the mismatch and
/// keeps the extractor logic in one place (parse_unsubscribe_headers,
/// which is unit-tested without touching mail-parser).
fn probe(id: &str) -> Result<ProbeResult> {
    let bytes = notmuch_db::raw_bytes(id)
        .with_context(|| format!("loading raw bytes for id:{id}"))?;

    let header_block = extract_header_block(&bytes);
    let list_unsub = read_unfolded_header(header_block, "List-Unsubscribe");
    let list_unsub_post = read_unfolded_header(header_block, "List-Unsubscribe-Post");

    let methods = parse_unsubscribe_headers(list_unsub.as_deref(), list_unsub_post.as_deref());

    // From: still goes through mail-parser — that header is straightforward
    // address syntax and we need it for the confirm-dialog label.
    let from = MessageParser::default()
        .parse(&bytes[..])
        .and_then(|parsed| {
            parsed
                .from()
                .and_then(|addrs| addrs.first())
                .and_then(|a| a.address().map(|s| s.to_string()))
        });

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
///
/// Single-message convenience wrapper around the batch path. For a
/// 50-row listing render, prefer [`batch_check_unsubscribe`] — one
/// notmuch subprocess instead of 50.
pub fn message_has_unsubscribe(id: &str) -> bool {
    let Ok(bytes) = notmuch_db::raw_bytes(id) else {
        return false;
    };
    file_bytes_have_unsubscribe(&bytes)
}

/// Batched List-Unsubscribe presence check across multiple ids.
///
/// Single notmuch subprocess: `notmuch search --output=files
/// id:a or id:b or ...` produces one path per matching message file.
/// We then read each file synchronously and parse headers locally.
/// Total cost: 1 notmuch fork (~50ms) + N file reads (microseconds
/// each) + N header parses (microseconds each). For a 50-message page
/// this brings the pre-load step under 100ms.
///
/// Returns a Vec<bool> aligned 1:1 with the input slice. Ids missing
/// from the file map (e.g. notmuch index drift) get `false` rather
/// than panicking.
pub fn batch_check_unsubscribe(ids: &[String]) -> Vec<bool> {
    if ids.is_empty() {
        return Vec::new();
    }

    // Build the notmuch query: id:a or id:b or ...
    let query = notmuch_db::ids_to_query(ids);
    let id_to_path = match notmuch_files_by_id(&query, ids) {
        Ok(map) => map,
        Err(e) => {
            tracing::warn!("batch_check_unsubscribe: file lookup failed: {e:#}");
            // Fall back to per-message lookup so we still produce the
            // right shape, even if it's slower.
            return ids.iter().map(|id| message_has_unsubscribe(id)).collect();
        }
    };

    ids.iter()
        .map(|id| {
            let Some(path) = id_to_path.get(id.as_str()) else {
                return false;
            };
            let Ok(bytes) = std::fs::read(path) else {
                return false;
            };
            file_bytes_have_unsubscribe(&bytes)
        })
        .collect()
}

/// Pure helper: takes raw RFC822 bytes, returns whether the message
/// has a usable List-Unsubscribe header. Split out for unit testing
/// without a notmuch DB.
fn file_bytes_have_unsubscribe(bytes: &[u8]) -> bool {
    let header_block = extract_header_block(bytes);
    let lu = read_unfolded_header(header_block, "List-Unsubscribe");
    let lup = read_unfolded_header(header_block, "List-Unsubscribe-Post");
    !parse_unsubscribe_headers(lu.as_deref(), lup.as_deref()).is_empty()
}

/// Single notmuch call to map a query to per-message file paths.
///
/// Returns a HashMap<id, path>. Notmuch's `--output=files` mode prints
/// one path per matching file (for messages with multiple backing files
/// — e.g. Gmail All Mail + label folders — only the first per id is
/// kept; we only need ANY readable file).
///
/// We could ask notmuch for `--output=messages` to get the id list and
/// then `--output=files` separately, but that's two forks and the
/// alignment between them is fragile. Instead we use the existing
/// `--format=json` approach to get id→path mapping in one shot.
fn notmuch_files_by_id(
    query: &str,
    expected_ids: &[String],
) -> Result<std::collections::HashMap<String, std::path::PathBuf>> {
    use std::process::{Command, Stdio};

    // `notmuch search --format=json --output=files` returns a JSON array
    // of file path strings (no per-id mapping). We need the mapping, so
    // we ask for `--output=summary` to get the id list AND then walk
    // each id's files via the existing `notmuch_db::raw_bytes` path...
    // but that's N forks again.
    //
    // Cheaper: use `notmuch show --format=json` which returns one
    // JSON record per matching message including filename + headers.
    // For our purposes we just want id+filename pairs.
    let output = Command::new("notmuch")
        .args(["show", "--format=json", "--body=false", "--entire-thread=false", query])
        .stderr(Stdio::null())
        .output()
        .with_context(|| format!("notmuch show {query}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "notmuch show failed (exit {:?})",
            output.status.code()
        );
    }

    // notmuch's show output is a tree-of-arrays: top-level array of
    // threads, each thread is an array of message-arrays. We don't
    // need the structure — we walk the whole JSON and pick out objects
    // that have an `id` key and a `filename` key.
    let v: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("parsing notmuch show JSON")?;

    let mut map: std::collections::HashMap<String, std::path::PathBuf> =
        std::collections::HashMap::with_capacity(expected_ids.len());
    walk_show_tree(&v, &mut map);
    Ok(map)
}

/// Recursively walk notmuch show's tree-of-arrays, collecting
/// (id, filename) pairs into the map.
fn walk_show_tree(
    v: &serde_json::Value,
    map: &mut std::collections::HashMap<String, std::path::PathBuf>,
) {
    match v {
        serde_json::Value::Array(arr) => {
            for item in arr {
                walk_show_tree(item, map);
            }
        }
        serde_json::Value::Object(obj) => {
            // Notmuch message records carry "id" + "filename" (the
            // latter sometimes a string, sometimes an array). We pick
            // the first existing file.
            let id = obj.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
            let path = match obj.get("filename") {
                Some(serde_json::Value::String(s)) => Some(std::path::PathBuf::from(s)),
                Some(serde_json::Value::Array(arr)) => arr
                    .iter()
                    .filter_map(|x| x.as_str())
                    .map(std::path::PathBuf::from)
                    .find(|p| p.exists()),
                _ => None,
            };
            if let (Some(id), Some(path)) = (id, path) {
                map.entry(id).or_insert(path);
            }
            // Recurse into other fields too — some shapes nest further.
            for (_, child) in obj {
                walk_show_tree(child, map);
            }
        }
        _ => {}
    }
}

/// Slice off the header block: the bytes up to (but not including) the
/// first blank line (CRLF/CRLF or LF/LF). Falls back to the whole file
/// if no blank line is found (defensive — shouldn't happen on real mail).
fn extract_header_block(bytes: &[u8]) -> &[u8] {
    // Look for "\r\n\r\n" first, then "\n\n".
    if let Some(pos) = find_subsequence(bytes, b"\r\n\r\n") {
        return &bytes[..pos];
    }
    if let Some(pos) = find_subsequence(bytes, b"\n\n") {
        return &bytes[..pos];
    }
    bytes
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

/// Read a header value, handling RFC 5322 line folding (continuation
/// lines start with whitespace). Returns the value with leading and
/// trailing whitespace stripped, internal CRLF/LF replaced with a
/// single space. Case-insensitive on the header name.
///
/// Returns the FIRST matching header. If a message has multiple
/// `List-Unsubscribe` headers (rare, but RFC 2369 doesn't forbid it)
/// only the first is honoured — matches what mail clients tend to do.
fn read_unfolded_header(header_block: &[u8], name: &str) -> Option<String> {
    let text = std::str::from_utf8(header_block).ok()?;
    let lower_name = name.to_ascii_lowercase();
    let prefix = format!("{lower_name}:");

    let mut lines = text.split('\n').peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_end_matches('\r');
        // Header start: name followed by colon.
        let line_lc_prefix = trimmed
            .splitn(2, ':')
            .next()
            .map(|s| s.trim().to_ascii_lowercase());
        let matches = line_lc_prefix
            .map(|s| {
                let target = lower_name.as_str();
                s == target
            })
            .unwrap_or(false);
        if !matches {
            continue;
        }
        // Found header start. Strip "Name:" then collect any
        // continuation lines (start with space or tab).
        let after_colon = trimmed
            .splitn(2, ':')
            .nth(1)
            .unwrap_or("")
            .trim_start();
        let mut value = String::from(after_colon);
        while let Some(&peek) = lines.peek() {
            let peek_trimmed = peek.trim_end_matches('\r');
            if peek_trimmed.starts_with(' ') || peek_trimmed.starts_with('\t') {
                value.push(' ');
                value.push_str(peek_trimmed.trim());
                lines.next();
            } else {
                break;
            }
        }
        // Sanity: prefix must match (defensive vs `splitn` partials).
        let _ = prefix;
        return Some(value.trim().to_string());
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
            }),
        )
            .into_response();
    }

    // Compute sender address + remaining-message count for the post-unsub
    // "delete N existing from this sender" follow-up prompt. Best-effort:
    // if the From: header doesn't parse, leave the fields None and the
    // frontend skips the prompt silently.
    let (sender_address, sender_message_count) = match notmuch_db::show(id) {
        Ok(msg) => {
            let from = msg.from.as_deref().unwrap_or_default();
            let address = extract_sender_address(from);
            if address.is_empty() {
                (None, None)
            } else {
                let count_q = format!("from:{address} and not tag:trash");
                let cnt = notmuch_db::count(&count_q).ok();
                (Some(address), cnt)
            }
        }
        Err(_) => (None, None),
    };

    (
        StatusCode::OK,
        Json(ExecuteResult {
            ok: true,
            method: "one_click_post",
            status: Some(status),
            open_url: None,
            error: None,
            sender_address,
            sender_message_count,
        }),
    )
        .into_response()
}

// ------------------------------------------------------------------
// Trash-all-from-sender (scorched earth follow-up)
// ------------------------------------------------------------------

/// Result of `POST /api/unsubscribe/trash-from-sender`.
#[derive(Debug, Serialize)]
pub struct TrashFromSenderResult {
    pub ok: bool,
    pub sender: Option<String>,
    pub count: u64,
    pub error: Option<String>,
}

/// POST `/api/unsubscribe/trash-from-sender?id=<msg-id>`. Tags every
/// non-trashed notmuch message whose From: header matches the same
/// sender as the given message. Optional follow-up to a successful
/// unsubscribe — clears historical clutter from a sender you've just
/// stopped accepting.
pub async fn trash_from_sender_post(Query(q): Query<UnsubQuery>) -> impl IntoResponse {
    let msg = match notmuch_db::show(&q.id) {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                Json(TrashFromSenderResult {
                    ok: false,
                    sender: None,
                    count: 0,
                    error: Some(format!("show failed: {e}")),
                }),
            )
                .into_response();
        }
    };
    let from = msg.from.as_deref().unwrap_or_default();
    let address = extract_sender_address(from);
    if address.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(TrashFromSenderResult {
                ok: false,
                sender: None,
                count: 0,
                error: Some("no parseable sender address".into()),
            }),
        )
            .into_response();
    }
    let query = format!("from:{address} and not tag:trash");
    let count = notmuch_db::count(&query).unwrap_or(0);
    if count > 0 {
        if let Err(e) = notmuch_db::apply_tag_changes(&query, &["trash"], &["inbox"]) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(TrashFromSenderResult {
                    ok: false,
                    sender: Some(address),
                    count: 0,
                    error: Some(format!("tag-update failed: {e}")),
                }),
            )
                .into_response();
        }
    }
    (
        StatusCode::OK,
        Json(TrashFromSenderResult {
            ok: true,
            sender: Some(address),
            count,
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

    // ---------- Raw header reader ----------

    #[test]
    fn read_unfolded_header_simple() {
        let block = b"From: a@b\r\nList-Unsubscribe: <https://x.com/u>\r\nDate: now\r\n";
        let got = read_unfolded_header(block, "List-Unsubscribe");
        assert_eq!(got.as_deref(), Some("<https://x.com/u>"));
    }

    #[test]
    fn read_unfolded_header_case_insensitive_name() {
        let block = b"LIST-UNSUBSCRIBE: <https://x.com/u>\r\n";
        let got = read_unfolded_header(block, "List-Unsubscribe");
        assert_eq!(got.as_deref(), Some("<https://x.com/u>"));
    }

    #[test]
    fn read_unfolded_header_handles_folding() {
        // RFC 5322 line folding: continuation lines start with whitespace.
        let block = b"List-Unsubscribe: <https://x.com/u?token=abc>,\r\n  <mailto:u@x.com>\r\nDate: now\r\n";
        let got = read_unfolded_header(block, "List-Unsubscribe");
        assert_eq!(
            got.as_deref(),
            Some("<https://x.com/u?token=abc>, <mailto:u@x.com>")
        );
    }

    #[test]
    fn read_unfolded_header_missing_returns_none() {
        let block = b"From: a@b\r\nDate: now\r\n";
        assert!(read_unfolded_header(block, "List-Unsubscribe").is_none());
    }

    #[test]
    fn read_unfolded_header_lf_only() {
        // Some senders emit LF-only line endings.
        let block = b"From: a@b\nList-Unsubscribe: <https://x.com>\nDate: now\n";
        let got = read_unfolded_header(block, "List-Unsubscribe");
        assert_eq!(got.as_deref(), Some("<https://x.com>"));
    }

    #[test]
    fn extract_header_block_strips_body() {
        let bytes = b"From: a@b\r\nDate: now\r\n\r\nbody body body";
        let block = extract_header_block(bytes);
        assert_eq!(block, b"From: a@b\r\nDate: now");
    }

    #[test]
    fn extract_header_block_lf_only_separator() {
        let bytes = b"From: a@b\nDate: now\n\nbody";
        let block = extract_header_block(bytes);
        assert_eq!(block, b"From: a@b\nDate: now");
    }

    #[test]
    fn file_bytes_have_unsubscribe_true_for_normal_message() {
        let bytes = b"From: a@b\r\n\
            List-Unsubscribe: <https://x.com/u>\r\n\
            \r\n\
            body";
        assert!(file_bytes_have_unsubscribe(bytes));
    }

    #[test]
    fn file_bytes_have_unsubscribe_false_for_message_without_header() {
        let bytes = b"From: a@b\r\nDate: now\r\n\r\nbody";
        assert!(!file_bytes_have_unsubscribe(bytes));
    }

    #[test]
    fn full_path_real_world_amazonses_message() {
        // Real-world shape from the user's promo inbox: SES's
        // List-Unsubscribe carries query string with `?` and `&`,
        // followed by a one-click directive on the next header.
        // Verifies the raw-header-bytes path produces the expected
        // method list.
        let bytes = b"From: cfas@isst-d.org\r\n\
            List-Unsubscribe: <https://polo.feathr.co/email_preferences?a_id=667c33c9edf4576be4607e62&project_id=6690255f1dc274a795bdeb97&cpn_id=69f3b13ce9cf74155d1d8277&email_addr=will@willnapier.com&per_id=66cf334ee9dc10c01c737e5b>\r\n\
            List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n\
            Date: now\r\n\
            \r\n\
            body";
        let block = extract_header_block(bytes);
        let lu = read_unfolded_header(block, "List-Unsubscribe").expect("LU present");
        let lup = read_unfolded_header(block, "List-Unsubscribe-Post").expect("LUP present");
        let methods = parse_unsubscribe_headers(Some(&lu), Some(&lup));
        assert!(!methods.is_empty(), "must detect at least one method");
        assert_eq!(methods[0].kind, UnsubKind::OneClickPost);
        assert!(methods[0].url.starts_with("https://polo.feathr.co/"));
    }
}
