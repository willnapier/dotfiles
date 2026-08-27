//! End-to-end tests for the MCP endpoint.
//!
//! Mirrors tests/integration.rs: a real axum server on 127.0.0.1:0 over a
//! tempdir of synthetic slug-named scrolls. Every request carries a
//! `CF-Connecting-IP` header because the server rejects anything without one
//! (see handler.rs §7) — omitting it is the documented way to misread a
//! healthy server as broken.

use scroll_server::audit::AuditLog;
use scroll_server::build_app;
use scroll_server::config::Config;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;

const TOKEN: &str = "testtokentesttokentesttoken0123456789";
const FINANCIAL_SLUG: &str = "financial-copper-harbour";
const MEDICAL_SLUG: &str = "medical-lantern-meadow";

struct TestServer {
    addr: SocketAddr,
    audit_path: std::path::PathBuf,
    _tempdir: tempfile::TempDir,
}

async fn start_server(token: Option<&str>) -> TestServer {
    let tmp = tempfile::tempdir().expect("tempdir");
    let scroll_dir = tmp.path().join("scrolls");
    std::fs::create_dir_all(&scroll_dir).unwrap();
    std::fs::write(
        scroll_dir.join(format!("{FINANCIAL_SLUG}.md")),
        b"# Financial Context\n\nthe body.\n",
    )
    .unwrap();
    std::fs::write(
        scroll_dir.join(format!("{MEDICAL_SLUG}.md")),
        b"# Medical Context\n\nother body.\n",
    )
    .unwrap();
    // Internal files that must never appear in a listing.
    std::fs::write(scroll_dir.join(".slug-index.md"), b"secret index\n").unwrap();
    std::fs::write(scroll_dir.join(".word-pool.txt"), b"words\n").unwrap();

    let audit_path = tmp.path().join("audit.log");
    let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let cfg = Config {
        bind,
        scroll_dir,
        audit_log: audit_path.clone(),
        mcp_token: token.map(|t| t.to_string()),
    };

    let audit = Arc::new(AuditLog::new(audit_path.clone()));
    let app = build_app(&cfg, audit);
    let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    TestServer {
        addr,
        audit_path,
        _tempdir: tmp,
    }
}

/// POST a JSON-RPC message with the standard headers a real client sends.
async fn rpc(srv: &TestServer, token: &str, body: Value) -> (reqwest::StatusCode, Value) {
    let resp = reqwest::Client::new()
        .post(format!("http://{}/mcp/{token}", srv.addr))
        .header("CF-Connecting-IP", "203.0.113.7")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-06-18")
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap();
    let value = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, value)
}

fn req(id: i64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

#[tokio::test]
async fn initialize_advertises_tools_capability() {
    let srv = start_server(Some(TOKEN)).await;
    let (status, v) = rpc(&srv, TOKEN, req(1, "initialize", json!({}))).await;
    assert_eq!(status, 200);
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["id"], 1);
    assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
    assert!(v["result"]["capabilities"]["tools"].is_object());
    assert_eq!(v["result"]["serverInfo"]["name"], "scroll-server");
}

#[tokio::test]
async fn tools_list_returns_both_tools() {
    let srv = start_server(Some(TOKEN)).await;
    let (status, v) = rpc(&srv, TOKEN, req(2, "tools/list", json!({}))).await;
    assert_eq!(status, 200);
    let tools = v["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"list_scrolls"));
    assert!(names.contains(&"read_scroll"));
    // read_scroll must declare `name` as required, or clients call it blind.
    let read = tools.iter().find(|t| t["name"] == "read_scroll").unwrap();
    assert_eq!(read["inputSchema"]["required"][0], "name");
}

#[tokio::test]
async fn list_scrolls_uses_public_names_and_hides_slugs() {
    let srv = start_server(Some(TOKEN)).await;
    let (_, v) = rpc(
        &srv,
        TOKEN,
        req(3, "tools/call", json!({ "name": "list_scrolls", "arguments": {} })),
    )
    .await;
    let text = v["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("financial"), "listing should name the scroll");
    assert!(text.contains("medical"));
    assert!(text.contains("Financial Context"), "title should come from the H1");
    // The bearer-token slug must never reach the model.
    assert!(!text.contains("copper-harbour"), "slug leaked into listing: {text}");
    assert!(!text.contains("lantern-meadow"), "slug leaked into listing: {text}");
    // Dotfiles are internal.
    assert!(!text.contains("slug-index"));
    assert!(!text.contains("word-pool"));
}

#[tokio::test]
async fn read_scroll_returns_full_content() {
    let srv = start_server(Some(TOKEN)).await;
    let (_, v) = rpc(
        &srv,
        TOKEN,
        req(4, "tools/call", json!({ "name": "read_scroll", "arguments": { "name": "financial" } })),
    )
    .await;
    assert_eq!(v["result"]["isError"], false);
    let text = v["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(text, "# Financial Context\n\nthe body.\n");
}

#[tokio::test]
async fn read_scroll_is_case_insensitive_and_trims() {
    let srv = start_server(Some(TOKEN)).await;
    let (_, v) = rpc(
        &srv,
        TOKEN,
        req(5, "tools/call", json!({ "name": "read_scroll", "arguments": { "name": "  FINANCIAL " } })),
    )
    .await;
    assert_eq!(v["result"]["isError"], false);
}

#[tokio::test]
async fn read_scroll_rejects_unknown_name_without_leaking_slugs() {
    let srv = start_server(Some(TOKEN)).await;
    let (_, v) = rpc(
        &srv,
        TOKEN,
        req(6, "tools/call", json!({ "name": "read_scroll", "arguments": { "name": "nope" } })),
    )
    .await;
    assert_eq!(v["result"]["isError"], true);
    let text = v["result"]["content"][0]["text"].as_str().unwrap();
    assert!(!text.contains("copper-harbour"), "slug leaked in error: {text}");
}

#[tokio::test]
async fn read_scroll_refuses_path_traversal() {
    let srv = start_server(Some(TOKEN)).await;
    for evil in ["../audit", "../../etc/passwd", "financial/../../x"] {
        let (_, v) = rpc(
            &srv,
            TOKEN,
            req(7, "tools/call", json!({ "name": "read_scroll", "arguments": { "name": evil } })),
        )
        .await;
        assert_eq!(v["result"]["isError"], true, "traversal accepted for {evil}");
    }
}

#[tokio::test]
async fn wrong_token_is_an_indistinguishable_404() {
    let srv = start_server(Some(TOKEN)).await;
    let (status, _) = rpc(&srv, "wrongtokenwrongtokenwrongtoken123456", req(8, "initialize", json!({}))).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn missing_cf_header_is_404() {
    let srv = start_server(Some(TOKEN)).await;
    // Deliberately no CF-Connecting-IP — the direct-localhost case.
    let resp = reqwest::Client::new()
        .post(format!("http://{}/mcp/{TOKEN}", srv.addr))
        .json(&req(9, "initialize", json!({})))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn route_absent_when_token_unset() {
    let srv = start_server(None).await;
    let (status, _) = rpc(&srv, TOKEN, req(10, "initialize", json!({}))).await;
    assert_eq!(status, 404, "MCP route must fail closed when no token configured");
}

#[tokio::test]
async fn foreign_origin_is_rejected() {
    let srv = start_server(Some(TOKEN)).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/mcp/{TOKEN}", srv.addr))
        .header("CF-Connecting-IP", "203.0.113.7")
        .header("Origin", "https://evil.example")
        .json(&req(11, "initialize", json!({})))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn claude_origin_is_accepted() {
    let srv = start_server(Some(TOKEN)).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/mcp/{TOKEN}", srv.addr))
        .header("CF-Connecting-IP", "203.0.113.7")
        .header("Origin", "https://claude.ai")
        .json(&req(12, "initialize", json!({})))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn unsupported_protocol_version_is_400() {
    let srv = start_server(Some(TOKEN)).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/mcp/{TOKEN}", srv.addr))
        .header("CF-Connecting-IP", "203.0.113.7")
        .header("MCP-Protocol-Version", "1999-01-01")
        .json(&req(13, "initialize", json!({})))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn absent_protocol_version_is_accepted() {
    let srv = start_server(Some(TOKEN)).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/mcp/{TOKEN}", srv.addr))
        .header("CF-Connecting-IP", "203.0.113.7")
        .json(&req(14, "initialize", json!({})))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn notification_returns_202_with_empty_body() {
    let srv = start_server(Some(TOKEN)).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/mcp/{TOKEN}", srv.addr))
        .header("CF-Connecting-IP", "203.0.113.7")
        .json(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);
    assert!(resp.text().await.unwrap().is_empty());
}

#[tokio::test]
async fn get_returns_405_not_a_stream() {
    let srv = start_server(Some(TOKEN)).await;
    let resp = reqwest::Client::new()
        .get(format!("http://{}/mcp/{TOKEN}", srv.addr))
        .header("CF-Connecting-IP", "203.0.113.7")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 405);
}

#[tokio::test]
async fn malformed_json_is_a_parse_error() {
    let srv = start_server(Some(TOKEN)).await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/mcp/{TOKEN}", srv.addr))
        .header("CF-Connecting-IP", "203.0.113.7")
        .header("Content-Type", "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let v: Value = serde_json::from_str(&resp.text().await.unwrap()).unwrap();
    assert_eq!(v["error"]["code"], -32700);
}

#[tokio::test]
async fn unknown_method_is_method_not_found() {
    let srv = start_server(Some(TOKEN)).await;
    let (status, v) = rpc(&srv, TOKEN, req(15, "does/notexist", json!({}))).await;
    assert_eq!(status, 200);
    assert_eq!(v["error"]["code"], -32601);
}

#[tokio::test]
async fn audit_log_records_the_scroll_but_never_the_token() {
    let srv = start_server(Some(TOKEN)).await;
    let _ = rpc(
        &srv,
        TOKEN,
        req(16, "tools/call", json!({ "name": "read_scroll", "arguments": { "name": "financial" } })),
    )
    .await;
    // Give the append a moment to land.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let log = std::fs::read_to_string(&srv.audit_path).unwrap_or_default();
    assert!(
        log.contains("read_scroll"),
        "audit should record which tool ran: {log}"
    );
    assert!(
        !log.contains(TOKEN),
        "the path token must never be written to the audit log: {log}"
    );
}
