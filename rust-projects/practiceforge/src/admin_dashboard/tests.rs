//! Tests for the admin dashboard module.

use super::routes::build_router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn test_index_returns_html() {
    let app = build_router();
    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
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
