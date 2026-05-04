//! Async Gmail REST client — `messages.list` + `messages.get?format=raw`.
//!
//! Mirrors the design of `practiceforge::email::gmail_pull::api` but
//! built on `reqwest`'s async client (we want concurrency over
//! `messages.get` to keep the pipe full given Google's per-request
//! latency of ~150-300ms).
//!
//! Quota math: `messages.list` is 5 units, `messages.get` is 5 units.
//! Gmail's per-user-per-second cap is ~250 units (and a 100-second
//! sliding budget of ~15,000 units → 150/s sustained). The 2026-05-04
//! overnight backfill blew past the second cap — at 40 msg/s × 5 units
//! we ran 200 units/s, right at the burst ceiling, and bursts at page
//! boundaries pushed over.
//!
//! v2 strategy:
//!  * Token-bucket rate limit at 150 units/s (60 % of burst, well below
//!    the 100 s sustained ceiling), 750-unit bucket (≈5 s headroom).
//!  * Concurrency cap of 3 in-flight `messages.get`.
//!  * On 403 quotaExceeded / 429: sleep ≥ 60 s, up to 15 retries,
//!    exponential backoff with jitter capped at 5 minutes. Quota
//!    windows are 100 s, so 60 s buys real recovery instead of
//!    hammering the same wall on millisecond timers.
//!  * 401 still bubbles up to the caller for one-shot token refresh.

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter as GovRateLimiter};
use serde::Deserialize;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

const API_BASE: &str = "https://gmail.googleapis.com/gmail/v1";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const USER_AGENT: &str = concat!("gmpull/", env!("CARGO_PKG_VERSION"));

/// Maximum retries per request. 15 is enough to ride through a full
/// 100-second Gmail quota window even with the floor 60 s sleep,
/// while still surrendering after ~15 minutes of futility.
const MAX_RETRIES: u32 = 15;

/// Floor on backoff sleep when we see a 403 quotaExceeded or 429.
/// The Gmail per-user quota window is 100 s; 60 s is enough headroom
/// for tokens to genuinely refill rather than just nibbling at the
/// edge.
const QUOTA_BACKOFF_FLOOR: Duration = Duration::from_secs(60);

/// Cap on a single backoff sleep. Five minutes is plenty — beyond
/// that we may as well surface the error and let the caller restart.
const BACKOFF_CAP: Duration = Duration::from_secs(300);

/// Quota cost of a single `messages.list` or `messages.get` call.
/// Gmail's quota model charges 5 units per message read.
pub const QUOTA_UNITS_PER_CALL: u32 = 5;

/// Quota cost of a single `users.history.list` call. Cheaper than
/// `messages.list` because we're asking "what changed since X?"
/// rather than enumerating the whole mailbox.
pub const QUOTA_UNITS_PER_HISTORY_CALL: u32 = 2;

/// Quota cost of a single `users.getProfile` call. Used once after
/// the initial backfill to seed the historyId checkpoint.
pub const QUOTA_UNITS_PER_PROFILE_CALL: u32 = 1;

/// Sustained units/second the limiter will allow. 150 is 60 % of
/// the burst ceiling and matches Gmail's 100 s sliding budget
/// (15 000 units / 100 s = 150/s).
pub const DEFAULT_RATE_UNITS_PER_SEC: u32 = 150;

/// Burst bucket size in quota units. 750 buys ~5 s of headroom
/// without ever being able to outrun the per-100 s budget.
pub const DEFAULT_RATE_BURST_UNITS: u32 = 750;

/// Concurrency cap for `messages.get`. Combined with the token
/// bucket this prevents bursts above 250 units/s even when Google's
/// latency briefly drops to single-digit milliseconds.
pub const DEFAULT_FETCH_CONCURRENCY: usize = 3;

/// Shared limiter type alias. `Arc` because it's cloned into every
/// fetch task. Direct quota: `Quota::per_second(N).allow_burst(B)`.
pub type SharedRateLimiter =
    Arc<GovRateLimiter<NotKeyed, InMemoryState, DefaultClock>>;

/// Build a token-bucket rate limiter sized for Gmail's per-user
/// quota. The limiter accepts up to `burst_units` instantly then
/// hands out `units_per_sec` tokens/s thereafter.
pub fn build_rate_limiter(units_per_sec: u32, burst_units: u32) -> SharedRateLimiter {
    let per_sec = NonZeroU32::new(units_per_sec.max(1)).expect("rate >= 1");
    let burst = NonZeroU32::new(burst_units.max(units_per_sec).max(1))
        .expect("burst >= rate >= 1");
    let quota = Quota::per_second(per_sec).allow_burst(burst);
    Arc::new(GovRateLimiter::direct(quota))
}

/// Wait until the limiter has `units` tokens available. Used by both
/// `messages.list` and `messages.get` callers.
async fn await_quota(limiter: &SharedRateLimiter, units: u32) {
    let n = NonZeroU32::new(units.max(1)).expect("units >= 1");
    // `until_n_ready` blocks until `n` cells are available — exactly
    // the semantics we want for "5 units per Gmail call".
    limiter
        .until_n_ready(n)
        .await
        .expect("rate limit cells <= burst by construction");
}

/// Concurrency gate for `messages.get`. Built once per session and
/// cloned into every fetch task.
pub fn build_fetch_semaphore(concurrency: usize) -> Arc<Semaphore> {
    Arc::new(Semaphore::new(concurrency.max(1)))
}

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

/// One row of the `users.history.list` response. Records can carry
/// any combination of `messagesAdded` / `messagesDeleted` /
/// `labelsAdded` / `labelsRemoved`. v0.2 only consumes the first
/// two; label changes don't affect the on-disk mirror because flags
/// are derived at write time from the message's full label list.
#[derive(Debug, Deserialize, Clone)]
pub struct HistoryRecord {
    /// The historyId for this record. Monotonically increasing.
    /// Kept as part of the parsed shape for debug visibility; the
    /// response-level `historyId` is what advances the checkpoint.
    #[serde(default)]
    #[allow(dead_code)]
    pub id: String,
    /// Messages newly added to the mailbox since the previous
    /// historyId. Each carries `id` (and `threadId`, ignored).
    #[serde(rename = "messagesAdded", default)]
    pub messages_added: Vec<HistoryMessageEntry>,
    /// Messages deleted (purged from Gmail entirely, not just
    /// trashed). v0.2 logs these but does not remove local files —
    /// the user's archive stays whole.
    #[serde(rename = "messagesDeleted", default)]
    pub messages_deleted: Vec<HistoryMessageEntry>,
}

/// An `{message: {id, ...}}` wrapper used inside both `messagesAdded`
/// and `messagesDeleted`. Gmail wraps the actual message under a
/// `message` key — we unwrap it here.
#[derive(Debug, Deserialize, Clone)]
pub struct HistoryMessageEntry {
    pub message: HistoryMessageRef,
}

/// Inner message reference inside a history entry. We keep `id` and
/// optionally the labelIds (used only for log readability on
/// deletes).
#[derive(Debug, Deserialize, Clone)]
pub struct HistoryMessageRef {
    pub id: String,
    #[serde(default, rename = "labelIds")]
    pub label_ids: Vec<String>,
}

/// Response from `users.history.list`. The `history` array may be
/// missing entirely if there have been no changes since
/// `startHistoryId`; serde defaults handle that.
#[derive(Debug, Deserialize)]
struct HistoryListResponse {
    #[serde(default)]
    history: Vec<HistoryRecord>,
    #[serde(rename = "nextPageToken", default)]
    next_page_token: Option<String>,
    /// The latest historyId at the moment the response was generated,
    /// even if `history` is empty. We use this to advance our
    /// checkpoint on quiet ticks.
    #[serde(rename = "historyId", default)]
    history_id: Option<String>,
}

/// Response from `users.getProfile`. Used at the end of the initial
/// backfill to seed the historyId checkpoint.
#[derive(Debug, Deserialize)]
struct ProfileResponse {
    #[serde(rename = "historyId", default)]
    history_id: Option<String>,
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
///
/// Acquires `QUOTA_UNITS_PER_CALL` tokens from the limiter before
/// hitting the API. Retries are also rate-limited (each attempt
/// re-acquires) so a long quota recovery doesn't immediately blow
/// through tokens once it succeeds.
pub async fn list_messages_page(
    http: &reqwest::Client,
    token: &str,
    page_token: Option<&str>,
    limiter: &SharedRateLimiter,
) -> Result<(Vec<MsgIdRef>, Option<String>)> {
    let mut url = format!("{API_BASE}/users/me/messages?maxResults=500");
    if let Some(t) = page_token {
        url.push_str(&format!("&pageToken={}", urlencoding::encode(t)));
    }

    let body = retry_get(http, token, &url, "messages.list", limiter).await?;
    let parsed: ListResponse =
        serde_json::from_str(&body).context("parsing messages.list JSON")?;
    Ok((parsed.messages, parsed.next_page_token))
}

/// Fetch one page of the `users.history.list` stream.
///
/// Returns `(records, nextPageToken, latestHistoryId)`. The third
/// value is non-`None` whenever Gmail returns a `historyId` field on
/// the response — even if the `history` array is empty (a quiet tick),
/// in which case the caller can use it to advance the local
/// checkpoint without doing any other work.
///
/// On a `404 / 400 historyId not found` error (Gmail expires history
/// after ~7 days for inactive accounts), we surface a tagged error
/// the caller (`pull()`) recognises as the "fall back to full
/// enumeration" trigger. Any other error bubbles up unchanged.
///
/// Costs `QUOTA_UNITS_PER_HISTORY_CALL` (2) tokens from the limiter
/// per attempt, including retries.
pub async fn list_history_page(
    http: &reqwest::Client,
    token: &str,
    start_history_id: &str,
    page_token: Option<&str>,
    limiter: &SharedRateLimiter,
) -> Result<(Vec<HistoryRecord>, Option<String>, Option<String>)> {
    let mut url = format!(
        "{API_BASE}/users/me/history?startHistoryId={}&maxResults=500",
        urlencoding::encode(start_history_id),
    );
    if let Some(t) = page_token {
        url.push_str(&format!("&pageToken={}", urlencoding::encode(t)));
    }

    let body = retry_get_with_units(
        http,
        token,
        &url,
        "history.list",
        limiter,
        QUOTA_UNITS_PER_HISTORY_CALL,
    )
    .await
    .map_err(classify_history_error)?;

    let parsed: HistoryListResponse =
        serde_json::from_str(&body).context("parsing history.list JSON")?;
    Ok((parsed.history, parsed.next_page_token, parsed.history_id))
}

/// Wrap a transport-level error coming back from `retry_get_with_units`
/// during a history call. If the error message indicates the
/// `startHistoryId` is too old or unknown we re-tag the error with
/// the literal string `"historyId not found"` so the pull loop can
/// match on it without parsing JSON. Any other error is passed
/// through unchanged.
fn classify_history_error(e: anyhow::Error) -> anyhow::Error {
    let msg = e.to_string();
    let stale = (msg.contains("HTTP 404") || msg.contains("HTTP 400"))
        && (msg.contains("historyId")
            || msg.contains("Invalid startHistoryId")
            || msg.contains("not found")
            || msg.contains("Start history ID"));
    if stale {
        e.context("historyId not found — caller should reseed via messages.list")
    } else {
        e
    }
}

/// Fetch the mailbox profile and return its current `historyId`.
/// Used after a fresh full backfill to seed the incremental
/// checkpoint. One quota unit; bubble up any error.
pub async fn get_profile_history_id(
    http: &reqwest::Client,
    token: &str,
    limiter: &SharedRateLimiter,
) -> Result<String> {
    let url = format!("{API_BASE}/users/me/profile");
    let body = retry_get_with_units(
        http,
        token,
        &url,
        "users.getProfile",
        limiter,
        QUOTA_UNITS_PER_PROFILE_CALL,
    )
    .await?;
    let parsed: ProfileResponse =
        serde_json::from_str(&body).context("parsing users.getProfile JSON")?;
    parsed
        .history_id
        .ok_or_else(|| anyhow!("users.getProfile returned no historyId"))
}

/// Fetch one message in `format=raw`. Decodes the base64url body
/// before returning so the caller writes plain RFC 5322 bytes.
pub async fn get_message_raw(
    http: &reqwest::Client,
    token: &str,
    id: &str,
    limiter: &SharedRateLimiter,
) -> Result<RawMessage> {
    let url = format!("{API_BASE}/users/me/messages/{id}?format=raw");
    let body = retry_get(http, token, &url, "messages.get", limiter).await?;
    parse_raw_message(&body)
}

/// Shared GET-with-retry helper. Returns the response body string.
///
/// The token bucket gates *every* attempt — including retries — so a
/// page that lands during a quota window doesn't burn through saved
/// tokens the moment it succeeds. On 401 we bubble up an error tagged
/// "unauthorized"; the caller (pull loop) re-fetches the token and
/// retries the call. On 403 quotaExceeded / 429 we sleep ≥ 60 s and
/// keep retrying for up to MAX_RETRIES attempts.
async fn retry_get(
    http: &reqwest::Client,
    token: &str,
    url: &str,
    op: &'static str,
    limiter: &SharedRateLimiter,
) -> Result<String> {
    retry_get_with_units(http, token, url, op, limiter, QUOTA_UNITS_PER_CALL).await
}

/// Variant of [`retry_get`] that takes the quota cost explicitly.
/// Used by `users.history.list` (2 units) and `users.getProfile`
/// (1 unit) which are cheaper than the default 5-unit `messages.*`
/// calls. The retry / backoff / 401 / 403 semantics are otherwise
/// identical.
async fn retry_get_with_units(
    http: &reqwest::Client,
    token: &str,
    url: &str,
    op: &'static str,
    limiter: &SharedRateLimiter,
    units: u32,
) -> Result<String> {
    let mut last_err: Option<anyhow::Error> = None;
    let mut quota_hit = false;
    for attempt in 0..MAX_RETRIES {
        // Wait for quota tokens before *every* attempt, including
        // retries. This is what stops us re-blowing the limit the
        // instant a 60 s sleep ends.
        await_quota(limiter, units).await;

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
                        quota_hit = true;
                        let cause = truncate(&body, 200);
                        tracing::warn!(
                            attempt = attempt + 1,
                            max = MAX_RETRIES,
                            op,
                            cause = %cause,
                            "429 rate-limited; will sleep >= 60 s"
                        );
                        last_err = Some(anyhow!(
                            "429 rate-limited on {op} (attempt {}/{}): {}",
                            attempt + 1,
                            MAX_RETRIES,
                            cause
                        ));
                    }
                    s if (500..=599).contains(&s) => {
                        let cause = truncate(&body, 200);
                        tracing::warn!(
                            attempt = attempt + 1,
                            max = MAX_RETRIES,
                            op,
                            status = s,
                            cause = %cause,
                            "5xx server error; backing off"
                        );
                        last_err = Some(anyhow!(
                            "{s} server error on {op} (attempt {}/{}): {}",
                            attempt + 1,
                            MAX_RETRIES,
                            cause
                        ));
                    }
                    403 if body.contains("userRateLimitExceeded")
                        || body.contains("rateLimitExceeded")
                        || body.contains("quotaExceeded") =>
                    {
                        quota_hit = true;
                        let cause = truncate(&body, 200);
                        tracing::warn!(
                            attempt = attempt + 1,
                            max = MAX_RETRIES,
                            op,
                            cause = %cause,
                            "403 quotaExceeded; will sleep >= 60 s"
                        );
                        last_err = Some(anyhow!(
                            "403 quota-exceeded on {op} (attempt {}/{}): {}",
                            attempt + 1,
                            MAX_RETRIES,
                            cause
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
                tracing::warn!(
                    attempt = attempt + 1,
                    max = MAX_RETRIES,
                    op,
                    error = %e,
                    "transport error; backing off"
                );
                last_err = Some(e);
            }
        }

        if attempt + 1 < MAX_RETRIES {
            let delay = backoff(attempt, quota_hit);
            tracing::info!(
                ?delay,
                attempt = attempt + 1,
                op,
                quota_hit,
                "retrying after backoff"
            );
            tokio::time::sleep(delay).await;
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow!("retry loop ran zero attempts on {op}")))
}

/// Exponential backoff with ±25 % jitter. When `quota_hit` is true,
/// the result is floored at 60 s so we never hammer a quota wall on
/// sub-second timers. Always capped at 5 minutes.
fn backoff(attempt: u32, quota_hit: bool) -> Duration {
    let base_ms = 1000u64.saturating_mul(2u64.saturating_pow(attempt));
    let cap_ms = BACKOFF_CAP.as_millis() as u64;
    let base_ms = base_ms.min(cap_ms);
    // Cheap pseudo-jitter from system clock — no rand crate needed.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let jitter_pct = (nanos % 500) as i64 - 250; // -250..250 → ±25 %
    let adjusted_ms =
        (base_ms as i64).saturating_add((base_ms as i64) * jitter_pct / 1000);
    let adjusted_ms = adjusted_ms.max(50) as u64;
    let mut delay = Duration::from_millis(adjusted_ms);
    if quota_hit && delay < QUOTA_BACKOFF_FLOOR {
        delay = QUOTA_BACKOFF_FLOOR;
    }
    if delay > BACKOFF_CAP {
        delay = BACKOFF_CAP;
    }
    delay
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
        // attempt=0 baseline ≈ 1 s ± 25 %.
        let d = backoff(0, false);
        assert!(
            d.as_millis() >= 700 && d.as_millis() <= 1300,
            "got {:?}",
            d
        );
        // High attempt counts cap at BACKOFF_CAP (5 min).
        assert!(backoff(20, false) <= BACKOFF_CAP);
    }

    #[test]
    fn backoff_quota_hit_floors_at_sixty_seconds() {
        // Even at attempt 0 (which would normally be ~1 s) the
        // quota-hit floor lifts the sleep to ≥ 60 s.
        let d = backoff(0, true);
        assert!(d >= QUOTA_BACKOFF_FLOOR, "got {:?}", d);
        // And we still respect the cap.
        assert!(d <= BACKOFF_CAP);
    }

    #[test]
    fn history_response_parses_added_and_deleted() {
        // A representative payload showing both messagesAdded and
        // messagesDeleted in a single record, plus a separate
        // labels-only record (which we intentionally ignore — flags
        // are derived at write time from the message's full label
        // list, not via diff).
        let body = r#"{
            "history": [
                {
                    "id": "1001",
                    "messages": [{"id": "msg-1", "threadId": "t1"}],
                    "messagesAdded": [
                        {"message": {"id": "msg-1", "labelIds": ["INBOX","UNREAD"]}}
                    ]
                },
                {
                    "id": "1002",
                    "messagesDeleted": [
                        {"message": {"id": "msg-2", "labelIds": ["TRASH"]}}
                    ]
                },
                {
                    "id": "1003",
                    "labelsAdded": [
                        {"message": {"id": "msg-3"}, "labelIds": ["STARRED"]}
                    ]
                }
            ],
            "nextPageToken": "PAGE",
            "historyId": "1003"
        }"#;
        let parsed: HistoryListResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.history.len(), 3);
        assert_eq!(parsed.next_page_token.as_deref(), Some("PAGE"));
        assert_eq!(parsed.history_id.as_deref(), Some("1003"));

        // First record has one added message.
        assert_eq!(parsed.history[0].messages_added.len(), 1);
        assert_eq!(parsed.history[0].messages_added[0].message.id, "msg-1");
        assert!(parsed.history[0].messages_deleted.is_empty());

        // Second record has one deleted message.
        assert!(parsed.history[1].messages_added.is_empty());
        assert_eq!(parsed.history[1].messages_deleted.len(), 1);
        assert_eq!(parsed.history[1].messages_deleted[0].message.id, "msg-2");

        // Third record (labels-only) yields nothing in either array.
        assert!(parsed.history[2].messages_added.is_empty());
        assert!(parsed.history[2].messages_deleted.is_empty());
    }

    #[test]
    fn history_response_handles_empty_quiet_tick() {
        // No changes since the last historyId. The `history` array
        // is absent; serde defaults give us a vec of zero records.
        // `historyId` is still set so the caller can advance the
        // checkpoint without doing more work.
        let body = r#"{"historyId":"42"}"#;
        let parsed: HistoryListResponse = serde_json::from_str(body).unwrap();
        assert!(parsed.history.is_empty());
        assert!(parsed.next_page_token.is_none());
        assert_eq!(parsed.history_id.as_deref(), Some("42"));
    }

    #[test]
    fn extract_added_ids_from_history_records() {
        // Helper-style test mirroring what main.rs::pull does:
        // flatten every record's messagesAdded into a deduped Vec
        // of Gmail IDs preserving order. Two records both add msg-A;
        // the duplicate must collapse.
        let body = r#"{
            "history": [
                {"id": "1", "messagesAdded": [
                    {"message": {"id": "msg-A", "labelIds": ["INBOX"]}}
                ]},
                {"id": "2", "messagesAdded": [
                    {"message": {"id": "msg-B", "labelIds": ["INBOX"]}},
                    {"message": {"id": "msg-A", "labelIds": ["INBOX"]}}
                ]}
            ],
            "historyId": "2"
        }"#;
        let parsed: HistoryListResponse = serde_json::from_str(body).unwrap();
        let mut seen = std::collections::HashSet::new();
        let mut ordered: Vec<String> = Vec::new();
        for rec in &parsed.history {
            for added in &rec.messages_added {
                if seen.insert(added.message.id.clone()) {
                    ordered.push(added.message.id.clone());
                }
            }
        }
        assert_eq!(ordered, vec!["msg-A".to_string(), "msg-B".to_string()]);
    }

    #[test]
    fn classify_history_error_tags_stale_history() {
        let stale = anyhow!(
            "HTTP 404 on history.list: {{\"error\":{{\"message\":\"Invalid startHistoryId\"}}}}"
        );
        let tagged = classify_history_error(stale);
        assert!(
            tagged.to_string().contains("historyId not found")
                || format!("{tagged:#}").contains("historyId not found"),
            "tagged: {tagged:#}",
        );

        // Unrelated transport errors must NOT be tagged.
        let other = anyhow!("HTTP 500 on history.list: server unavailable");
        let unchanged = classify_history_error(other);
        assert!(!unchanged.to_string().contains("historyId not found"));
    }

    #[test]
    fn profile_response_parses_history_id() {
        let body = r#"{
            "emailAddress": "x@y.z",
            "messagesTotal": 12345,
            "threadsTotal": 6789,
            "historyId": "987654"
        }"#;
        let parsed: ProfileResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.history_id.as_deref(), Some("987654"));
    }

    #[tokio::test]
    async fn rate_limiter_blocks_then_admits() {
        // 10 units/s, 10-unit bucket. Drain the bucket, then time
        // the next 5-unit acquisition — it should take ~500 ms.
        let limiter = build_rate_limiter(10, 10);
        await_quota(&limiter, 10).await; // drain

        let start = std::time::Instant::now();
        await_quota(&limiter, 5).await;
        let elapsed = start.elapsed();
        // Tokens accrue at 10/s → 5 tokens take ~500 ms. Allow a
        // wide window for slow CI: 200..1500 ms is reasonable.
        assert!(
            elapsed.as_millis() >= 200,
            "limiter admitted too quickly: {:?}",
            elapsed
        );
        assert!(
            elapsed.as_millis() <= 1500,
            "limiter held too long: {:?}",
            elapsed
        );
    }
}
