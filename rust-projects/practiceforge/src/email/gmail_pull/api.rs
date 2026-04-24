//! Raw-HTTP Gmail API client — the wire layer for gmail-pull.
//!
//! Five endpoints, one rate-limit strategy. Everything Leg-2+ needs
//! for maildir mirroring runs through here; no other module speaks
//! HTTP to Gmail directly.
//!
//! ## Why raw reqwest instead of `google-gmail1`
//!
//! practiceforge already has `reqwest`, `serde`, `serde_json` on its
//! dependency path (used by m365_oauth, gmail_oauth, gmail_push_tags,
//! the SMTP and Graph backends). Adding the generated `google-gmail1`
//! crate drags in Hyper 1.x and ~500 KB of binary bloat for five
//! endpoints of actual surface. Raw reqwest keeps the build slim and
//! the error-handling uniform with the rest of the email module.
//!
//! ## Authentication
//!
//! Tokens come from [`crate::email::gmail_oauth::refresh`] +
//! `gmail-pf-access` keychain entry. `GmailApi::new` refreshes once
//! at construction; long-running pulls call `refresh_token_if_stale`
//! before each batch to handle the ~60 min Google access-token
//! expiry without mid-batch 401s.
//!
//! ## Rate limits and retries
//!
//! Gmail grants 250 quota units / sec / user (15,000 / min). One
//! `messages.get` / `messages.list` / `history.list` call costs 1
//! unit; `messages.batch` is counted per sub-request. We're nowhere
//! near the ceiling in normal operation, but brief spikes do happen
//! — handled by [`retry_with_backoff`] which does `min(2^n + jitter,
//! 64s)` on 429 and 5xx, up to 6 tries.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const API_BASE: &str = "https://gmail.googleapis.com/gmail/v1";
const BATCH_URL: &str = "https://gmail.googleapis.com/batch/gmail/v1";
const BATCH_CHUNK_SIZE: usize = 50;
const REQUEST_TIMEOUT_SECS: u64 = 180;

// -----------------------------------------------------------------
// Error type
// -----------------------------------------------------------------

/// Gmail API errors we care about for flow control. Other errors
/// (network, parse) flow through `anyhow::Error` via `Result`.
#[derive(Debug)]
pub enum GmailApiError {
    /// The stored `startHistoryId` is older than Gmail's history
    /// retention (typically ~7 days). Caller must fall back to a
    /// full resync: clear state, run a fresh `messages.list`.
    HistoryIdExpired,
    /// Gmail returned 429 or a `userRateLimitExceeded` message.
    /// `retry_after` is the Gmail-hinted seconds to wait, or None
    /// if we should use our own exponential backoff.
    RateLimit { retry_after: Option<u64> },
    /// Catch-all for HTTP failures that don't match the above.
    Http { status: u16, body: String },
}

impl std::fmt::Display for GmailApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HistoryIdExpired => write!(
                f,
                "Gmail historyId is expired (>~7 days old) — full resync required"
            ),
            Self::RateLimit { retry_after } => write!(
                f,
                "Gmail rate limit hit (retry-after: {:?})",
                retry_after
            ),
            Self::Http { status, body } => write!(
                f,
                "Gmail HTTP {status}: {body}"
            ),
        }
    }
}

impl std::error::Error for GmailApiError {}

// -----------------------------------------------------------------
// Response shapes
// -----------------------------------------------------------------

/// Subset of `users.getProfile` response we care about.
#[derive(Debug, Deserialize, Serialize)]
pub struct Profile {
    #[serde(rename = "emailAddress")]
    pub email_address: String,
    #[serde(rename = "messagesTotal")]
    pub messages_total: u64,
    #[serde(rename = "threadsTotal")]
    pub threads_total: u64,
    /// Anchor point for incremental `history.list` calls. Store and
    /// pass back as `startHistoryId` for delta syncs.
    #[serde(rename = "historyId")]
    pub history_id: String,
}

/// Subset of `users.messages.list` response we care about.
#[derive(Debug, Deserialize)]
struct MessageListResponse {
    #[serde(default)]
    messages: Vec<MessageIdRef>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
    #[allow(dead_code)]
    #[serde(rename = "resultSizeEstimate")]
    result_size_estimate: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct MessageIdRef {
    id: String,
    #[allow(dead_code)]
    #[serde(rename = "threadId")]
    thread_id: Option<String>,
}

/// One message fetched via `batch_get_raw`. `raw_bytes` contains the
/// full RFC-5322 message (headers + body); Gmail returns it
/// base64-URL-encoded over the wire and we decode before returning.
#[derive(Debug)]
pub struct BatchMessage {
    pub gmail_id: String,
    pub raw_bytes: Vec<u8>,
    pub label_ids: Vec<String>,
    #[allow(dead_code)]
    pub thread_id: String,
}

/// Subset of `users.history.list` response. Full event taxonomy
/// handled in Leg 3 (`history` module); for now we parse enough to
/// signal freshness and count events.
#[derive(Debug, Deserialize)]
pub struct HistoryResponse {
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
    /// The `historyId` value caller should store for the next
    /// incremental sync. If the account was quiet, this equals the
    /// value we passed in as `startHistoryId`.
    #[serde(rename = "historyId")]
    pub history_id: String,
}

#[derive(Debug, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    #[serde(default, rename = "messagesAdded")]
    pub messages_added: Vec<HistoryMessageRef>,
    #[serde(default, rename = "messagesDeleted")]
    pub messages_deleted: Vec<HistoryMessageRef>,
    #[serde(default, rename = "labelsAdded")]
    pub labels_added: Vec<HistoryLabelChange>,
    #[serde(default, rename = "labelsRemoved")]
    pub labels_removed: Vec<HistoryLabelChange>,
}

#[derive(Debug, Deserialize)]
pub struct HistoryMessageRef {
    pub message: HistoryMessage,
}

#[derive(Debug, Deserialize)]
pub struct HistoryMessage {
    pub id: String,
    #[serde(default, rename = "threadId")]
    pub thread_id: String,
    #[serde(default, rename = "labelIds")]
    pub label_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct HistoryLabelChange {
    pub message: HistoryMessage,
    #[serde(default, rename = "labelIds")]
    pub label_ids: Vec<String>,
}

// -----------------------------------------------------------------
// Client
// -----------------------------------------------------------------

/// A thin wrapper around `reqwest::blocking::Client` that carries a
/// cached Gmail bearer token. Construct with [`GmailApi::new`]; the
/// constructor performs a silent token refresh so downstream calls
/// rarely hit the refresh path inline.
pub struct GmailApi {
    http: reqwest::blocking::Client,
    access_token: String,
}

impl GmailApi {
    /// Build a new client. Refreshes the Gmail OAuth token on the way
    /// in, so the constructor is the single "do we have working
    /// credentials?" test point for the caller.
    pub fn new() -> Result<Self> {
        crate::email::gmail_oauth::refresh()
            .context("refreshing Gmail OAuth before API client construction")?;
        let access_token = crate::keystore::get("himalaya-cli", "gmail-pf-access")
            .context("reading gmail-pf-access from keystore")?
            .ok_or_else(|| {
                anyhow!("no Gmail access token in keychain — run `practiceforge email init` first")
            })?;
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .context("building HTTP client for Gmail API")?;
        Ok(Self { http, access_token })
    }

    // -----------------------------------------------------------------
    // GET /users/me/profile
    // -----------------------------------------------------------------

    /// Fetch the account profile — email address, message/thread
    /// totals, and the current `historyId` (the anchor for subsequent
    /// incremental syncs).
    pub fn get_profile(&self) -> Result<Profile> {
        let url = format!("{API_BASE}/users/me/profile");
        let resp = retry_with_backoff(|| {
            self.http
                .get(&url)
                .bearer_auth(&self.access_token)
                .send()
                .context("GET /users/me/profile")
        })?;
        let status = resp.status();
        let body = resp.text().context("reading profile response body")?;
        if !status.is_success() {
            return Err(http_error(status, &body).into());
        }
        serde_json::from_str(&body).context("parsing profile JSON").map_err(Into::into)
    }

    // -----------------------------------------------------------------
    // GET /users/me/messages
    // -----------------------------------------------------------------

    /// Fetch one page of message IDs matching `q`. Pass `None` for
    /// `page_token` on the first call; subsequent calls use the token
    /// returned from the previous response.
    pub fn list_message_ids(
        &self,
        q: &str,
        page_token: Option<&str>,
    ) -> Result<(Vec<String>, Option<String>)> {
        let mut url = format!(
            "{API_BASE}/users/me/messages?maxResults=500&q={}",
            urlencoding::encode(q)
        );
        if let Some(t) = page_token {
            url.push_str(&format!("&pageToken={}", urlencoding::encode(t)));
        }

        let resp = retry_with_backoff(|| {
            self.http
                .get(&url)
                .bearer_auth(&self.access_token)
                .send()
                .context("GET /users/me/messages")
        })?;
        let status = resp.status();
        let body = resp.text().context("reading messages.list body")?;
        if !status.is_success() {
            return Err(http_error(status, &body).into());
        }
        let parsed: MessageListResponse =
            serde_json::from_str(&body).context("parsing messages.list JSON")?;
        let ids: Vec<String> = parsed.messages.into_iter().map(|m| m.id).collect();
        Ok((ids, parsed.next_page_token))
    }

    /// Paginate [`Self::list_message_ids`] to completion, calling
    /// `progress` after each page with the cumulative count so the
    /// caller can report long-running progress.
    pub fn list_all_message_ids<F: FnMut(usize)>(
        &self,
        q: &str,
        mut progress: F,
    ) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let (ids, next) = self.list_message_ids(q, page_token.as_deref())?;
            out.extend(ids);
            progress(out.len());
            match next {
                Some(t) => page_token = Some(t),
                None => break,
            }
        }
        Ok(out)
    }

    // -----------------------------------------------------------------
    // GET /users/me/messages/{id}?format=raw   (single-message fetch)
    // -----------------------------------------------------------------

    /// Single-message variant of the raw fetch. Used for cases where
    /// batching is overkill (one-off, retry of a failed batch entry,
    /// the probe subcommand). For bulk sync, prefer
    /// [`Self::batch_get_raw`].
    pub fn get_message_raw(&self, gmail_id: &str) -> Result<BatchMessage> {
        let url = format!(
            "{API_BASE}/users/me/messages/{gmail_id}?format=raw"
        );
        let resp = retry_with_backoff(|| {
            self.http
                .get(&url)
                .bearer_auth(&self.access_token)
                .send()
                .context("GET /users/me/messages/{id}?format=raw")
        })?;
        let status = resp.status();
        let body = resp.text().context("reading message body")?;
        if !status.is_success() {
            return Err(http_error(status, &body).into());
        }
        parse_raw_message_json(&body)
    }

    // -----------------------------------------------------------------
    // POST /batch/gmail/v1   (multipart batch fetch)
    // -----------------------------------------------------------------

    /// Fetch many messages in a single HTTP round-trip via Gmail's
    /// batch endpoint. `ids` is automatically chunked into sub-batches
    /// of [`BATCH_CHUNK_SIZE`] (Gmail's per-batch limit). The order of
    /// returned messages matches the input IDs as closely as the
    /// batch parser can recover (keyed by Content-ID).
    ///
    /// Any individual sub-request that fails is reported in the
    /// caller-visible error channel (as a separate `Err` entry
    /// alongside the `Ok` messages) rather than aborting the whole
    /// batch; this mirrors lieer's tolerance for the occasional
    /// transient 5xx on a single message.
    pub fn batch_get_raw(&self, ids: &[&str]) -> Result<Vec<Result<BatchMessage>>> {
        let mut all_results: Vec<Result<BatchMessage>> = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(BATCH_CHUNK_SIZE) {
            let chunk_results = self.batch_get_raw_chunk(chunk)?;
            all_results.extend(chunk_results);
        }
        Ok(all_results)
    }

    fn batch_get_raw_chunk(&self, ids: &[&str]) -> Result<Vec<Result<BatchMessage>>> {
        let boundary = format!(
            "practiceforge_batch_{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let body = build_batch_request_body(&boundary, ids);
        let content_type = format!("multipart/mixed; boundary={boundary}");

        let resp = retry_with_backoff(|| {
            self.http
                .post(BATCH_URL)
                .bearer_auth(&self.access_token)
                .header(reqwest::header::CONTENT_TYPE, &content_type)
                .body(body.clone())
                .send()
                .context("POST /batch/gmail/v1")
        })?;

        let status = resp.status();
        let response_content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .to_string();
        let response_body = resp.text().context("reading batch response body")?;
        if !status.is_success() {
            return Err(http_error(status, &response_body).into());
        }

        let response_boundary = parse_boundary(&response_content_type)
            .ok_or_else(|| anyhow!("no boundary in batch response Content-Type: {response_content_type:?}"))?;
        Ok(parse_batch_response(&response_body, &response_boundary))
    }

    // -----------------------------------------------------------------
    // GET /users/me/history
    // -----------------------------------------------------------------

    /// Fetch one page of history events since `start_history_id`.
    /// Returns [`GmailApiError::HistoryIdExpired`] on 404 so the
    /// caller can trigger a full resync.
    pub fn list_history(
        &self,
        start_history_id: &str,
        page_token: Option<&str>,
    ) -> Result<HistoryResponse> {
        let mut url = format!(
            "{API_BASE}/users/me/history?startHistoryId={}",
            urlencoding::encode(start_history_id)
        );
        if let Some(t) = page_token {
            url.push_str(&format!("&pageToken={}", urlencoding::encode(t)));
        }
        let resp = retry_with_backoff(|| {
            self.http
                .get(&url)
                .bearer_auth(&self.access_token)
                .send()
                .context("GET /users/me/history")
        })?;
        let status = resp.status();
        let body = resp.text().context("reading history.list body")?;
        if status.as_u16() == 404 {
            return Err(GmailApiError::HistoryIdExpired.into());
        }
        if !status.is_success() {
            return Err(http_error(status, &body).into());
        }
        serde_json::from_str(&body)
            .context("parsing history.list JSON")
            .map_err(Into::into)
    }

    // -----------------------------------------------------------------
    // GET /users/me/labels
    // -----------------------------------------------------------------

    /// List every label in the account with its opaque ID + visible
    /// name. Used by the cleanup tool to resolve "new" /
    /// "curator-*-seen" strings to Gmail label IDs.
    pub fn list_all_labels(&self) -> Result<Vec<GmailLabel>> {
        let url = format!("{API_BASE}/users/me/labels");
        let resp = retry_with_backoff(|| {
            self.http
                .get(&url)
                .bearer_auth(&self.access_token)
                .send()
                .context("GET /users/me/labels")
        })?;
        let status = resp.status();
        let body = resp.text().context("reading labels.list body")?;
        if !status.is_success() {
            return Err(http_error(status, &body).into());
        }

        #[derive(Deserialize)]
        struct Resp {
            #[serde(default)]
            labels: Vec<GmailLabel>,
        }
        let parsed: Resp = serde_json::from_str(&body).context("parsing labels.list JSON")?;
        Ok(parsed.labels)
    }

    // -----------------------------------------------------------------
    // GET /users/me/messages?labelIds=X   (list by label ID)
    // -----------------------------------------------------------------

    /// Fetch ONE page of message IDs by label. Returns (ids,
    /// next_page_token). Caller is responsible for loop-until-
    /// next=None. Kept as a public primitive so callers can stream
    /// process-one-page-at-a-time rather than allocate the full
    /// result up front — important for large labels (e.g. `new`
    /// with 60k+ messages) where timeouts on a late page would
    /// throw away the collected-so-far result.
    pub fn list_message_ids_by_label_page(
        &self,
        label_id: &str,
        page_token: Option<&str>,
    ) -> Result<(Vec<String>, Option<String>)> {
        let mut url = format!(
            "{API_BASE}/users/me/messages?maxResults=500&labelIds={}",
            urlencoding::encode(label_id)
        );
        if let Some(t) = page_token {
            url.push_str(&format!("&pageToken={}", urlencoding::encode(t)));
        }
        let resp = retry_with_backoff(|| {
            self.http
                .get(&url)
                .bearer_auth(&self.access_token)
                .send()
                .context("GET /users/me/messages?labelIds=")
        })?;
        let status = resp.status();
        let body = resp.text().context("reading messages.list (by label) body")?;
        if !status.is_success() {
            return Err(http_error(status, &body).into());
        }
        let parsed: MessageListResponse =
            serde_json::from_str(&body).context("parsing messages.list (by label) JSON")?;
        let ids: Vec<String> = parsed.messages.into_iter().map(|m| m.id).collect();
        Ok((ids, parsed.next_page_token))
    }

    /// Paginate `users.messages.list` by `labelIds` (not `q=`).
    /// Returns all message IDs carrying the given label. `progress`
    /// is called with the cumulative count after each page.
    ///
    /// ⚠️ For large labels, prefer the streaming
    /// [`list_message_ids_by_label_page`] + loop pattern: this
    /// function holds every ID in memory and is not resumable if
    /// a late page errors.
    pub fn list_all_message_ids_by_label<F: FnMut(usize)>(
        &self,
        label_id: &str,
        mut progress: F,
    ) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let (ids, next) = self.list_message_ids_by_label_page(label_id, page_token.as_deref())?;
            out.extend(ids);
            progress(out.len());
            match next {
                Some(t) => page_token = Some(t),
                None => break,
            }
        }
        Ok(out)
    }

    // -----------------------------------------------------------------
    // POST /users/me/messages/batchModify
    // -----------------------------------------------------------------

    /// Add and/or remove labels from up to 1000 messages in a single
    /// API call. `ids` is chunked if longer than 1000. 50 quota units
    /// per sub-call regardless of `ids.len()`, so this is enormously
    /// cheaper than per-message modify calls for bulk operations.
    pub fn batch_modify(
        &self,
        ids: &[&str],
        add_label_ids: &[&str],
        remove_label_ids: &[&str],
    ) -> Result<()> {
        const BATCH_MODIFY_CHUNK: usize = 1000;
        for chunk in ids.chunks(BATCH_MODIFY_CHUNK) {
            self.batch_modify_chunk(chunk, add_label_ids, remove_label_ids)?;
        }
        Ok(())
    }

    fn batch_modify_chunk(
        &self,
        ids: &[&str],
        add_label_ids: &[&str],
        remove_label_ids: &[&str],
    ) -> Result<()> {
        #[derive(Serialize)]
        struct Body<'a> {
            ids: Vec<&'a str>,
            #[serde(rename = "addLabelIds", skip_serializing_if = "Vec::is_empty")]
            add: Vec<&'a str>,
            #[serde(rename = "removeLabelIds", skip_serializing_if = "Vec::is_empty")]
            remove: Vec<&'a str>,
        }
        let body = Body {
            ids: ids.to_vec(),
            add: add_label_ids.to_vec(),
            remove: remove_label_ids.to_vec(),
        };
        let url = format!("{API_BASE}/users/me/messages/batchModify");
        let resp = retry_with_backoff(|| {
            self.http
                .post(&url)
                .bearer_auth(&self.access_token)
                .json(&body)
                .send()
                .context("POST /users/me/messages/batchModify")
        })?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let resp_body = resp.text().unwrap_or_default();
        Err(http_error(status, &resp_body).into())
    }

    // -----------------------------------------------------------------
    // DELETE /users/me/labels/{id}
    // -----------------------------------------------------------------

    /// Permanently delete a label from the account. Messages that
    /// had the label simply lose it (they are NOT deleted). System
    /// labels (INBOX, UNREAD, etc.) cannot be deleted — Gmail returns
    /// 400 and this method bubbles that up.
    pub fn delete_label(&self, label_id: &str) -> Result<()> {
        let url = format!(
            "{API_BASE}/users/me/labels/{}",
            urlencoding::encode(label_id)
        );
        let resp = retry_with_backoff(|| {
            self.http
                .delete(&url)
                .bearer_auth(&self.access_token)
                .send()
                .context("DELETE /users/me/labels/{id}")
        })?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let body = resp.text().unwrap_or_default();
        Err(http_error(status, &body).into())
    }
}

/// A Gmail label record as returned by `users.labels.list`. We care
/// about `id` (for API calls) and `name` (for human-readable
/// matching); the rest of the schema (type, visibility hints, etc.)
/// is ignored for now.
#[derive(Debug, Deserialize, Clone)]
pub struct GmailLabel {
    pub id: String,
    pub name: String,
    #[serde(rename = "type", default)]
    pub label_type: Option<String>,
}

// -----------------------------------------------------------------
// Internal helpers — parsing + HTTP error mapping
// -----------------------------------------------------------------

/// Convert an HTTP status + body into either our typed error
/// ([`GmailApiError::RateLimit`]) or the catch-all `Http` variant.
fn http_error(status: reqwest::StatusCode, body: &str) -> GmailApiError {
    if status.as_u16() == 429 {
        return GmailApiError::RateLimit { retry_after: None };
    }
    // Gmail sometimes returns 403 with `userRateLimitExceeded` in the
    // body rather than a proper 429. Pattern-match for that.
    if status.as_u16() == 403
        && (body.contains("userRateLimitExceeded") || body.contains("rateLimitExceeded"))
    {
        return GmailApiError::RateLimit { retry_after: None };
    }
    GmailApiError::Http {
        status: status.as_u16(),
        body: body.to_string(),
    }
}

/// Parse a single `messages.get?format=raw` JSON body into a
/// [`BatchMessage`]. Used both by [`GmailApi::get_message_raw`] and
/// by the batch-response parser.
fn parse_raw_message_json(json: &str) -> Result<BatchMessage> {
    #[derive(Deserialize)]
    struct Shape {
        id: String,
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(default, rename = "labelIds")]
        label_ids: Vec<String>,
        /// base64url-encoded RFC-5322 message
        raw: String,
    }
    let s: Shape = serde_json::from_str(json).context("parsing raw-message JSON")?;
    // Gmail returns raw as URL-safe base64, possibly unpadded.
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let raw_bytes = URL_SAFE_NO_PAD
        .decode(s.raw.trim_end_matches('='))
        .context("decoding base64url raw message body")?;
    Ok(BatchMessage {
        gmail_id: s.id,
        raw_bytes,
        label_ids: s.label_ids,
        thread_id: s.thread_id,
    })
}

/// Extract the `boundary=` parameter from a `Content-Type:
/// multipart/mixed; boundary=...` header. Handles the quoted and
/// unquoted form.
fn parse_boundary(content_type: &str) -> Option<String> {
    let needle = "boundary=";
    let start = content_type.find(needle)? + needle.len();
    let rest = &content_type[start..];
    let rest = rest.trim_start_matches('"');
    let end = rest
        .find(|c: char| c == '"' || c == ';' || c == ' ')
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// Build the multipart/mixed request body for a batch of GET
/// `messages.get?format=raw` sub-requests. Content-ID values are
/// sequential integers so we can recover ordering in the response
/// even if Gmail reorders parts.
fn build_batch_request_body(boundary: &str, ids: &[&str]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ids.len() * 256);
    for (i, id) in ids.iter().enumerate() {
        out.extend_from_slice(b"--");
        out.extend_from_slice(boundary.as_bytes());
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(b"Content-Type: application/http\r\n");
        out.extend_from_slice(format!("Content-ID: <item-{i}>\r\n").as_bytes());
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(
            format!("GET /gmail/v1/users/me/messages/{id}?format=raw\r\n\r\n").as_bytes(),
        );
    }
    out.extend_from_slice(b"--");
    out.extend_from_slice(boundary.as_bytes());
    out.extend_from_slice(b"--\r\n");
    out
}

/// Parse the multipart/mixed response from the batch endpoint into
/// one result per sub-request. Order-preserving by iterating the
/// multipart body; each part's inner HTTP response is parsed for its
/// JSON body, which is then handed to [`parse_raw_message_json`].
pub(crate) fn parse_batch_response(
    body: &str,
    boundary: &str,
) -> Vec<Result<BatchMessage>> {
    let mut results: Vec<Result<BatchMessage>> = Vec::new();
    let delimiter = format!("--{boundary}");
    let terminator = format!("--{boundary}--");

    for part in body.split(&delimiter).skip(1) {
        let part = part.trim_start_matches("\r\n").trim_start_matches('\n');
        if part.trim() == "--" || part.starts_with("--") {
            // terminator
            let _ = terminator; // silence unused warning if only delimiter present
            break;
        }
        // Split the part into its multipart headers (application/http,
        // Content-ID, etc.) and the inner HTTP response body.
        let Some(inner_start) = part.find("\r\n\r\n") else {
            continue;
        };
        let inner = &part[inner_start + 4..];

        // The inner payload is itself an HTTP response: status line,
        // headers, blank line, body. Skip past headers to the body.
        let Some(body_start) = inner.find("\r\n\r\n") else {
            // No body — likely a sub-request error with no payload.
            results.push(Err(anyhow!(
                "batch sub-response has no body: {}",
                inner.lines().next().unwrap_or("")
            )));
            continue;
        };

        let sub_status_line = inner.lines().next().unwrap_or("");
        let sub_body = &inner[body_start + 4..];
        // Trim any trailing CRLF before the next boundary.
        let sub_body = sub_body.trim_end_matches("\r\n").trim_end();

        if !sub_status_line.contains(" 200 ") {
            results.push(Err(anyhow!(
                "batch sub-request failed: {sub_status_line}: {}",
                sub_body.chars().take(200).collect::<String>()
            )));
            continue;
        }

        results.push(parse_raw_message_json(sub_body));
    }

    results
}

// -----------------------------------------------------------------
// Retry helper — exponential backoff on 429/5xx
// -----------------------------------------------------------------

/// Run `f` with exponential backoff on transient errors. Retries on
/// connection-level errors, 429, and 5xx. Gives up after 6 tries
/// (~63s cumulative wait) and surfaces the last error.
pub fn retry_with_backoff<F>(mut f: F) -> Result<reqwest::blocking::Response>
where
    F: FnMut() -> Result<reqwest::blocking::Response>,
{
    const MAX_TRIES: u32 = 6;
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..MAX_TRIES {
        match f() {
            Ok(resp) => {
                let s = resp.status().as_u16();
                if s == 429 || (500..=599).contains(&s) {
                    // Transient — back off and retry, losing this
                    // response so the caller never sees a 429/5xx.
                    last_err = Some(anyhow!(
                        "transient Gmail HTTP {s} (attempt {})",
                        attempt + 1
                    ));
                } else {
                    return Ok(resp);
                }
            }
            Err(e) => {
                last_err = Some(e);
            }
        }
        if attempt + 1 < MAX_TRIES {
            let delay_ms = backoff_ms(attempt);
            std::thread::sleep(Duration::from_millis(delay_ms));
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("retry_with_backoff: no attempts ran")))
}

/// `min((2^n) * 1000 + jitter, 64_000)` milliseconds. Simple and
/// sufficient — Gmail's 429 is rare enough in our workload that
/// more sophisticated scheduling isn't worth the code.
fn backoff_ms(attempt: u32) -> u64 {
    let base = 1000u64.saturating_mul(2u64.saturating_pow(attempt));
    // Jitter 0..1000 ms — keyed off the system clock rather than a
    // full PRNG dependency, which is sufficient for decorrelating
    // retries from parallel tasks.
    let jitter = (chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64) % 1000;
    (base.saturating_add(jitter)).min(64_000)
}

// -----------------------------------------------------------------
// Tests — pure parsers against fixtures, no network
// -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE_FIXTURE: &str = r#"{
        "emailAddress": "test@example.com",
        "messagesTotal": 12345,
        "threadsTotal": 9876,
        "historyId": "1234567"
    }"#;

    const MESSAGES_LIST_FIXTURE: &str = r#"{
        "messages": [
            {"id": "abc123", "threadId": "t1"},
            {"id": "def456", "threadId": "t2"}
        ],
        "nextPageToken": "xyz",
        "resultSizeEstimate": 2
    }"#;

    const MESSAGES_LIST_LAST_PAGE: &str = r#"{
        "messages": [{"id": "ghi789", "threadId": "t3"}]
    }"#;

    const RAW_MESSAGE_FIXTURE: &str = r#"{
        "id": "abc123",
        "threadId": "thread_1",
        "labelIds": ["INBOX", "UNREAD", "Label_1234"],
        "raw": "RnJvbTogYWxpY2VAZXhhbXBsZS5jb20NClN1YmplY3Q6IGhlbGxvDQoNCmhpIQ"
    }"#;

    const HISTORY_FIXTURE: &str = r#"{
        "history": [
            {
                "id": "100",
                "messagesAdded": [
                    {"message": {"id": "new1", "threadId": "t1", "labelIds": ["INBOX"]}}
                ]
            },
            {
                "id": "101",
                "labelsAdded": [
                    {"message": {"id": "existing1", "threadId": "t2", "labelIds": ["INBOX", "UNREAD"]}, "labelIds": ["STARRED"]}
                ]
            }
        ],
        "historyId": "102"
    }"#;

    #[test]
    fn parses_profile() {
        let p: Profile = serde_json::from_str(PROFILE_FIXTURE).unwrap();
        assert_eq!(p.email_address, "test@example.com");
        assert_eq!(p.messages_total, 12345);
        assert_eq!(p.history_id, "1234567");
    }

    #[test]
    fn parses_messages_list_with_page_token() {
        let r: MessageListResponse = serde_json::from_str(MESSAGES_LIST_FIXTURE).unwrap();
        assert_eq!(r.messages.len(), 2);
        assert_eq!(r.messages[0].id, "abc123");
        assert_eq!(r.next_page_token.as_deref(), Some("xyz"));
    }

    #[test]
    fn parses_messages_list_final_page() {
        let r: MessageListResponse = serde_json::from_str(MESSAGES_LIST_LAST_PAGE).unwrap();
        assert_eq!(r.messages.len(), 1);
        assert!(r.next_page_token.is_none());
    }

    #[test]
    fn parses_raw_message_and_decodes_body() {
        let m = parse_raw_message_json(RAW_MESSAGE_FIXTURE).unwrap();
        assert_eq!(m.gmail_id, "abc123");
        assert_eq!(m.thread_id, "thread_1");
        assert_eq!(m.label_ids, vec!["INBOX", "UNREAD", "Label_1234"]);
        let text = String::from_utf8(m.raw_bytes).unwrap();
        assert!(text.contains("From: alice@example.com"));
        assert!(text.contains("Subject: hello"));
    }

    #[test]
    fn parses_history_with_added_and_labels() {
        let r: HistoryResponse = serde_json::from_str(HISTORY_FIXTURE).unwrap();
        assert_eq!(r.history_id, "102");
        assert_eq!(r.history.len(), 2);
        assert_eq!(r.history[0].messages_added.len(), 1);
        assert_eq!(r.history[0].messages_added[0].message.id, "new1");
        assert_eq!(r.history[1].labels_added.len(), 1);
        assert_eq!(r.history[1].labels_added[0].label_ids, vec!["STARRED"]);
    }

    #[test]
    fn boundary_extraction_handles_quoted_and_unquoted() {
        assert_eq!(
            parse_boundary("multipart/mixed; boundary=xyz").as_deref(),
            Some("xyz")
        );
        assert_eq!(
            parse_boundary(r#"multipart/mixed; boundary="xyz""#).as_deref(),
            Some("xyz")
        );
        assert_eq!(
            parse_boundary("multipart/mixed; boundary=xyz; charset=utf-8").as_deref(),
            Some("xyz")
        );
    }

    #[test]
    fn batch_request_body_format_matches_spec() {
        let body = build_batch_request_body("BOUND", &["id-a", "id-b"]);
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("--BOUND\r\n"));
        assert!(text.contains("Content-Type: application/http\r\n"));
        assert!(text.contains("Content-ID: <item-0>"));
        assert!(text.contains("Content-ID: <item-1>"));
        assert!(text.contains("GET /gmail/v1/users/me/messages/id-a?format=raw"));
        assert!(text.contains("GET /gmail/v1/users/me/messages/id-b?format=raw"));
        assert!(text.ends_with("--BOUND--\r\n"));
    }

    #[test]
    fn batch_response_parser_extracts_messages_and_errors() {
        // Simulated response containing one 200 and one 404.
        let body = "--BOUND\r\n\
            Content-Type: application/http\r\n\
            Content-ID: <response-item-0>\r\n\
            \r\n\
            HTTP/1.1 200 OK\r\n\
            Content-Type: application/json\r\n\
            \r\n\
            {\"id\":\"abc\",\"threadId\":\"t1\",\"labelIds\":[\"INBOX\"],\"raw\":\"aGk\"}\r\n\
            --BOUND\r\n\
            Content-Type: application/http\r\n\
            Content-ID: <response-item-1>\r\n\
            \r\n\
            HTTP/1.1 404 Not Found\r\n\
            Content-Type: application/json\r\n\
            \r\n\
            {\"error\":{\"code\":404}}\r\n\
            --BOUND--\r\n";
        let results = parse_batch_response(body, "BOUND");
        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        let msg = results[0].as_ref().unwrap();
        assert_eq!(msg.gmail_id, "abc");
        assert!(results[1].is_err());
    }

    #[test]
    fn http_error_maps_429_to_ratelimit() {
        let e = http_error(reqwest::StatusCode::TOO_MANY_REQUESTS, "");
        assert!(matches!(e, GmailApiError::RateLimit { .. }));
    }

    #[test]
    fn http_error_maps_403_ratelimit_body_to_ratelimit() {
        let e = http_error(
            reqwest::StatusCode::FORBIDDEN,
            r#"{"error":{"errors":[{"reason":"userRateLimitExceeded"}]}}"#,
        );
        assert!(matches!(e, GmailApiError::RateLimit { .. }));
    }

    #[test]
    fn http_error_maps_generic_403_to_http() {
        let e = http_error(reqwest::StatusCode::FORBIDDEN, r#"{"error":"nope"}"#);
        assert!(matches!(e, GmailApiError::Http { .. }));
    }

    #[test]
    fn backoff_ms_grows_exponentially_and_caps() {
        assert!(backoff_ms(0) >= 1000 && backoff_ms(0) <= 2000);
        assert!(backoff_ms(1) >= 2000 && backoff_ms(1) <= 3000);
        assert!(backoff_ms(2) >= 4000 && backoff_ms(2) <= 5000);
        assert!(backoff_ms(10) == 64_000); // capped
    }
}
