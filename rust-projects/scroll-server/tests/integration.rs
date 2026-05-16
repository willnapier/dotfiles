//! End-to-end tests against an ephemeral scroll-server.
//!
//! Spins up a real axum server on 127.0.0.1:0 with a synthetic seed,
//! word-list, and scroll dir in a tempdir; hits it with reqwest.

use chrono::Utc;
use scroll_server::audit::AuditLog;
use scroll_server::build_app;
use scroll_server::config::Config;
use scroll_server::day_word::day_word;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

struct TestServer {
    addr: SocketAddr,
    _tempdir: tempfile::TempDir,
    todays_word: String,
}

async fn start_server() -> TestServer {
    let tmp = tempfile::tempdir().expect("tempdir");
    let scroll_dir = tmp.path().join("scrolls");
    std::fs::create_dir_all(&scroll_dir).unwrap();
    std::fs::write(scroll_dir.join("financial.md"), b"# Financial scroll\n\nbody.\n").unwrap();
    std::fs::write(scroll_dir.join("medical.md"), b"# Medical scroll\n\nbody.\n").unwrap();

    let seed = b"integration-test-seed-bytes-here".to_vec();
    let word_list: Vec<String> = vec![
        "HARBOUR", "MEADOW", "COMPASS", "LANTERN", "ORCHARD",
        "COTTAGE", "PEBBLE", "RIBBON", "THUNDER", "WILLOW",
        "COPPER", "ANCHOR", "GARDEN", "MOUNTAIN", "SAPPHIRE",
        "OXFORD", "MARBLE", "CANYON", "FORTRESS", "OCEAN",
        "PARCHMENT", "HORIZON", "COMET", "GRANITE", "JOURNEY",
        "MELODY", "PRAIRIE", "RIVER", "SIGNAL", "VELVET",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    let audit_path: PathBuf = tmp.path().join("audit.log");
    let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();

    let cfg = Config {
        bind,
        scroll_dir: scroll_dir.clone(),
        seed: seed.clone(),
        word_list: word_list.clone(),
        audit_log: audit_path.clone(),
    };

    let audit = Arc::new(AuditLog::new(audit_path));
    let app = build_app(&cfg, audit);

    let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let todays_word = day_word(&seed, &word_list, Utc::now());

    TestServer {
        addr,
        _tempdir: tmp,
        todays_word,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().build().unwrap()
}

#[tokio::test]
async fn healthz_returns_ok() {
    let srv = start_server().await;
    let resp = client()
        .get(format!("http://{}/healthz", srv.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_eq!(resp.text().await.unwrap(), "ok\n");
}

#[tokio::test]
async fn valid_path_returns_markdown() {
    let srv = start_server().await;
    let resp = client()
        .get(format!(
            "http://{}/financial/{}",
            srv.addr, srv.todays_word
        ))
        .header("CF-Connecting-IP", "203.0.113.1")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.starts_with("text/markdown"));
    let cc = resp
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(cc.contains("no-store"));
    let body = resp.text().await.unwrap();
    assert!(body.contains("Financial scroll"));
}

#[tokio::test]
async fn wrong_word_returns_404() {
    let srv = start_server().await;
    // Pick a word that's definitely not today's.
    let wrong = if srv.todays_word == "HARBOUR" { "MEADOW" } else { "HARBOUR" };
    let resp = client()
        .get(format!("http://{}/financial/{}", srv.addr, wrong))
        .header("CF-Connecting-IP", "203.0.113.2")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
    assert_eq!(resp.text().await.unwrap(), "");
}

#[tokio::test]
async fn wrong_topic_returns_404() {
    let srv = start_server().await;
    let resp = client()
        .get(format!(
            "http://{}/Financial/{}",
            srv.addr, srv.todays_word
        ))
        .header("CF-Connecting-IP", "203.0.113.3")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
    assert_eq!(resp.text().await.unwrap(), "");
}

#[tokio::test]
async fn missing_file_returns_404_identical_to_wrong_word() {
    let srv = start_server().await;
    // `business.md` doesn't exist in the tempdir; today's word IS valid.
    let resp = client()
        .get(format!(
            "http://{}/business/{}",
            srv.addr, srv.todays_word
        ))
        .header("CF-Connecting-IP", "203.0.113.4")
        .send()
        .await
        .unwrap();
    let status_missing = resp.status();
    let body_missing = resp.text().await.unwrap();
    assert_eq!(status_missing, reqwest::StatusCode::NOT_FOUND);
    assert_eq!(body_missing, "");

    // Verify byte-for-byte identical 404 against a wrong-word path. This is the
    // threat-model guarantee from §5: an attacker probing must not be able to
    // distinguish "wrong word" from "missing file".
    let wrong = if srv.todays_word == "HARBOUR" { "MEADOW" } else { "HARBOUR" };
    let resp2 = client()
        .get(format!("http://{}/financial/{}", srv.addr, wrong))
        .header("CF-Connecting-IP", "203.0.113.5")
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), reqwest::StatusCode::NOT_FOUND);
    assert_eq!(resp2.text().await.unwrap(), "");
}

#[tokio::test]
async fn missing_cf_connecting_ip_is_rejected() {
    let srv = start_server().await;
    let resp = client()
        .get(format!(
            "http://{}/financial/{}",
            srv.addr, srv.todays_word
        ))
        .send()
        .await
        .unwrap();
    // Per §7 we reject. Implementation choice: 404 (uniform with the rest).
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn root_returns_404() {
    let srv = start_server().await;
    let resp = client()
        .get(format!("http://{}/", srv.addr))
        .header("CF-Connecting-IP", "203.0.113.6")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn topic_only_returns_404() {
    let srv = start_server().await;
    let resp = client()
        .get(format!("http://{}/financial", srv.addr))
        .header("CF-Connecting-IP", "203.0.113.7")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rate_limit_kicks_in_after_60() {
    let srv = start_server().await;
    let cli = client();
    let url = format!("http://{}/financial/{}", srv.addr, srv.todays_word);
    let mut last = reqwest::StatusCode::OK;
    // Burst 65 with the SAME IP — bucket capacity is 60, so >= one of the last
    // few should hit 429.
    for _ in 0..65 {
        last = cli
            .get(&url)
            .header("CF-Connecting-IP", "203.0.113.99")
            .send()
            .await
            .unwrap()
            .status();
    }
    assert_eq!(
        last,
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        "expected 429 within 65 requests; got {last}"
    );
}
