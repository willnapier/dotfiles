//! Microsoft 365 OAuth2 device-flow + refresh — IMAP/SMTP scopes.
//!
//! Parallel to [`crate::email::m365_oauth`] but registers a different OAuth
//! client and asks for Outlook IMAP/SMTP scopes rather than Graph. Reason:
//! Microsoft has two separate authorisation surfaces for M365 mail.
//! The Graph `sendMail` endpoint uses `https://graph.microsoft.com/*`
//! scopes and accepts the Microsoft Graph CLI public client ID. IMAP /
//! SMTP (via the long-standing `outlook.office365.com` endpoints) need
//! `https://outlook.office.com/IMAP.AccessAsUser.All` +
//! `https://outlook.office.com/SMTP.Send` and a client that is registered
//! to request them. Thunderbird's well-known public client ID satisfies
//! that — the same ID isync + neomutt communities use for XOAUTH2-
//! capable IMAP against M365. No tenant admin action required.
//!
//! Tokens land in the OS keystore under a distinct label set so neither
//! refresh loop can clobber the other: IMAP tokens under
//! `cohs-m365-imap-*`, Graph tokens stay under `cohs-m365-graph-*`.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

/// Thunderbird's registered public client ID. Has IMAP.AccessAsUser.All +
/// SMTP.Send on outlook.office.com. Widely used by mbsync/isync, neomutt,
/// aerc, himalaya etc. for personal M365 IMAP access. Treat as a public
/// constant — the value is in every OSS MUA's source tree.
pub const IMAP_CLIENT_ID: &str = "08162f7c-0fd2-4200-a84a-f25a4db0b584";

const DEVICECODE_URL: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/devicecode";
const TOKEN_URL: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";

/// Outlook IMAP + SMTP, plus `offline_access` for a refresh_token.
const SCOPES: &str = "https://outlook.office.com/IMAP.AccessAsUser.All \
                      https://outlook.office.com/SMTP.Send \
                      offline_access";

pub(crate) const KEYCHAIN_SERVICE: &str = "himalaya-cli";
pub(crate) const KEY_ACCESS: &str = "cohs-m365-imap-access";
pub(crate) const KEY_REFRESH: &str = "cohs-m365-imap-refresh";
pub(crate) const KEY_EXPIRES_AT: &str = "cohs-m365-imap-expires-at";

#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceFlowStart {
    pub user_code: String,
    pub verification_uri: String,
    pub device_code: String,
    pub expires_in: u64,
    pub interval: u64,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum PollResult {
    Pending,
    Complete,
    Error { message: String },
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
    #[serde(default)]
    error_description: String,
}

/// Kick off a device-code flow: returns the user_code/verification_uri
/// the user enters in a browser + the device_code to pass to [`poll`].
pub fn begin() -> Result<DeviceFlowStart> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("building HTTP client for IMAP device-code begin")?;

    let params = [("client_id", IMAP_CLIENT_ID), ("scope", SCOPES)];

    let resp = client
        .post(DEVICECODE_URL)
        .form(&params)
        .send()
        .context("POST /devicecode (IMAP scopes)")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(anyhow!(
            "/devicecode returned HTTP {status}: {body}"
        ));
    }

    resp.json::<DeviceFlowStart>()
        .context("parsing /devicecode response (IMAP scopes)")
}

/// Poll the /token endpoint with a device_code. Returns `Pending` while
/// the user is still authorising, `Complete` on success (tokens persisted
/// to keystore), `Error` on terminal failure.
pub fn poll(device_code: &str) -> Result<PollResult> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("building HTTP client for IMAP token poll")?;

    let params = [
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ("client_id", IMAP_CLIENT_ID),
        ("device_code", device_code),
    ];

    let resp = client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .context("POST /token during IMAP device-flow poll")?;

    let status = resp.status();
    let body = resp.text().context("reading /token response body")?;

    if status.is_success() {
        let tokens: TokenResponse = serde_json::from_str(&body)
            .context("parsing successful /token response (IMAP)")?;
        store_tokens(&tokens)?;
        return Ok(PollResult::Complete);
    }

    let err: TokenError = serde_json::from_str(&body)
        .context("parsing /token error response (IMAP)")?;

    match err.error.as_str() {
        "authorization_pending" | "slow_down" => Ok(PollResult::Pending),
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

/// Refresh the IMAP/SMTP access token using the stored refresh_token.
pub fn refresh() -> Result<()> {
    let refresh_token = keychain_get(KEY_REFRESH)?.ok_or_else(|| {
        anyhow!(
            "no IMAP refresh token in keychain — run `practiceforge email imap-init --account cohs` first"
        )
    })?;

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .context("building HTTP client for IMAP refresh")?;

    let params = [
        ("grant_type", "refresh_token"),
        ("client_id", IMAP_CLIENT_ID),
        ("refresh_token", refresh_token.as_str()),
        ("scope", SCOPES),
    ];

    let resp = client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .context("POST /token during IMAP refresh")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(anyhow!("IMAP refresh failed: HTTP {status}: {body}"));
    }

    let tokens: TokenResponse = resp
        .json()
        .context("parsing refreshed IMAP token response")?;
    store_tokens(&tokens)
}

/// Drive the device-code flow synchronously on the terminal — print the
/// instructions, poll until completion. Used by the `imap-init` CLI.
pub fn run_device_flow_interactive() -> Result<()> {
    let start = begin()?;
    eprintln!("\nTo authorise IMAP/SMTP access for COHS:");
    eprintln!("\n  1. Open: {}", start.verification_uri);
    eprintln!("  2. Enter code: {}", start.user_code);
    eprintln!("\n(This window will poll every {}s and complete automatically.)", start.interval);

    let started = std::time::Instant::now();
    let max = std::time::Duration::from_secs(start.expires_in.max(60));
    let interval = std::time::Duration::from_secs(start.interval.max(1));

    loop {
        std::thread::sleep(interval);
        match poll(&start.device_code)? {
            PollResult::Pending => {
                eprint!(".");
            }
            PollResult::Complete => {
                eprintln!("\n✓ COHS IMAP tokens acquired and stored in keychain.");
                return Ok(());
            }
            PollResult::Error { message } => {
                return Err(anyhow!("device-code flow failed: {message}"));
            }
        }
        if started.elapsed() > max {
            return Err(anyhow!(
                "device-code flow timed out after {}s without completion",
                max.as_secs()
            ));
        }
    }
}

/// Print a fresh IMAP access token to stdout, refreshing first if needed.
/// Used as mbsync `PassCmd`: the printed token is consumed directly as
/// the SASL XOAUTH2 bearer.
pub fn show() -> Result<()> {
    // Refresh proactively when we can — the launchd timer calls us every
    // 30 min so usually this is a no-op, but belt-and-braces for the
    // path where the token expired between ticks. If there's no refresh
    // token yet, don't fail here — fall through and let the keychain
    // read give the more user-actionable "run imap-init" message.
    let _ = refresh();
    let token = keychain_get(KEY_ACCESS)?.ok_or_else(|| {
        anyhow!(
            "no IMAP access token in keychain — run `practiceforge email imap-init --account cohs` first"
        )
    })?;
    println!("{}", token);
    Ok(())
}

fn store_tokens(tokens: &TokenResponse) -> Result<()> {
    keychain_set(KEY_ACCESS, &tokens.access_token)
        .context("storing IMAP access_token in keychain")?;
    if let Some(rt) = &tokens.refresh_token {
        keychain_set(KEY_REFRESH, rt)
            .context("storing IMAP refresh_token in keychain")?;
    }
    if let Some(secs) = tokens.expires_in {
        let expires_at = chrono::Utc::now().timestamp() + secs as i64;
        keychain_set(KEY_EXPIRES_AT, &expires_at.to_string())
            .context("storing IMAP expires_at in keychain")?;
    }
    Ok(())
}

fn keychain_set(account: &str, value: &str) -> Result<()> {
    crate::keystore::set(KEYCHAIN_SERVICE, account, value)
        .with_context(|| format!("storing {account} in keystore (IMAP)"))
}

fn keychain_get(account: &str) -> Result<Option<String>> {
    crate::keystore::get(KEYCHAIN_SERVICE, account)
        .with_context(|| format!("reading {account} from keystore (IMAP)"))
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
        let test_account = "test-pf-m365-imap-oauth";
        let _ = crate::keystore::delete(KEYCHAIN_SERVICE, test_account);

        keychain_set(test_account, "secret-imap-token").expect("write");
        let got = keychain_get(test_account).expect("read");
        assert_eq!(got.as_deref(), Some("secret-imap-token"));

        let _ = crate::keystore::delete(KEYCHAIN_SERVICE, test_account);
    }
}
