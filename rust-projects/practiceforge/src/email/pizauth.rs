//! pizauth integration helpers for the email setup wizard.
//!
//! Wraps the system-installed `pizauth` binary with the operations the
//! wizard needs:
//!
//! - **pre-flight check** — is pizauth installed and on PATH?
//! - **config bootstrap** — does the user's pizauth.conf have an account
//!   block for the identity we're about to set up? If not, append a
//!   templated block (idempotent — never rewrites existing blocks).
//! - **device-code orchestration** — kick off `pizauth refresh <account>`,
//!   poll `pizauth show <account>` until a token is available, with
//!   a sensible timeout.
//!
//! Why these helpers exist: the email setup wizard previously assumed
//! pizauth was already configured out-of-band. Colleagues running the
//! wizard fresh would walk through it, see "✓ setup complete", then
//! discover at first send that the OAuth flow had never run. This
//! module closes that gap.

use anyhow::{anyhow, bail, Context, Result};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

/// One pizauth account block, ready to render into pizauth.conf.
///
/// Public clients (Microsoft Thunderbird msal client for M365) have no
/// `client_secret`. Confidential clients (Google Cloud OAuth) require one.
#[derive(Debug, Clone)]
pub struct PizauthAccount {
    pub name: String,
    pub auth_uri: String,
    pub token_uri: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    pub login_hint: Option<String>,
    /// Optional comment lines printed above the `account "..." {` opener.
    /// Each entry becomes one `// ...` line. Helps colleagues understand
    /// what the block is for when reading their own config later.
    pub comments: Vec<String>,
}

impl PizauthAccount {
    /// Microsoft 365 Graph sendMail scope. Uses the Thunderbird msal
    /// public client (admin-consented in the COHS tenant for the
    /// `9e5f94bc-...` app — colleagues sharing the COHS tenant can use
    /// it unchanged). Non-COHS tenants would need their own admin-
    /// consented client; that's a v1.1 concern (see `Tenancy Posture`
    /// in architecture.md).
    pub fn cohs_graph_template(login_hint: &str) -> Self {
        Self {
            name: "cohs-graph".to_string(),
            auth_uri: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize".to_string(),
            token_uri: "https://login.microsoftonline.com/common/oauth2/v2.0/token".to_string(),
            client_id: "9e5f94bc-e8a4-4e73-b8be-63364c29d753".to_string(),
            client_secret: None,
            scopes: vec![
                "https://graph.microsoft.com/Mail.Send".to_string(),
                "offline_access".to_string(),
            ],
            login_hint: Some(login_hint.to_string()),
            comments: vec![
                "COHS Microsoft 365 — Graph sendMail scope. Public Thunderbird".to_string(),
                "msal client (admin-consented in COHS tenant). Used by graph-send".to_string(),
                "(meli, scripts) and practiceforge clinical mail.".to_string(),
            ],
        }
    }

    /// Microsoft 365 IMAP/SMTP scopes (Outlook), in case the tenant ever
    /// re-enables SMTP AUTH. Same client as Graph; different scope set.
    /// Optional — Graph alone covers send. IMAP needs this if the user
    /// wants mbsync to pull mail.
    pub fn cohs_imap_smtp_template(login_hint: &str) -> Self {
        Self {
            name: "cohs".to_string(),
            auth_uri: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize".to_string(),
            token_uri: "https://login.microsoftonline.com/common/oauth2/v2.0/token".to_string(),
            client_id: "9e5f94bc-e8a4-4e73-b8be-63364c29d753".to_string(),
            client_secret: None,
            scopes: vec![
                "https://outlook.office.com/IMAP.AccessAsUser.All".to_string(),
                "https://outlook.office.com/SMTP.Send".to_string(),
                "offline_access".to_string(),
            ],
            login_hint: Some(login_hint.to_string()),
            comments: vec![
                "COHS Microsoft 365 — Outlook IMAP/SMTP scopes. Optional;".to_string(),
                "needed only for mbsync IMAP pull. Send goes via cohs-graph.".to_string(),
            ],
        }
    }

    /// Gmail (Google Workspace or @gmail.com). Requires the user to
    /// supply their own Google Cloud OAuth client credentials — there's
    /// no shared "PracticeForge" Google client app at this stage. The
    /// wizard prompts for client_id and client_secret separately.
    pub fn gmail_template(login_hint: &str, client_id: String, client_secret: String) -> Self {
        Self {
            name: "gmail".to_string(),
            auth_uri: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
            token_uri: "https://www.googleapis.com/oauth2/v3/token".to_string(),
            client_id,
            client_secret: Some(client_secret),
            scopes: vec!["https://mail.google.com/".to_string()],
            login_hint: Some(login_hint.to_string()),
            comments: vec![
                "Gmail / Google Workspace — full mail.google.com scope".to_string(),
                "(IMAP read + SMTP send). Requires per-user Google Cloud OAuth".to_string(),
                "client (no shared PracticeForge client app exists yet).".to_string(),
            ],
        }
    }

    /// Render as a pizauth.conf block. Trailing newline included.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for comment in &self.comments {
            out.push_str("// ");
            out.push_str(comment);
            out.push('\n');
        }
        out.push_str(&format!("account \"{}\" {{\n", self.name));
        out.push_str(&format!("    auth_uri = \"{}\";\n", self.auth_uri));
        out.push_str(&format!("    token_uri = \"{}\";\n", self.token_uri));
        out.push_str(&format!("    client_id = \"{}\";\n", self.client_id));
        if let Some(secret) = &self.client_secret {
            out.push_str(&format!("    client_secret = \"{}\";\n", secret));
        }
        out.push_str("    scopes = [\n");
        for (i, scope) in self.scopes.iter().enumerate() {
            let comma = if i + 1 < self.scopes.len() { "," } else { "" };
            out.push_str(&format!("      \"{}\"{comma}\n", scope));
        }
        out.push_str("    ];\n");
        if let Some(hint) = &self.login_hint {
            out.push_str(&format!(
                "    auth_uri_fields = {{ \"login_hint\": \"{}\" }};\n",
                hint
            ));
        }
        out.push_str("}\n");
        out
    }
}

/// Result of `ensure_account` — informs the wizard whether to expect an
/// existing token or whether the user needs to run the device-code flow.
#[derive(Debug, PartialEq, Eq)]
pub enum EnsureResult {
    /// The account block was already present in pizauth.conf. No write
    /// happened. Caller should still validate that a token exists.
    AlreadyPresent,
    /// The account block was appended. Caller should reload pizauth and
    /// run the device-code flow.
    Appended,
}

/// Pre-flight check: is pizauth installed and on PATH?
///
/// Returns a helpful error with install instructions if not. Doesn't
/// check whether the daemon is running — that's a separate concern,
/// surfaced by the first command that needs it.
pub fn check_installed() -> Result<()> {
    let out = Command::new("pizauth")
        .arg("--version")
        .output()
        .map_err(|e| {
            anyhow!(
                "pizauth not found on PATH ({e}). Install with:\n\
                 \n    cargo install --git https://github.com/ltratt/pizauth --tag pizauth-1.0.11\n\
                 \nThen ensure ~/.cargo/bin or ~/.local/bin is on PATH and the daemon is running\n\
                 (`pizauth server` on first launch; usually configured via launchd/systemd).\n\
                 \nFull setup: ~/Assistants/shared/CLI-EMAIL-SYSTEM.md"
            )
        })?;
    if !out.status.success() {
        bail!("pizauth --version exited non-zero — installation may be broken");
    }
    Ok(())
}

/// Path to pizauth.conf. Honours `PIZAUTH_CONF` env override (handy for tests).
pub fn pizauth_conf_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("PIZAUTH_CONF") {
        return Ok(PathBuf::from(p));
    }
    let home = std::env::var("HOME").context("HOME env var unset")?;
    Ok(PathBuf::from(home).join(".config").join("pizauth.conf"))
}

/// Does pizauth.conf contain an `account "<name>"` block?
///
/// Naive substring match — pizauth.conf is small and human-managed,
/// false positives in comments are vanishingly unlikely given the
/// `account "..." {` syntax. Returns Ok(false) if the file doesn't
/// exist (caller will create it).
pub fn account_exists(name: &str) -> Result<bool> {
    account_exists_at(&pizauth_conf_path()?, name)
}

pub fn account_exists_at(path: &std::path::Path, name: &str) -> Result<bool> {
    let content = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(anyhow!(e)).with_context(|| format!("reading {}", path.display()))
        }
    };
    Ok(account_block_present(&content, name))
}

fn account_block_present(content: &str, name: &str) -> bool {
    let needle = format!("account \"{name}\"");
    content.contains(&needle)
}

/// Idempotently ensure pizauth.conf has the given account block.
///
/// If the block is already present (matched by `account "<name>"`),
/// returns `AlreadyPresent` and writes nothing. Otherwise backs up the
/// existing pizauth.conf to `<path>.bak.<timestamp>` and appends the
/// new block. If the file doesn't exist, creates it.
///
/// The append is the safest possible operation: existing blocks are
/// untouched, comments and whitespace preserved, only a new block at
/// the end. Users keep editorial control over their own config.
pub fn ensure_account(account: &PizauthAccount) -> Result<EnsureResult> {
    ensure_account_at(&pizauth_conf_path()?, account)
}

pub fn ensure_account_at(path: &std::path::Path, account: &PizauthAccount) -> Result<EnsureResult> {
    let existing = match fs::read_to_string(path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(anyhow!(e)).with_context(|| format!("reading {}", path.display()))
        }
    };

    if let Some(content) = &existing {
        if account_block_present(content, &account.name) {
            return Ok(EnsureResult::AlreadyPresent);
        }
    }

    // Backup before write (only if file exists).
    if existing.is_some() {
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let bak = path.with_extension(format!("conf.bak.{ts}"));
        fs::copy(&path, &bak)
            .with_context(|| format!("backing up to {}", bak.display()))?;
    }

    // Ensure parent dir exists when creating fresh.
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let mut new_content = existing.unwrap_or_default();
    if !new_content.is_empty() && !new_content.ends_with("\n\n") {
        if !new_content.ends_with('\n') {
            new_content.push('\n');
        }
        new_content.push('\n');
    }
    new_content.push_str(&account.render());

    fs::write(&path, &new_content)
        .with_context(|| format!("writing {}", path.display()))?;
    // pizauth expects 0600 — same as ssh keys. The daemon refuses to
    // load otherwise.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = fs::set_permissions(&path, perms);
    }
    Ok(EnsureResult::Appended)
}

/// `pizauth reload` — tells the running daemon to re-read pizauth.conf.
/// Required after appending an account block before that account is
/// reachable via `pizauth refresh` / `pizauth show`.
pub fn reload_daemon() -> Result<()> {
    let out = Command::new("pizauth")
        .arg("reload")
        .output()
        .context("invoking `pizauth reload`")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!(
            "pizauth reload failed: {}. Is the daemon running? Try `pizauth server` once.",
            stderr.trim()
        );
    }
    Ok(())
}

/// Kick off the device-code flow for an account by running
/// `pizauth refresh <name>`. The pizauth daemon will print a verification
/// URL (and code, depending on config) to its log; opening the URL in a
/// browser and authenticating completes the flow.
///
/// This call returns quickly — the actual user-facing authentication
/// happens in the daemon. Use `wait_for_token` afterwards to poll until
/// a token is actually available.
pub fn refresh_account(name: &str) -> Result<()> {
    let out = Command::new("pizauth")
        .args(["refresh", name])
        .output()
        .with_context(|| format!("invoking `pizauth refresh {name}`"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!(
            "pizauth refresh {name} failed: {}. \
             Common causes: account not in pizauth.conf, daemon not running, \
             or `pizauth reload` not run since adding the block.",
            stderr.trim()
        );
    }
    Ok(())
}

/// Validate that `pizauth show <name>` returns a non-empty token.
/// One-shot; doesn't poll.
pub fn validate_account(name: &str) -> Result<()> {
    let out = Command::new("pizauth")
        .args(["show", name])
        .output()
        .with_context(|| format!("invoking `pizauth show {name}`"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("pizauth show {name} failed: {}", stderr.trim());
    }
    let token = String::from_utf8_lossy(&out.stdout);
    if token.trim().is_empty() {
        bail!("pizauth show {name} returned empty — no token available yet");
    }
    Ok(())
}

/// Poll `pizauth show <name>` until a token appears or the timeout
/// elapses. Used after `refresh_account` to wait for the user to
/// complete the device-code flow in their browser.
///
/// Polls every `poll_interval`. Reports progress to stderr every 10s
/// so the user knows we're still waiting.
pub fn wait_for_token(
    name: &str,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<()> {
    let start = Instant::now();
    let mut last_progress = Instant::now();
    eprintln!(
        "Waiting for token for `{name}` (up to {}s) — complete the device-code flow in your browser...",
        timeout.as_secs()
    );
    loop {
        if validate_account(name).is_ok() {
            eprintln!("✓ Token available for `{name}`");
            return Ok(());
        }
        if start.elapsed() > timeout {
            bail!(
                "Timed out waiting for `{name}` token after {}s. \
                 Did you complete the device-code flow? You can retry with \
                 `pizauth refresh {name}` and then re-run this wizard.",
                timeout.as_secs()
            );
        }
        if last_progress.elapsed() > Duration::from_secs(10) {
            let elapsed = start.elapsed().as_secs();
            eprintln!("  …still waiting ({elapsed}s elapsed)");
            last_progress = Instant::now();
        }
        sleep(poll_interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_block_present_finds_named_block() {
        let content = r#"
// some comment
account "cohs-graph" {
    auth_uri = "https://example.com";
}
account "gmail" {
    auth_uri = "https://example.com";
}
"#;
        assert!(account_block_present(content, "cohs-graph"));
        assert!(account_block_present(content, "gmail"));
        assert!(!account_block_present(content, "outlook"));
    }

    #[test]
    fn account_block_present_distinguishes_substring() {
        // "cohs" is a substring of "cohs-graph" — make sure we don't
        // accept the prefix as a hit.
        let content = r#"
account "cohs-graph" {
}
"#;
        assert!(!account_block_present(content, "cohs"));
    }

    #[test]
    fn cohs_graph_template_renders_expected_block() {
        let acc = PizauthAccount::cohs_graph_template("user@cohs.example");
        let rendered = acc.render();
        assert!(rendered.contains("account \"cohs-graph\" {"));
        assert!(rendered.contains("client_id = \"9e5f94bc-e8a4-4e73-b8be-63364c29d753\""));
        assert!(rendered.contains("https://graph.microsoft.com/Mail.Send"));
        assert!(rendered.contains("offline_access"));
        assert!(rendered.contains("\"login_hint\": \"user@cohs.example\""));
        // Public client — no client_secret.
        assert!(!rendered.contains("client_secret"));
    }

    #[test]
    fn gmail_template_includes_client_secret() {
        let acc = PizauthAccount::gmail_template(
            "u@g.com",
            "client-id-here".into(),
            "secret-here".into(),
        );
        let rendered = acc.render();
        assert!(rendered.contains("client_id = \"client-id-here\""));
        assert!(rendered.contains("client_secret = \"secret-here\""));
        assert!(rendered.contains("https://mail.google.com/"));
    }

    #[test]
    fn ensure_account_appends_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pizauth.conf");
        fs::write(&path, "// existing\naccount \"existing\" {\n}\n").unwrap();
        unsafe { std::env::set_var("PIZAUTH_CONF", &path); }

        let acc = PizauthAccount::cohs_graph_template("u@cohs.example");
        let result = ensure_account(&acc).unwrap();
        assert_eq!(result, EnsureResult::Appended);

        let final_content = fs::read_to_string(&path).unwrap();
        assert!(final_content.contains("account \"existing\""));
        assert!(final_content.contains("account \"cohs-graph\""));

        unsafe { std::env::remove_var("PIZAUTH_CONF"); }
    }

    #[test]
    fn ensure_account_skips_when_already_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pizauth.conf");
        fs::write(&path, "account \"cohs-graph\" {\n}\n").unwrap();
        unsafe { std::env::set_var("PIZAUTH_CONF", &path); }

        let acc = PizauthAccount::cohs_graph_template("u@cohs.example");
        let result = ensure_account(&acc).unwrap();
        assert_eq!(result, EnsureResult::AlreadyPresent);

        // No backup file should have been created.
        let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 1, "expected only pizauth.conf, no backup");

        unsafe { std::env::remove_var("PIZAUTH_CONF"); }
    }

    #[test]
    fn ensure_account_creates_file_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pizauth.conf");
        unsafe { std::env::set_var("PIZAUTH_CONF", &path); }

        let acc = PizauthAccount::cohs_graph_template("u@cohs.example");
        let result = ensure_account(&acc).unwrap();
        assert_eq!(result, EnsureResult::Appended);
        assert!(path.exists());
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("account \"cohs-graph\""));

        unsafe { std::env::remove_var("PIZAUTH_CONF"); }
    }
}
