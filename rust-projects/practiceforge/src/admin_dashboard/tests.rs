//! Tests for the admin dashboard module.

use super::routes::build_router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn test_index_redirects_to_login_without_auth() {
    let app = build_router();
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    // Unauthenticated: should redirect to /login
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers().get("location").map(|v| v.to_str().unwrap_or("")).unwrap_or("");
    assert_eq!(location, "/login");
}

#[tokio::test]
async fn test_login_page_returns_html() {
    let app = build_router();
    let response = app
        .oneshot(Request::builder().uri("/login").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap_or(""))
        .unwrap_or("");
    assert!(
        content_type.contains("text/html"),
        "Expected text/html, got: {}",
        content_type
    );
}

#[tokio::test]
async fn test_clients_endpoint_returns_json() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/clients")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should return 200 even if registry is empty/unconfigured
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap_or(""))
        .unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "Expected application/json, got: {}",
        content_type
    );
}

#[tokio::test]
async fn test_search_requires_query() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/search?q=")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Empty query should return empty array
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let results: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_calendar_with_date_param() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/calendar?date=2026-04-16")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap_or(""))
        .unwrap_or("");
    assert!(
        content_type.contains("application/json"),
        "Expected application/json, got: {}",
        content_type
    );
}

#[tokio::test]
async fn test_calendar_week_view() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/calendar?date=2026-04-16&week=true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_billing_status_returns_json() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/billing/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_billing_summary_returns_json() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/billing/summary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_practice_info_returns_json() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/practice")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_practitioners_returns_json() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/practitioners")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_unknown_route_returns_404() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// OAuth setup endpoints
//
// These test request/response shape only — the handlers shell out to a real
// pizauth daemon so we can't unit-test the device-code path. Whatever pizauth
// is on the test runner's PATH (or absent) we should still get well-formed
// JSON back.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_oauth_preflight_returns_json() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/email/oauth/preflight")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // pizauth_installed must always be present and a bool — install_help is
    // null when installed, string when missing.
    assert!(v.get("pizauth_installed").and_then(|x| x.as_bool()).is_some());
    assert!(v.get("install_help").is_some());
}

#[tokio::test]
async fn test_oauth_init_rejects_missing_email() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/email/oauth/init")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"provider":"m365","email":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let err = v.get("error").and_then(|x| x.as_str()).unwrap_or("");
    assert!(err.contains("email"), "expected 'email' in error, got: {err}");
}

#[tokio::test]
async fn test_oauth_init_rejects_unknown_provider() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/email/oauth/init")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"provider":"yahoo","email":"a@b.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    // Either preflight-fail (no pizauth) or unknown-provider error — both 400.
    // We don't assert on the message because CI may or may not have pizauth.
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_oauth_init_gmail_requires_credentials() {
    // If pizauth isn't installed on the runner, init bails at preflight
    // before reaching the credential check — skip in that case.
    let pizauth_present = std::process::Command::new("pizauth")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !pizauth_present {
        eprintln!("skipping: pizauth not on PATH");
        return;
    }

    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/email/oauth/init")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"provider":"gmail","email":"a@b.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let err = v.get("error").and_then(|x| x.as_str()).unwrap_or("");
    assert!(
        err.contains("client_id") && err.contains("client_secret"),
        "expected client_id+client_secret in error, got: {err}"
    );
}

#[tokio::test]
async fn test_oauth_status_requires_account_param() {
    let app = build_router();
    // No account query param — Axum's Query extractor rejects with 400.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/email/oauth/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_oauth_status_returns_well_formed_json() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/email/oauth/status?account=nonexistent-account-xyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        v.get("account").and_then(|x| x.as_str()),
        Some("nonexistent-account-xyz")
    );
    // Without a real pizauth account, has_token must be false (whether
    // pizauth is installed or not — same shape, different error string).
    assert_eq!(v.get("has_token").and_then(|x| x.as_bool()), Some(false));
    assert!(v.get("error").is_some());
}
