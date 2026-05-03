//! Async Gmail REST client — `messages.list` + `messages.get?format=raw`.
//!
//! Mirrors the design of `practiceforge::email::gmail_pull::api` but
//! built on `reqwest`'s async client (we want concurrency over
//! `messages.get` to keep the pipe full given Google's per-request
//! latency of ~150-300ms).
//!
//! Quota math: `messages.list` is 5 units, `messages.get` is 5 units.
//! The default per-user-per-second budget is 250 units. With
//! batches of 16 concurrent `get`s we burn 80 units/s — well under
//! the cap, and well below the daily 1B cap (we'd hit ~6.9M
//! units/day at full pull rate, all comfortably within budget).
//!
//! Retries: 401 triggers a one-shot token refresh (caller's job —
//! see `pull` loop). 429 and 5xx use exponential backoff with
//! jitter; max 5 retries per request.

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use std::time::Duration;

const API_BASE: &str = "https://gmail.googleapis.com/gmail/v1";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const USER_AGENT: &str = concat!("gmpull/", env!("CARGO_PKG_VERSION"));
const MAX_RETRIES: u32 = 5;

/// Response from `users.messages.list`. We keep `id` only; threadId
/// and resultSizeEstimate aren't needed for the maildir mirror.
#[derive(Debug, Deserialize)]
struct ListResponse {
    #[serde(default)]
    messages: Vec<MsgIdRef>,
    #[serde(rename = "nextPageToken", default)]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MsgIdRef {
    pub id: String,
}

/// One message fetched via `messages.get?format=raw`.
#[derive(Debug)]
pub struct RawMessage {
    pub id: String,
    pub label_ids: Vec<String>,
    /// Gmail's `internalDate` is milliseconds since epoch (string in
    /// the JSON). Used for maildir mtime so date-sorted clients show
    /// real receipt order.
    pub internal_date_ms: i64,
    /// Decoded RFC 5322 message bytes (headers + body, including the
    /// blank line). Suitable for direct write to maildir.
    pub raw_rfc822: Vec<u8>,
}

/// Build the shared async HTTP client. One instance per process is
/// fine — `reqwest::Client` is cheap to clone and pools connections
/// internally.
pub fn build_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .context("building reqwest client")
}

/// Fetch one page of message IDs. Returns the IDs and the next page
/// token (if any). Pass `page_token = None` for the first call.
pub async fn list_messages_page(
    http: &reqwest::Client,
    token: &str,
    page_token: Option<&str>,
) -> Result<(Vec<MsgIdRef>, Option<String>)> {
    let mut url = format!("{API_BASE}/users/me/messages?maxResults=500");
    if let Some(t) = page_token {
        url.push_str(&format!("&pageToken={}", urlencoding::encode(t)));
    }

    let body = retry_get(http, token, &url, "messages.list").await?;
    let parsed: ListResponse =
        serde_json::from_str(&body).context("parsing messages.list JSON")?;
    Ok((parsed.messages, parsed.next_page_token))
}

/// Fetch one message in `format=raw`. Decodes the base64url body
/// before returning so the caller writes plain RFC 5322 bytes.
pub async fn get_message_raw(
    http: &reqwest::Client,
    token: &str,
    id: &str,
) -> Result<RawMessage> {
    let url = format!("{API_BASE}/users/me/messages/{id}?format=raw");
    let body = retry_get(http, token, &url, "messages.get").await?;
    parse_raw_message(&body)
}

/// Shared GET-with-retry helper. Returns the response body string.
/// On 401, returns Ok with an empty body would be wrong, so we bubble
/// up an error tagged "unauthorized" — the caller (pull loop) is
/// expected to re-fetch the token and retry once at the next message.
async fn retry_get(
    http: &reqwest::Client,
    token: &str,
    url: &str,
    op: &'static str,
) -> Result<String> {
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..MAX_RETRIES {
        let resp_result = http
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .with_context(|| format!("HTTP GET {op}"));

        match resp_result {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                match status.as_u16() {
                    200 => return Ok(body),
                    401 => {
                        return Err(anyhow!(
                            "401 unauthorized on {op} — token expired (body: {})",
                            truncate(&body, 200)
                        ));
                    }
                    429 => {
                        last_err = Some(anyhow!(
                            "429 rate-limited on {op} (attempt {}/{})",
                            attempt + 1,
                            MAX_RETRIES
                        ));
                    }
                    s if (500..=599).contains(&s) => {
                        last_err = Some(anyhow!(
                            "{s} server error on {op} (attempt {}/{}): {}",
                            attempt + 1,
                            MAX_RETRIES,
                            truncate(&body, 200)
                        ));
                    }
                    s if s == 403
                        && (body.contains("userRateLimitExceeded")
                            || body.contains("rateLimitExceeded")) =>
                    {
                        last_err = Some(anyhow!(
                            "403 quota-exceeded on {op} (attempt {}/{})",
                            attempt + 1,
                            MAX_RETRIES
                        ));
                    }
                    s => {
                        // Non-transient error — fail fast.
                        return Err(anyhow!(
                            "HTTP {s} on {op}: {}",
                            truncate(&body, 500)
                        ));
                    }
                }
            }
            Err(e) => {
                last_err = Some(e);
            }
        }

        if attempt + 1 < MAX_RETRIES {
            let delay = backoff(attempt);
            tracing::debug!(?delay, attempt, op, "retrying after backoff");
            tokio::time::sleep(delay).await;
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow!("retry loop ran zero attempts on {op}")))
}

/// Exponential backoff with ±25% jitter. Capped at 64 s.
fn backoff(attempt: u32) -> Duration {
    let base_ms = 1000u64.saturating_mul(2u64.saturating_pow(attempt));
    let base_ms = base_ms.min(64_000);
    // Cheap pseudo-jitter from system clock — no rand crate needed.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let jitter_pct = (nanos % 500) as i64 - 250; // -250..250 → ±25 %
    let adjusted = (base_ms as i64).saturating_add((base_ms as i64) * jitter_pct / 1000);
    Duration::from_millis(adjusted.max(50) as u64)
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…[+{} bytes]", &s[..n], s.len() - n)
    }
}

fn parse_raw_message(json: &str) -> Result<RawMessage> {
    #[derive(Deserialize)]
    struct Shape {
        id: String,
        #[serde(default, rename = "labelIds")]
        label_ids: Vec<String>,
        #[serde(rename = "internalDate")]
        internal_date: String,
        raw: String,
    }
    let s: Shape = serde_json::from_str(json).context("parsing messages.get JSON")?;
    let raw_rfc822 = URL_SAFE_NO_PAD
        .decode(s.raw.trim_end_matches('='))
        .context("decoding base64url raw body")?;
    let internal_date_ms: i64 = s
        .internal_date
        .parse()
        .with_context(|| format!("parsing internalDate {:?}", s.internal_date))?;
    Ok(RawMessage {
        id: s.id,
        label_ids: s.label_ids,
        internal_date_ms,
        raw_rfc822,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_raw_message_decodes_base64url() {
        let json = r#"{
            "id": "abc",
            "labelIds": ["INBOX", "UNREAD"],
            "internalDate": "1700000000000",
            "raw": "RnJvbTogYUBiLmMNClN1YmplY3Q6IGhpDQoNCmJvZHk"
        }"#;
        let m = parse_raw_message(json).unwrap();
        assert_eq!(m.id, "abc");
        assert_eq!(m.label_ids, vec!["INBOX", "UNREAD"]);
        assert_eq!(m.internal_date_ms, 1_700_000_000_000);
        let s = String::from_utf8(m.raw_rfc822).unwrap();
        assert!(s.contains("From: a@b.c"));
        assert!(s.contains("Subject: hi"));
        assert!(s.contains("body"));
    }

    #[test]
    fn list_response_handles_empty_and_paginated() {
        let body = r#"{"messages":[{"id":"1"},{"id":"2"}],"nextPageToken":"X"}"#;
        let r: ListResponse = serde_json::from_str(body).unwrap();
        assert_eq!(r.messages.len(), 2);
        assert_eq!(r.next_page_token.as_deref(), Some("X"));

        let empty = r#"{}"#;
        let r: ListResponse = serde_json::from_str(empty).unwrap();
        assert!(r.messages.is_empty());
        assert!(r.next_page_token.is_none());
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert!(backoff(0).as_millis() >= 700 && backoff(0).as_millis() <= 1300);
        assert!(backoff(10).as_millis() <= 80_000); // capped + jitter
    }
}
