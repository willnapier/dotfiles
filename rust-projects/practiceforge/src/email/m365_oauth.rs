//! Microsoft 365 OAuth2 device-flow + token refresh — pure Rust.
//!
//! Used by the admin dashboard's "Add Microsoft 365 account" button to
//! perform the full OAuth flow server-side (no Python dependency, no msal
//! cache, no subprocess). Tokens land in the OS keychain under the same
//! label convention cohs-oauth-graph uses, so any consumer reading
//! `cohs-m365-graph-access` from keychain (e.g. `GraphTransport` via
//! `CommandTokenSource`) works unchanged.
//!
//! Scope is deliberately narrow — this module is Graph-only. IMAP/SMTP
//! Outlook scopes stay with the `cohs-oauth` Python helper for CLI use.
//!
//! ## Flow
//!
//! 1. [`begin`] → POST `/devicecode`, returns a user_code + verification
//!    URL + device_code. Caller (web UI) displays code+URL to user; user
//!    authenticates in their browser.
//! 2. [`poll`] → POST `/token` with device_code. Returns `Pending` while
//!    user is still authenticating, `Complete` once tokens issued,
//!    `Error(msg)` on denial / expiry / other failure.
//! 3. [`refresh`] → POST `/token` with stored refresh_token. Called from
//!    systemd/launchd timer to keep access_token fresh.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

/// Microsoft Graph CLI public client ID. Tenant-agnostic, registered for
/// Graph resources. See `project_cohs_m365_auth.md` memory for why.
pub const GRAPH_CLIENT_ID: &str = "14d82eec-204b-4c2f-b7e8-296a70dab67e";

const DEVICECODE_URL: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/devicecode";
const TOKEN_URL: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";

/// Scopes for Graph sendMail. `offline_access` gets us a refresh_token.
const SCOPES: &str = "https://graph.microsoft.com/Mail.Send offline_access";

/// Keychain/libsecret service under which all `cohs-oauth-graph` tokens
/// live. Shared with the Python helper — a Rust-acquired entry is read by
/// the Python helper and vice-versa.
pub(crate) const KEYCHAIN_SERVICE: &str = "himalaya-cli";
/// Account name for the access token. JWT, ~60–90 min lifetime.
pub(crate) const KEY_ACCESS: &str = "cohs-m365-graph-access";
/// Account name for the refresh token. Long-lived (60–90 days inactive).
pub(crate) const KEY_REFRESH: &str = "cohs-m365-graph-refresh";
/// Account name for the access-token absolute Unix expiry timestamp,
/// stored as a base-10 string. Computed at write time as `now() +
/// expires_in`; `None` from Microsoft's response → no entry written
/// (legacy/uncertain entries trigger an immediate refresh on next read).
pub(crate) const KEY_EXPIRES_AT: &str = "cohs-m365-graph-expires-at";

/// What a begin call returns. All fields come straight from Microsoft's
/// devicecode endpoint except for `device_code`, which the caller stores
/// and passes back to [`poll`] to complete the flow.
#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceFlowStart {
    pub user_code: String,
    pub verification_uri: String,
    pub device_code: String,
    pub expires_in: u64,
    pub interval: u64,
    pub message: String,
}

/// Result of a poll cycle.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum PollResult {
    /// User hasn't completed browser sign-in yet. Caller should wait
    /// `interval` seconds (from [`DeviceFlowStart`]) and poll again.
    Pending,
    /// Tokens acquired and written to keychain. OAuth flow complete.
    Complete,
    /// Terminal failure — user denied, code expired, Microsoft rejected.
    /// Caller should stop polling and surface the message.
    Error { message: String },
}

/// Raw token response shape from Microsoft. Private to this module.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    /// Lifetime of `access_token` in seconds. Microsoft always returns
    /// this in practice (typically 3600–5400) but the spec marks it
    /// optional, so we treat absence as "expiry unknown — refresh on
    /// next read".
    expires_in: Option<u64>,
}

/// Raw error response shape. Private.
#[derive(Deserialize)]
struct TokenError {
    error: String,
    #[serde(default)]
    error_description: String,
}

/// Start a device-code flow. Call from the dashboard's begin endpoint.
pub fn begin() -> Result<DeviceFlowStart> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("building HTTP client for device-code begin")?;

    let params = [
        ("client_id", GRAPH_CLIENT_ID),
        ("scope", SCOPES),
    ];

    let resp = client
        .post(DEVICECODE_URL)
        .form(&params)
        .send()
        .context("POST /devicecode")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(anyhow!("/devicecode returned HTTP {status}: {body}"));
    }

    resp.json::<DeviceFlowStart>()
        .context("parsing /devicecode response")
}

/// Poll once for token completion. Call repeatedly from the dashboard's
/// poll endpoint with the `device_code` from [`begin`].
pub fn poll(device_code: &str) -> Result<PollResult> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("building HTTP client for token poll")?;

    let params = [
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ("client_id", GRAPH_CLIENT_ID),
        ("device_code", device_code),
    ];

    let resp = client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .context("POST /token during device-flow poll")?;

    let status = resp.status();
    let body = resp
        .text()
        .context("reading /token response body")?;

    if status.is_success() {
        let tokens: TokenResponse = serde_json::from_str(&body)
            .context("parsing successful /token response")?;
        store_tokens(&tokens)?;
        return Ok(PollResult::Complete);
    }

    // Not success — parse as error. Microsoft returns 400 for
    // authorization_pending, slow_down, expired_token, etc.
    let err: TokenError = serde_json::from_str(&body)
        .context("parsing /token error response")?;

    match err.error.as_str() {
        "authorization_pending" | "slow_down" => Ok(PollResult::Pending),
        // Anything else is terminal — user denied, code expired, etc.
        _ => Ok(PollResult::Error {
            message: format!(
                "{}: {}",
                err.error,
                if err.error_description.is_empty() {
                    "(no description)".to_string()
                } else {
                    err.error_description
                }
            ),
        }),
    }
}

/// Refresh access_token using the stored refresh_token. Called from the
/// systemd/launchd refresh timer (replaces `cohs-oauth-graph refresh` for
/// dashboard-acquired tokens).
///
/// Returns Ok(()) on success with keychain updated; Err if the refresh
/// token is missing / rejected (in which case the user must re-authorise
/// via the dashboard button).
pub fn refresh() -> Result<()> {
    let refresh_token = keychain_get(KEY_REFRESH)?
        .ok_or_else(|| anyhow!("no refresh token in keychain — run the M365 OAuth flow first"))?;

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("building HTTP client for refresh")?;

    let params = [
        ("grant_type", "refresh_token"),
        ("client_id", GRAPH_CLIENT_ID),
        ("refresh_token", refresh_token.as_str()),
        ("scope", SCOPES),
    ];

    let resp = client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .context("POST /token during refresh")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(anyhow!("refresh failed: HTTP {status}: {body}"));
    }

    let tokens: TokenResponse = resp
        .json()
        .context("parsing refreshed token response")?;
    store_tokens(&tokens)
}

// ---------------------------------------------------------------------------
// Keychain I/O — parallels the Python cohs-oauth-graph's storage layout so
// any tool that reads `cohs-m365-graph-access` / `cohs-m365-graph-refresh`
// works interchangeably with Python- and Rust-acquired tokens.
// ---------------------------------------------------------------------------

fn store_tokens(tokens: &TokenResponse) -> Result<()> {
    keychain_set(KEY_ACCESS, &tokens.access_token)
        .context("storing access_token in keychain")?;
    if let Some(rt) = &tokens.refresh_token {
        keychain_set(KEY_REFRESH, rt)
            .context("storing refresh_token in keychain")?;
    }
    // Compute and persist absolute expiry. Storing `now + expires_in`
    // (rather than `expires_in` itself) means later readers don't need
    // to know when the token was issued — they just compare to wall
    // clock. If `expires_in` is missing we deliberately leave any
    // existing expiry entry stale; the proactive refresh path treats
    // "no expiry" as "must refresh", which is the safe default.
    if let Some(secs) = tokens.expires_in {
        let expires_at = chrono::Utc::now().timestamp() + secs as i64;
        keychain_set(KEY_EXPIRES_AT, &expires_at.to_string())
            .context("storing expires_at in keychain")?;
    }
    Ok(())
}

fn keychain_set(account: &str, value: &str) -> Result<()> {
    crate::keystore::set(KEYCHAIN_SERVICE, account, value)
        .with_context(|| format!("storing {account} in keystore"))
}

fn keychain_get(account: &str) -> Result<Option<String>> {
    crate::keystore::get(KEYCHAIN_SERVICE, account)
        .with_context(|| format!("reading {account} from keystore"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_result_serializes_with_tagged_status() {
        let pending = serde_json::to_value(&PollResult::Pending).unwrap();
        assert_eq!(pending["status"], "pending");

        let complete = serde_json::to_value(&PollResult::Complete).unwrap();
        assert_eq!(complete["status"], "complete");

        let error = serde_json::to_value(&PollResult::Error {
            message: "denied".into(),
        })
        .unwrap();
        assert_eq!(error["status"], "error");
        assert_eq!(error["message"], "denied");
    }

    #[test]
    fn keychain_round_trip() {
        let test_account = "test-pf-m365-oauth";
        let _ = crate::keystore::delete(KEYCHAIN_SERVICE, test_account);

        keychain_set(test_account, "secret-value").expect("write");
        let got = keychain_get(test_account).expect("read");
        assert_eq!(got.as_deref(), Some("secret-value"));

        let _ = crate::keystore::delete(KEYCHAIN_SERVICE, test_account);
    }
}
