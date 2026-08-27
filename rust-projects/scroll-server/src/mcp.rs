//! MCP (Model Context Protocol) endpoint — Streamable HTTP transport, stateless.
//!
//! Purpose: let claude.ai reach the scrolls as a *custom connector*, so the
//! local files stay the single source of truth and nothing is ever synced,
//! uploaded or duplicated into a vendor's store. Design record:
//! ~/Assistants/shared/scroll-server-design.md.
//!
//! ## Why this is small
//!
//! The 2025-06-18 spec permits a server to answer a POSTed JSON-RPC *request*
//! with `Content-Type: application/json` instead of opening an SSE stream, and
//! makes `Mcp-Session-Id` optional ("a server ... **MAY** assign a session
//! id"). A read-only server needs neither, so there is no stream state, no
//! session table and no resumability logic — just JSON-RPC over one POST.
//! `GET` returns 405, which the spec names as the correct way to say "no
//! server-initiated stream here".
//!
//! ## Auth
//!
//! An unguessable path segment (`/mcp/<token>`), compared in constant time.
//! This is deliberately the *same* threat model already accepted for the
//! bearer-token scroll URLs: whoever knows the URL can read the scrolls.
//! Rotate by changing `SCROLL_SERVER_MCP_TOKEN` and restarting. If the env var
//! is unset the route is never registered, so the feature fails closed.
//!
//! ## Token hygiene
//!
//! Two leaks are guarded here. The path token is never written to the audit
//! log (we log `/mcp` plus the JSON-RPC method). The *scroll* slugs — which are
//! themselves bearer tokens for the browse URLs — are never exposed to the
//! model either: tools speak in public names (`financial`), and the slug
//! suffix is stripped on the way out and re-resolved on the way in.

use crate::audit::AuditEntry;
use crate::handler::{
    audit_entry, client_ip, not_found_response, too_many_response, user_agent, AppState,
};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{json, Value};

/// Protocol versions we will answer to. The newest is what we advertise from
/// `initialize`; the older two are accepted so a client that negotiated down
/// still works.
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
const PREFERRED_PROTOCOL_VERSION: &str = "2025-06-18";

const SERVER_NAME: &str = "scroll-server";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

// JSON-RPC 2.0 error codes.
const PARSE_ERROR: i32 = -32700;
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const INTERNAL_ERROR: i32 = -32603;

/// Constant-time byte comparison, so a wrong token cannot be recovered by
/// timing the 404. Length is compared first and *not* short-circuited into the
/// byte loop; leaking only the length of a 40-char random token is harmless.
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// `financial-sapphire-meadow` → `financial`; `system-overview-ribbon-marble`
/// → `system-overview`. The last two components are the random word-pair that
/// makes the browse URL unguessable, and must not reach the model.
///
/// Returns `None` for anything without at least one component left over, which
/// keeps stray files out of the listing rather than exposing them half-named.
pub(crate) fn public_name(slug: &str) -> Option<String> {
    let parts: Vec<&str> = slug.split('-').collect();
    if parts.len() < 3 {
        return None;
    }
    Some(parts[..parts.len() - 2].join("-"))
}

/// First `# ` heading in the file, used as a human-readable title in listings.
fn extract_title(text: &str) -> Option<String> {
    text.lines()
        .take(40)
        .find_map(|l| l.strip_prefix("# ").map(|t| t.trim().to_string()))
        .filter(|t| !t.is_empty())
}

/// Enumerate readable scrolls as (public_name, slug).
async fn list_slugs(dir: &std::path::Path) -> std::io::Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut rd = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = rd.next_entry().await? {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        // Skip dotfiles — `.slug-index.md` and `.word-pool.txt` are internal.
        if name.starts_with('.') {
            continue;
        }
        let Some(slug) = name.strip_suffix(".md") else {
            continue;
        };
        if !entry.file_type().await.map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if let Some(public) = public_name(slug) {
            out.push((public, slug.to_string()));
        }
    }
    out.sort();
    Ok(out)
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Wrap a text payload in the MCP `CallToolResult` shape.
fn tool_text(text: String) -> Value {
    json!({ "content": [ { "type": "text", "text": text } ], "isError": false })
}

fn tool_error(text: String) -> Value {
    json!({ "content": [ { "type": "text", "text": text } ], "isError": true })
}

fn json_response(status: StatusCode, body: Value) -> Response {
    let mut resp = (status, body.to_string()).into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        "application/json; charset=utf-8".parse().unwrap(),
    );
    h.insert(
        header::CACHE_CONTROL,
        "no-store, no-cache, must-revalidate".parse().unwrap(),
    );
    resp
}

/// The two tools we expose. Descriptions are written for the model, not for a
/// human reader — they are the only guidance it gets on when to reach for them.
fn tool_definitions() -> Value {
    json!([
        {
            "name": "list_scrolls",
            "description": "List William's available personal-context scrolls \
                            (biographical, financial, philosophical, social, dietary, \
                            lifestyle, system-overview) with titles, sizes and last-modified \
                            dates. Call this first to see what context is available, then \
                            call read_scroll for the one you need.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "read_scroll",
            "description": "Read the full text of one personal-context scroll by its name \
                            (e.g. 'financial'). Use the names returned by list_scrolls. \
                            Returns the scroll's complete current content from William's \
                            machine — always live, never a cached copy.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Scroll name as returned by list_scrolls, e.g. 'financial'."
                    }
                },
                "required": ["name"],
                "additionalProperties": false
            }
        }
    ])
}

async fn handle_list_scrolls(state: &AppState) -> Value {
    let slugs = match list_slugs(&state.scroll_dir).await {
        Ok(s) => s,
        Err(e) => return tool_error(format!("could not read scroll directory: {e}")),
    };
    if slugs.is_empty() {
        return tool_text("No scrolls are currently available.".to_string());
    }
    let mut lines = Vec::new();
    for (public, slug) in slugs {
        let path = state.scroll_dir.join(format!("{slug}.md"));
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        let text = String::from_utf8_lossy(&bytes);
        let title = extract_title(&text).unwrap_or_else(|| public.clone());
        let modified = tokio::fs::metadata(&path)
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .map(|t| {
                let dt: chrono::DateTime<chrono::Utc> = t.into();
                dt.format("%Y-%m-%d").to_string()
            })
            .unwrap_or_else(|| "unknown".to_string());
        lines.push(format!(
            "- {public} — \"{title}\" ({} KB, updated {modified})",
            bytes.len() / 1024
        ));
    }
    tool_text(format!(
        "Available scrolls (call read_scroll with the name before the em-dash):\n\n{}",
        lines.join("\n")
    ))
}

async fn handle_read_scroll(state: &AppState, args: &Value) -> Value {
    let Some(requested) = args.get("name").and_then(|v| v.as_str()) else {
        return tool_error("read_scroll requires a 'name' argument.".to_string());
    };
    let requested = requested.trim().to_ascii_lowercase();

    let slugs = match list_slugs(&state.scroll_dir).await {
        Ok(s) => s,
        Err(e) => return tool_error(format!("could not read scroll directory: {e}")),
    };
    // Resolve public name → slug. Never trust the input as a path component:
    // it is only ever compared against names we generated ourselves.
    let Some((_, slug)) = slugs.iter().find(|(public, _)| *public == requested) else {
        let available: Vec<&str> = slugs.iter().map(|(p, _)| p.as_str()).collect();
        return tool_error(format!(
            "No scroll named '{requested}'. Available: {}",
            available.join(", ")
        ));
    };

    let path = state.scroll_dir.join(format!("{slug}.md"));
    // Defence in depth: `slug` came from read_dir, but re-assert containment in
    // case the naming rules are ever widened.
    if path.parent() != Some(state.scroll_dir.as_path()) {
        return tool_error("scroll path rejected".to_string());
    }
    match tokio::fs::read(&path).await {
        Ok(bytes) => tool_text(String::from_utf8_lossy(&bytes).to_string()),
        Err(e) => tool_error(format!("could not read scroll '{requested}': {e}")),
    }
}

/// `POST /mcp/{token}` — the MCP endpoint.
pub async fn mcp_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
    body: Bytes,
) -> Response {
    let ua = user_agent(&headers);

    // Step 0: same front door as the scroll route — CF-Connecting-IP required,
    // then rate limit. A direct localhost hit has no such header and 404s.
    let ip = match client_ip(&headers) {
        Some(ip) => ip,
        None => {
            log_mcp(&state, "", &ua, "unauthenticated", 404, 0).await;
            return not_found_response();
        }
    };
    if !state.rate_limiter.allow(ip) {
        log_mcp(&state, &ip.to_string(), &ua, "ratelimited", 429, 0).await;
        return too_many_response();
    }

    // Step 1: token. Uniform 404 on failure — indistinguishable from a bad path.
    let expected = match state.mcp_token.as_deref() {
        Some(t) => t,
        None => {
            log_mcp(&state, &ip.to_string(), &ua, "disabled", 404, 0).await;
            return not_found_response();
        }
    };
    if !ct_eq(&token, expected) {
        log_mcp(&state, &ip.to_string(), &ua, "bad-token", 404, 0).await;
        return not_found_response();
    }

    // Step 2: Origin. The spec requires validating it to blunt DNS rebinding.
    // Anthropic's server-side fetcher sends none; a browser would. Absent is
    // allowed, claude.ai is allowed, anything else is refused.
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|h| h.to_str().ok()) {
        let ok = origin == "https://claude.ai"
            || origin == "https://www.claude.ai"
            || origin.ends_with(".anthropic.com");
        if !ok {
            log_mcp(&state, &ip.to_string(), &ua, "bad-origin", 404, 0).await;
            return not_found_response();
        }
    }

    // Step 3: protocol version. Spec: an unsupported value MUST be 400. An
    // absent header means "assume 2025-03-26", which we support.
    if let Some(v) = headers.get("mcp-protocol-version").and_then(|h| h.to_str().ok()) {
        if !SUPPORTED_PROTOCOL_VERSIONS.contains(&v) {
            log_mcp(&state, &ip.to_string(), &ua, "bad-version", 400, 0).await;
            return json_response(
                StatusCode::BAD_REQUEST,
                rpc_error(Value::Null, INVALID_REQUEST, "unsupported MCP-Protocol-Version"),
            );
        }
    }

    // Step 4: parse.
    let msg: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            log_mcp(&state, &ip.to_string(), &ua, "parse-error", 400, 0).await;
            return json_response(
                StatusCode::BAD_REQUEST,
                rpc_error(Value::Null, PARSE_ERROR, "invalid JSON"),
            );
        }
    };

    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    // Absent `id` ⇒ notification. Spec: accept with 202 and an empty body.
    let id = msg.get("id").cloned();
    let Some(id) = id else {
        log_mcp(&state, &ip.to_string(), &ua, method, 202, 0).await;
        return StatusCode::ACCEPTED.into_response();
    };

    let (status, payload) = match method {
        "initialize" => (
            StatusCode::OK,
            rpc_result(
                id,
                json!({
                    "protocolVersion": PREFERRED_PROTOCOL_VERSION,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
                    "instructions": "William's personal-context scrolls, served live from \
                                     his own machine. Call list_scrolls to see what is \
                                     available, then read_scroll to load one."
                }),
            ),
        ),
        "ping" => (StatusCode::OK, rpc_result(id, json!({}))),
        "tools/list" => (
            StatusCode::OK,
            rpc_result(id, json!({ "tools": tool_definitions() })),
        ),
        "tools/call" => {
            let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            match name {
                "list_scrolls" => (
                    StatusCode::OK,
                    rpc_result(id, handle_list_scrolls(&state).await),
                ),
                "read_scroll" => (
                    StatusCode::OK,
                    rpc_result(id, handle_read_scroll(&state, &args).await),
                ),
                "" => (
                    StatusCode::OK,
                    rpc_error(id, INVALID_PARAMS, "tools/call requires a tool name"),
                ),
                other => (
                    StatusCode::OK,
                    rpc_error(id, METHOD_NOT_FOUND, &format!("unknown tool: {other}")),
                ),
            }
        }
        // Declared-but-empty capabilities: answer politely rather than erroring,
        // so a client that probes them does not treat us as broken.
        "resources/list" => (StatusCode::OK, rpc_result(id, json!({ "resources": [] }))),
        "prompts/list" => (StatusCode::OK, rpc_result(id, json!({ "prompts": [] }))),
        "" => (
            StatusCode::OK,
            rpc_error(id, INVALID_REQUEST, "missing method"),
        ),
        other => (
            StatusCode::OK,
            rpc_error(id, METHOD_NOT_FOUND, &format!("unknown method: {other}")),
        ),
    };

    let body_len = payload.to_string().len();
    log_mcp(
        &state,
        &ip.to_string(),
        &ua,
        &describe(method, &msg),
        status.as_u16(),
        body_len,
    )
    .await;
    let _ = INTERNAL_ERROR; // reserved for future failure paths
    json_response(status, payload)
}

/// `GET /mcp/{token}` — we offer no server-initiated SSE stream. The spec names
/// 405 as the correct answer, and clients treat it as "POST-only server".
pub async fn mcp_get() -> Response {
    let mut resp = StatusCode::METHOD_NOT_ALLOWED.into_response();
    resp.headers_mut()
        .insert(header::ALLOW, "POST".parse().unwrap());
    resp
}

/// Audit line for an MCP request. The path token is deliberately **not**
/// recorded — `/mcp` plus the JSON-RPC method is enough to reconstruct usage
/// without putting the credential in a log file that syncs.
async fn log_mcp(state: &AppState, ip: &str, ua: &str, what: &str, status: u16, bytes: usize) {
    let entry: AuditEntry = audit_entry(ip, ua, &format!("/mcp:{what}"), status, bytes);
    if let Err(e) = state.audit_log.append(&entry).await {
        eprintln!("audit append failed: {e:#}");
    }
}

/// `tools/call` is the only method whose *argument* is worth auditing, since it
/// records which scroll was actually read.
fn describe(method: &str, msg: &Value) -> String {
    if method == "tools/call" {
        if let Some(name) = msg.pointer("/params/name").and_then(|v| v.as_str()) {
            if let Some(scroll) = msg.pointer("/params/arguments/name").and_then(|v| v.as_str()) {
                return format!("{method}:{name}:{scroll}");
            }
            return format!("{method}:{name}");
        }
    }
    method.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_matches_and_rejects() {
        assert!(ct_eq("abc123", "abc123"));
        assert!(!ct_eq("abc123", "abc124"));
        assert!(!ct_eq("abc123", "abc1234"));
        assert!(!ct_eq("", "x"));
        assert!(ct_eq("", ""));
    }

    #[test]
    fn public_name_strips_the_word_pair() {
        assert_eq!(public_name("financial-sapphire-meadow").as_deref(), Some("financial"));
        assert_eq!(public_name("biographical-lantern-river").as_deref(), Some("biographical"));
        assert_eq!(
            public_name("system-overview-ribbon-marble").as_deref(),
            Some("system-overview")
        );
        assert_eq!(public_name("philosophical-harbour-compass").as_deref(), Some("philosophical"));
    }

    #[test]
    fn public_name_rejects_too_few_components() {
        assert_eq!(public_name("financial"), None);
        assert_eq!(public_name("financial-sapphire"), None);
    }

    #[test]
    fn title_comes_from_first_h1() {
        assert_eq!(
            extract_title("---\nx: 1\n---\n\n# Financial Context\n\nbody").as_deref(),
            Some("Financial Context")
        );
        assert_eq!(extract_title("no heading here"), None);
        assert_eq!(extract_title("## not h1"), None);
    }

    #[test]
    fn describe_records_which_scroll_was_read() {
        let msg = json!({
            "method": "tools/call",
            "params": { "name": "read_scroll", "arguments": { "name": "financial" } }
        });
        assert_eq!(describe("tools/call", &msg), "tools/call:read_scroll:financial");
        let msg2 = json!({ "method": "tools/call", "params": { "name": "list_scrolls" } });
        assert_eq!(describe("tools/call", &msg2), "tools/call:list_scrolls");
        assert_eq!(describe("initialize", &json!({})), "initialize");
    }
}
