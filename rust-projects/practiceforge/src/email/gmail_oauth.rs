//! Gmail OAuth2 auth-code flow + token refresh — pure Rust.
//!
//! Parallels `m365_oauth.rs` but for Google Workspace / Gmail. Key
//! differences from the Microsoft side:
//!
//! - **Auth-code flow with loopback redirect**, not device code. Google's
//!   device-code endpoint rejects "Desktop" client types (Microsoft's
//!   equivalent is more permissive). Auth-code with `http://127.0.0.1:PORT`
//!   redirect works with the Desktop client type himalaya already has
//!   registered in the "himalaya" Google Cloud project. No new Google Cloud
//!   project registration required for the first-time William-local use.
//! - **SMTP backend**, not Graph. Gmail keeps serving SMTP with XOAUTH2;
//!   tokens from this module feed `SmtpTransport` in its `XOAuth2` mode.
//! - **Client secret required.** Google insists on `client_secret` in the
//!   `/token` exchange even for Desktop apps (unlike Microsoft public
//!   apps which accept PKCE-only). himalaya already stores the secret in
//!   keychain as `gmail-oauth2-client-secret`; we read it from there.
//!
//! Future scalability: when PracticeForge deploys to practice servers
//! accessed via browsers on other machines (Leigh, COHS colleagues), the
//! loopback redirect breaks because the callback arrives on the server
//! rather than the user's browser machine. At that point we register a
//! "TV and Limited Input" OAuth client in Google Cloud Console, switch to
//! device-code flow, and this module grows a second entry point. For now,
//! loopback is the right tradeoff.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Scope for sending via Gmail SMTP with XOAUTH2. Same scope himalaya uses.
const SCOPE: &str = "https://mail.google.com/";

const KEYCHAIN_SERVICE: &str = "himalaya-cli";
const KEY_ACCESS: &str = "gmail-pf-access";
const KEY_REFRESH: &str = "gmail-pf-refresh";
/// Himalaya stores its own Google OAuth client secret under this keychain
/// account. We reuse it so we don't need a second Google Cloud project
/// on this machine. On a machine without himalaya configured, the user
/// would register their own Desktop OAuth client and store its secret
/// under this same keychain entry name before first use.
const KEY_CLIENT_SECRET: &str = "gmail-oauth2-client-secret";

/// Default Google Cloud project client ID — himalaya's "himalaya" project,
/// Desktop-type OAuth 2.0 credentials. Published openly in
/// `~/dotfiles/himalaya/config.toml`; treating as a public constant is fine
/// because client secrets are the only sensitive half and they live in the
/// keychain, not the source.
///
/// Colleagues using PracticeForge against their own Google Workspace should
/// either register their own project or — eventually — PracticeForge ships
/// with a published "PracticeForge" Google Cloud project client ID usable
/// by any Google user (the Microsoft Graph CLI pattern, reversed).
pub const DEFAULT_CLIENT_ID: &str =
    "692443843049-ibibtduhua6nn7tsrqkar3fluffkkqqf.apps.googleusercontent.com";

/// State carried between `begin` (issues an auth URL) and the callback
/// handler (exchanges the returned code for tokens). One-off per flow.
#[derive(Clone)]
struct FlowState {
    code_verifier: String,
    redirect_uri: String,
    created: SystemTime,
    status: FlowStatus,
}

#[derive(Clone, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum FlowStatus {
    Pending,
    Complete,
    Error { message: String },
}

fn state_table() -> &'static Mutex<HashMap<String, FlowState>> {
    static TABLE: OnceLock<Mutex<HashMap<String, FlowState>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Response from [`begin`]: what the frontend needs to open a new browser
/// window at Google's auth page, and a state token the frontend uses to
/// poll for completion.
#[derive(Serialize)]
pub struct BeginResult {
    pub auth_url: String,
    pub state: String,
    pub redirect_uri: String,
}

/// Start an auth-code flow. Returns a Google auth URL to open in a browser
/// + a state token. The caller (dashboard frontend) opens the URL in a new
/// tab; once the user signs in and consents, Google redirects the tab to
/// `redirect_uri` with a code query param, which [`handle_callback`]
/// processes. The frontend polls [`poll_status`] to detect completion.
///
/// `redirect_uri` must be the public URL of the callback endpoint — for
/// local dashboard use `http://127.0.0.1:3457/api/email/gmail/callback`.
pub fn begin(redirect_uri: &str) -> Result<BeginResult> {
    let state = random_string(32);
    let code_verifier = random_string(64);
    let code_challenge = pkce_challenge(&code_verifier);

    // Remember what we need for the exchange. Garbage-collect old entries
    // while we hold the lock so the table doesn't grow unbounded.
    {
        let mut table = state_table().lock().unwrap();
        let now = SystemTime::now();
        table.retain(|_, v| {
            now.duration_since(v.created)
                .map(|d| d < Duration::from_secs(600))
                .unwrap_or(false)
        });
        table.insert(
            state.clone(),
            FlowState {
                code_verifier: code_verifier.clone(),
                redirect_uri: redirect_uri.to_string(),
                created: now,
                status: FlowStatus::Pending,
            },
        );
    }

    let auth_url = format!(
        "{AUTH_URL}?response_type=code\
        &client_id={}\
        &redirect_uri={}\
        &scope={}\
        &state={}\
        &code_challenge={}\
        &code_challenge_method=S256\
        &access_type=offline\
        &prompt=consent",
        urlencoding::encode(DEFAULT_CLIENT_ID),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(SCOPE),
        urlencoding::encode(&state),
        urlencoding::encode(&code_challenge),
    );

    Ok(BeginResult {
        auth_url,
        state,
        redirect_uri: redirect_uri.to_string(),
    })
}

/// Called by the HTTP callback handler when Google redirects back with a
/// code. Validates state, exchanges code for tokens, stores tokens in
/// keychain. Returns Ok(()) on success.
pub fn handle_callback(state: &str, code: &str) -> Result<()> {
    let flow = {
        let table = state_table().lock().unwrap();
        table
            .get(state)
            .cloned()
            .ok_or_else(|| anyhow!("unknown or expired state token — flow may have timed out"))?
    };

    let client_secret = keychain_get(KEY_CLIENT_SECRET)?.ok_or_else(|| {
        anyhow!(
            "Google OAuth client_secret missing in keychain (service='{}', account='{}'). \
             Either set up himalaya Gmail first (which stores it there), or add it manually \
             with `security add-generic-password -s himalaya-cli -a {} -w <secret>`.",
            KEYCHAIN_SERVICE,
            KEY_CLIENT_SECRET,
            KEY_CLIENT_SECRET
        )
    })?;

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .context("building HTTP client for Gmail token exchange")?;

    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", DEFAULT_CLIENT_ID),
        ("client_secret", client_secret.as_str()),
        ("redirect_uri", flow.redirect_uri.as_str()),
        ("code_verifier", flow.code_verifier.as_str()),
    ];

    let resp = client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .context("POST /token during Gmail auth-code exchange")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        mark_state_error(
            state,
            &format!("token exchange failed: HTTP {status}: {body}"),
        );
        return Err(anyhow!("Gmail token exchange failed: HTTP {status}: {body}"));
    }

    let tokens: TokenResponse = resp
        .json()
        .context("parsing Gmail token response")?;
    store_tokens(&tokens)?;
    mark_state_complete(state);
    Ok(())
}

/// Check the status of a flow. Frontend polls this with the state it
/// received from [`begin`]; when status flips to `Complete`, it moves to
/// the details form.
pub fn poll_status(state: &str) -> FlowStatus {
    let table = state_table().lock().unwrap();
    table
        .get(state)
        .map(|f| f.status.clone())
        .unwrap_or(FlowStatus::Error {
            message: "unknown state (flow expired or never started)".to_string(),
        })
}

/// Refresh access_token using the stored refresh_token. Called from the
/// refresh timer. Works whether tokens were acquired via the dashboard
/// or via some future CLI flow — only needs the refresh_token in keychain.
pub fn refresh() -> Result<()> {
    let refresh_token = keychain_get(KEY_REFRESH)?
        .ok_or_else(|| anyhow!("no Gmail refresh token in keychain — run the Gmail OAuth flow first"))?;
    let client_secret = keychain_get(KEY_CLIENT_SECRET)?.ok_or_else(|| {
        anyhow!("client_secret missing in keychain under '{}'", KEY_CLIENT_SECRET)
    })?;

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .context("building HTTP client for Gmail refresh")?;

    let params = [
        ("grant_type", "refresh_token"),
        ("client_id", DEFAULT_CLIENT_ID),
        ("client_secret", client_secret.as_str()),
        ("refresh_token", refresh_token.as_str()),
    ];

    let resp = client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .context("POST /token during Gmail refresh")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(anyhow!("Gmail refresh failed: HTTP {status}: {body}"));
    }

    let tokens: TokenResponse = resp.json().context("parsing Gmail refresh response")?;
    store_tokens(&tokens)
}

/// Print the current access token to stdout. Invoked by
/// `CommandTokenSource` ("practiceforge email gmail-show") from
/// `SmtpTransport` XOAUTH2 mode. Refreshes first so the printed token is
/// always fresh.
pub fn show() -> Result<()> {
    // Try to refresh; if no refresh_token yet, just print whatever access
    // token we have (and let the caller fail with a useful error if none).
    let _ = refresh();
    let token = keychain_get(KEY_ACCESS)?
        .ok_or_else(|| anyhow!("no Gmail access token in keychain — complete the OAuth flow first"))?;
    println!("{}", token);
    Ok(())
}

// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    #[allow(dead_code)]
    expires_in: Option<u64>,
    #[allow(dead_code)]
    token_type: Option<String>,
    #[allow(dead_code)]
    scope: Option<String>,
}

fn store_tokens(tokens: &TokenResponse) -> Result<()> {
    keychain_set(KEY_ACCESS, &tokens.access_token)
        .context("storing Gmail access_token in keychain")?;
    if let Some(rt) = &tokens.refresh_token {
        keychain_set(KEY_REFRESH, rt).context("storing Gmail refresh_token in keychain")?;
    }
    Ok(())
}

fn mark_state_complete(state: &str) {
    if let Ok(mut t) = state_table().lock() {
        if let Some(entry) = t.get_mut(state) {
            entry.status = FlowStatus::Complete;
        }
    }
}

fn mark_state_error(state: &str, msg: &str) {
    if let Ok(mut t) = state_table().lock() {
        if let Some(entry) = t.get_mut(state) {
            entry.status = FlowStatus::Error {
                message: msg.to_string(),
            };
        }
    }
}

fn random_string(len: usize) -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}

fn pkce_challenge(verifier: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(verifier.as_bytes());
    data_encoding::BASE64URL_NOPAD.encode(&digest)
}

fn keychain_set(account: &str, value: &str) -> Result<()> {
    if cfg!(target_os = "macos") {
        let output = Command::new("security")
            .args([
                "add-generic-password",
                "-U",
                "-s",
                KEYCHAIN_SERVICE,
                "-a",
                account,
                "-w",
                value,
            ])
            .output()
            .context("spawning `security add-generic-password`")?;
        if !output.status.success() {
            return Err(anyhow!(
                "keychain write failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    } else {
        use std::io::Write;
        let mut child = Command::new("secret-tool")
            .args([
                "store",
                "--label",
                account,
                "service",
                KEYCHAIN_SERVICE,
                "account",
                account,
            ])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .context("spawning `secret-tool store`")?;
        {
            let stdin = child
                .stdin
                .as_mut()
                .ok_or_else(|| anyhow!("no stdin on secret-tool"))?;
            stdin.write_all(value.as_bytes())?;
        }
        let status = child.wait().context("waiting for secret-tool")?;
        if !status.success() {
            return Err(anyhow!("secret-tool store exited with status {status}"));
        }
    }
    Ok(())
}

fn keychain_get(account: &str) -> Result<Option<String>> {
    let output = if cfg!(target_os = "macos") {
        Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                KEYCHAIN_SERVICE,
                "-a",
                account,
                "-w",
            ])
            .output()
            .context("spawning `security find-generic-password`")?
    } else {
        Command::new("secret-tool")
            .args([
                "lookup",
                "service",
                KEYCHAIN_SERVICE,
                "account",
                account,
            ])
            .output()
            .context("spawning `secret-tool lookup`")?
    };

    if !output.status.success() {
        return Ok(None);
    }
    let value = String::from_utf8(output.stdout)
        .context("keychain output not UTF-8")?
        .trim_end_matches(['\r', '\n'])
        .to_string();
    if value.is_empty() { Ok(None) } else { Ok(Some(value)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_spec() {
        // Vector from RFC 7636 Appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let expected = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert_eq!(pkce_challenge(verifier), expected);
    }

    #[test]
    fn begin_returns_a_state_and_auth_url() {
        let result = begin("http://127.0.0.1:3457/api/email/gmail/callback").unwrap();
        assert!(result.auth_url.contains("accounts.google.com"));
        assert!(result.auth_url.contains("code_challenge_method=S256"));
        assert!(result.auth_url.contains(DEFAULT_CLIENT_ID));
        assert!(!result.state.is_empty());
    }

    #[test]
    fn poll_status_unknown_state_is_error() {
        match poll_status("never-issued-this-state") {
            FlowStatus::Error { message } => assert!(message.contains("unknown")),
            other => panic!("expected Error, got {:?}", serde_json::to_value(&other).unwrap()),
        }
    }
}
