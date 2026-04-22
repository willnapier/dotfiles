//! Interactive setup wizard (Phase 3b, 2026-04-21).
//!
//! Replaces the flat SMTP-only `legacy::init_config` with a branching wizard
//! that writes the new multi-identity `[[email.identities]]` TOML shape
//! consumed by [`crate::email::backends`].
//!
//! Flow:
//! 1. Ask the user which kind of account they're adding (Gmail / M365 /
//!    generic SMTP / advanced).
//! 2. Collect just the fields that branch really needs. Auto-fill server +
//!    port for well-known providers; never auto-decide auth mode (OAuth vs
//!    password) — that's tenant/account-specific, ask the user.
//! 3. Build `IdentityConfig` + metadata (label, from_email, from_name,
//!    primary).
//! 4. Run a live send test — OTP round-trip to the user's own address.
//!    On failure, surface a branch-specific hint and offer "save anyway".
//! 5. Merge the new identity into `~/.config/practiceforge/config.toml`,
//!    promoting any legacy flat `[email]` section to an identities entry
//!    first so we never lose the existing setup.
//!
//! The prompt/stdin-driven outer function is not directly unit-testable;
//! the pure helpers it delegates to (`detect_provider`, `append_identity`,
//! `derive_display_name_from_email`) are, and they carry the logic worth
//! checking.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::PathBuf;

use crate::email::backends::{
    transport_for, AuthConfig, BackendConfig, GraphConfig, IdentityConfig,
};
use crate::email::backends::smtp::{AuthMode, Encryption, SmtpConfig};
use crate::email::{Body, Envelope, Mailbox};

// ---------------------------------------------------------------------------
// TOML on-disk shape for [[email.identities]]
// ---------------------------------------------------------------------------
//
// `IdentityConfig` from `backends/mod.rs` only carries the wire-level halves
// (backend + auth). The config file additionally needs the human-facing
// metadata — label, from_email, from_name, primary. `IdentityEntry` wraps
// both so we can serialize a complete entry in one shot.
//
// Field order below is also the order `toml` will emit: label first (so the
// human scanning config.toml sees the name immediately), then routing
// metadata, then the two sub-tables. Matches the canonical shape in the
// Phase 3b spec.

/// One row under `[[email.identities]]` in config.toml.
///
/// The backend/auth halves are flattened from `IdentityConfig` so the
/// serialized shape matches the spec exactly — `[email.identities.backend]`
/// and `[email.identities.auth]` as nested tables, not a wrapping
/// `[email.identities.identity]` indirection.
///
/// Public because `append_identity` is a public pure helper for tests and
/// future external config-merge tooling. Fields stay `pub(crate)` — only
/// the type name needs to cross the module boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityEntry {
    pub(crate) label: String,
    pub(crate) from_email: String,
    pub(crate) from_name: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) primary: bool,
    pub(crate) backend: BackendConfig,
    pub(crate) auth: AuthConfig,
}

fn is_false(b: &bool) -> bool {
    !*b
}

// ---------------------------------------------------------------------------
// Provider auto-detection
// ---------------------------------------------------------------------------

/// Auto-detection result from an email address's domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provider {
    /// Gmail / Google Workspace (any domain served by Google — we can't
    /// cheaply detect "Workspace with custom domain" from the address
    /// alone, only the consumer suffixes).
    Gmail,
    /// Microsoft 365 — only returned for domains we know are M365 from
    /// the suffix. Custom-domain tenants (like `changeofharleystreet.com`)
    /// cannot be safely auto-classified; they fall through to `Generic`.
    Microsoft365,
    /// Any other provider. Carries a best-guess SMTP host + port using the
    /// widespread cPanel convention (`mail.<domain>`, implicit TLS on 465).
    /// These defaults are *suggestions* — the wizard still lets the user
    /// override.
    Generic { host: String, port: u16 },
}

/// Detect provider from an email address.
///
/// Pure — no I/O, no keychain, no network. The set of "known M365"
/// suffixes is deliberately conservative: only domains where we have
/// certainty. `changeofharleystreet.com` is an M365 tenant but not
/// visible as such from the name, so it falls through to `Generic`;
/// the wizard's Microsoft 365 branch is reached by explicit menu
/// selection, not auto-detection.
pub fn detect_provider(email: &str) -> Provider {
    let domain = match email.split('@').nth(1) {
        Some(d) if !d.is_empty() => d.to_lowercase(),
        _ => {
            // Malformed address — fall back to "mail.<whatever>" pattern
            // using the raw input. Harmless default; caller will override.
            return Provider::Generic {
                host: format!("mail.{}", email),
                port: 465,
            };
        }
    };

    // Gmail / Google consumer + common Workspace aliases.
    match domain.as_str() {
        "gmail.com" | "googlemail.com" | "google.com" => return Provider::Gmail,
        _ => {}
    }

    // Microsoft 365 — only the suffixes that can be safely auto-classified.
    match domain.as_str() {
        "outlook.com"
        | "hotmail.com"
        | "live.com"
        | "msn.com"
        | "office365.com"
        | "onmicrosoft.com" => return Provider::Microsoft365,
        _ => {}
    }
    if domain.ends_with(".onmicrosoft.com") {
        return Provider::Microsoft365;
    }

    // Fallback: cPanel convention for everyone else.
    Provider::Generic {
        host: format!("mail.{}", domain),
        port: 465,
    }
}

/// Title-case display name derived from the local part of an email.
///
/// `will.napier@example.com` → `Will Napier`. Dots and underscores become
/// spaces, each resulting word is title-cased. If no local part is present
/// the input is returned unchanged.
pub fn derive_display_name_from_email(email: &str) -> String {
    let local = email.split('@').next().unwrap_or("");
    if local.is_empty() {
        return email.to_string();
    }
    local
        .replace(['.', '_', '-'], " ")
        .split_whitespace()
        .map(title_case_word)
        .collect::<Vec<_>>()
        .join(" ")
}

fn title_case_word(w: &str) -> String {
    let mut chars = w.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

// ---------------------------------------------------------------------------
// Prompt helpers (local — legacy's versions are private to that module)
// ---------------------------------------------------------------------------

fn prompt(label: &str, default: Option<&str>) -> Result<String> {
    if let Some(d) = default {
        eprint!("{} [{}]: ", label, d);
    } else {
        eprint!("{}: ", label);
    }
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();

    if input.is_empty() {
        match default {
            Some(d) => Ok(d.to_string()),
            None => bail!("Required field"),
        }
    } else {
        Ok(input.to_string())
    }
}

fn prompt_yes_no(label: &str, default_yes: bool) -> Result<bool> {
    let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
    eprint!("{} {}: ", label, suffix);
    io::stderr().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let t = input.trim().to_lowercase();
    Ok(match t.as_str() {
        "" => default_yes,
        "y" | "yes" => true,
        _ => false,
    })
}

fn prompt_password(label: &str) -> Result<String> {
    eprint!("{}: ", label);
    io::stderr().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

// ---------------------------------------------------------------------------
// Keychain helper — reused here because legacy's `store_password` is private.
// ---------------------------------------------------------------------------

/// Store a password under (service, account) in the OS keystore.
///
/// Thin wrapper over `crate::keystore::set` so the wizard's call-sites stay
/// readable.
fn store_password(service: &str, account: &str, password: &str) -> Result<()> {
    crate::keystore::set(service, account, password)
        .with_context(|| format!("storing {service}/{account} in keystore"))
}

// ---------------------------------------------------------------------------
// OTP helpers
// ---------------------------------------------------------------------------

fn generate_otp() -> String {
    use rand::Rng;
    let n: u32 = rand::thread_rng().gen_range(0..1_000_000);
    format!("{:06}", n)
}

/// Send a verification code via the configured identity. Returns the code
/// that was sent so the wizard can compare against user input.
fn send_verification(entry: &IdentityEntry) -> Result<String> {
    let code = generate_otp();
    let transport = transport_for(&IdentityConfig {
        backend: entry.backend.clone(),
        auth: entry.auth.clone(),
    })?;

    let envelope = Envelope::builder(Mailbox::with_name(&entry.from_email, &entry.from_name))
        .to(Mailbox::new(&entry.from_email))
        .subject("PracticeForge — Email Verification")
        .body(Body::Text(format!(
            "Your verification code is: {code}\n\n\
             Enter this code in the PracticeForge setup wizard to confirm \
             your email configuration.\n\n\
             If you didn't request this, you can ignore it."
        )))
        .build();

    transport.send(&envelope)?;
    Ok(code)
}

/// Branch-specific hint for `send_verification` failures.
///
/// We don't try to parse error strings — the failure mode is usually
/// implied by the backend variant and the user's story. Gmail with an
/// app-password attempt that fails is almost always 2FA-not-enabled or
/// the wrong app-password. SMTP against M365 is tenant policy. Graph
/// needs `cohs-oauth-graph init` done first (Graph scope, distinct from
/// `cohs-oauth` which covers the Outlook/IMAP scopes).
fn failure_hint(entry: &IdentityEntry) -> &'static str {
    match (&entry.backend, &entry.auth) {
        (BackendConfig::Smtp(smtp), AuthConfig::Password { .. })
            if smtp.host == "smtp.gmail.com" =>
        {
            "Gmail SMTP + password requires an App Password (Google Account → \
             Security → 2-Step Verification → App passwords). Regular account \
             passwords are not accepted."
        }
        (BackendConfig::Smtp(smtp), _)
            if smtp.host == "smtp.office365.com" || smtp.host.contains("outlook") =>
        {
            "Microsoft 365 tenants commonly block SMTP AUTH at the tenant \
             level. If the send fails with 535/5.7.3, switch to the Microsoft 365 \
             (Graph) setup path instead — it does not require SMTP AUTH."
        }
        (BackendConfig::Graph(_), _) => {
            "Graph sends require a valid Graph-scoped OAuth token (distinct \
             from Outlook-scoped tokens). Run `cohs-oauth-graph init` in \
             another terminal to complete the device-code flow with the \
             Microsoft Graph CLI public client, then retry."
        }
        (BackendConfig::Smtp(_), _) => {
            "Double-check host, port, and credentials. Port 465 → TLS; port 587 \
             → STARTTLS. Some providers require app-specific passwords rather \
             than the account password."
        }
    }
}

// ---------------------------------------------------------------------------
// Config writer
// ---------------------------------------------------------------------------

/// Merge `new_entry` into `existing_toml` and return the updated document.
///
/// Handles three cases:
/// 1. Empty / no `[email]` → append a fresh `[[email.identities]]` section.
/// 2. Legacy flat `[email]` with `smtp_server=...` → promote that section
///    into an `[[email.identities]]` entry (preserving it as primary), then
///    append `new_entry`.
/// 3. Already-new shape with one or more `[[email.identities]]` → strip the
///    existing identities, rewrite the merged list. If `new_entry.primary`
///    is true, clear `primary` on all existing entries first (only one
///    identity can be primary at a time).
///
/// Pure function over strings so it's straightforward to unit-test.
pub fn append_identity(existing_toml: &str, new_entry: &IdentityEntry) -> Result<String> {
    // Parse what's there. An empty document is fine — that's case 1.
    let mut doc: toml::Value = if existing_toml.trim().is_empty() {
        toml::Value::Table(toml::map::Map::new())
    } else {
        existing_toml
            .parse::<toml::Value>()
            .context("Failed to parse existing config.toml")?
    };

    let root = doc
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("config.toml top level is not a table"))?;

    // Collect existing identities from either the legacy or new shape, then
    // rewrite the `email` table from scratch. This is simpler than surgical
    // in-place editing and produces a clean, serde-ordered output.
    let mut identities: Vec<IdentityEntry> = Vec::new();

    if let Some(email_val) = root.remove("email") {
        if let toml::Value::Table(email_tbl) = email_val {
            // --- Case 3: new shape ---
            if let Some(toml::Value::Array(arr)) = email_tbl.get("identities") {
                for item in arr {
                    // Round-trip via serde so we catch shape mismatches
                    // early rather than silently dropping fields.
                    let entry: IdentityEntry = item
                        .clone()
                        .try_into()
                        .context("Failed to parse existing [[email.identities]] entry")?;
                    identities.push(entry);
                }
            }

            // --- Case 2: legacy flat [email] (SMTP-only, password auth) ---
            if identities.is_empty() {
                if let (Some(from_email), Some(smtp_server), Some(username)) = (
                    email_tbl.get("from_email").and_then(|v| v.as_str()),
                    email_tbl.get("smtp_server").and_then(|v| v.as_str()),
                    email_tbl.get("username").and_then(|v| v.as_str()),
                ) {
                    let port = email_tbl
                        .get("smtp_port")
                        .and_then(|v| v.as_integer())
                        .unwrap_or(465) as u16;
                    let encryption = if port == 587 {
                        Encryption::StartTls
                    } else {
                        Encryption::Tls
                    };
                    let from_name = email_tbl
                        .get("from_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or(from_email);

                    identities.push(IdentityEntry {
                        label: "Primary".to_string(),
                        from_email: from_email.to_string(),
                        from_name: from_name.to_string(),
                        primary: true,
                        backend: BackendConfig::Smtp(SmtpConfig {
                            host: smtp_server.to_string(),
                            port,
                            encryption,
                            username: username.to_string(),
                            auth_mode: AuthMode::Password,
                        }),
                        auth: AuthConfig::Password {
                            keyring_service: "clinical-email".to_string(),
                            keyring_account: username.to_string(),
                        },
                    });
                }
            }
        }
        // Non-table `email` (unexpected) → drop silently; caller's new
        // entry becomes the canonical content.
    }

    // If the new entry is primary, demote any existing primary flags so we
    // don't write a config with two primaries.
    if new_entry.primary {
        for e in identities.iter_mut() {
            e.primary = false;
        }
    }

    // If there were no identities at all and the new entry doesn't claim
    // primary, force it — a lone identity is by definition primary.
    let mut new_entry = new_entry.clone();
    if identities.is_empty() && !new_entry.primary {
        new_entry.primary = true;
    }

    identities.push(new_entry);

    // Build the merged email table and re-insert at the top level.
    let mut email_tbl = toml::map::Map::new();
    email_tbl.insert(
        "identities".to_string(),
        toml::Value::try_from(&identities).context("Serialising identities")?,
    );
    root.insert("email".to_string(), toml::Value::Table(email_tbl));

    toml::to_string_pretty(&doc).context("Serialising updated config.toml")
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("practiceforge/config.toml"))
        .unwrap_or_default()
}

fn write_entry_to_config(entry: &IdentityEntry) -> Result<PathBuf> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Creating {}", parent.display()))?;
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let merged = append_identity(&existing, entry)?;

    // Atomic-ish write: write to .tmp then rename, so a crash mid-write
    // leaves the old file intact.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, merged).with_context(|| format!("Writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("Renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(path)
}

/// Count identities currently in the config (zero if the file is missing
/// or malformed). Used to set the default answer to "is this primary?".
fn existing_identity_count() -> usize {
    let path = config_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let table: toml::Table = match content.parse() {
        Ok(t) => t,
        Err(_) => return 0,
    };
    let n_new = table
        .get("email")
        .and_then(|v| v.as_table())
        .and_then(|e| e.get("identities"))
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    if n_new > 0 {
        return n_new;
    }
    // Legacy flat [email] counts as one existing identity.
    let legacy = table
        .get("email")
        .and_then(|v| v.as_table())
        .and_then(|e| e.get("smtp_server"))
        .is_some();
    if legacy {
        1
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Branch: Gmail
// ---------------------------------------------------------------------------

fn wizard_gmail(initial_email: Option<&str>) -> Result<IdentityEntry> {
    let email = match initial_email {
        Some(e) => e.to_string(),
        None => prompt("Gmail address", None)?,
    };
    let default_name = derive_display_name_from_email(&email);
    let from_name = prompt("Display name", Some(&default_name))?;
    let label = prompt("Label (short nickname for this identity)", Some("Gmail"))?;

    eprintln!("\nAuth mode:");
    eprintln!("  [a] App password (simple — requires 2FA + an app-specific password)");
    eprintln!("  [b] OAuth2 via helper command (XOAUTH2 — more secure)");
    let choice = prompt("Choose [a/b]", Some("a"))?.to_lowercase();

    let (auth_mode, auth) = match choice.as_str() {
        "a" => {
            let password = prompt_password("Gmail app password")?;
            if password.is_empty() {
                bail!("App password is required");
            }
            store_password("clinical-email", &email, &password)?;
            (
                AuthMode::Password,
                AuthConfig::Password {
                    keyring_service: "clinical-email".to_string(),
                    keyring_account: email.clone(),
                },
            )
        }
        "b" => {
            eprintln!(
                "\nNote: PracticeForge does not ship a Gmail OAuth helper yet. You will \
                 need to supply a command that prints a fresh access token to stdout \
                 (e.g. a wrapper around `oauth2l` or `oauth2ms`)."
            );
            let command = prompt(
                "OAuth token command",
                Some("oauth2ms-gmail show"),
            )?;
            (
                AuthMode::XOAuth2,
                AuthConfig::OAuth2Command { command },
            )
        }
        _ => bail!("Invalid choice: {}", choice),
    };

    Ok(IdentityEntry {
        label,
        from_email: email.clone(),
        from_name,
        primary: false, // decided later
        backend: BackendConfig::Smtp(SmtpConfig {
            host: "smtp.gmail.com".to_string(),
            port: 465,
            encryption: Encryption::Tls,
            username: email,
            auth_mode,
        }),
        auth,
    })
}

// ---------------------------------------------------------------------------
// Branch: Microsoft 365
// ---------------------------------------------------------------------------

fn wizard_m365(initial_email: Option<&str>) -> Result<IdentityEntry> {
    eprintln!(
        "\nM365 tenants commonly block SMTP AUTH. This wizard configures \
         Microsoft Graph for sending (no SMTP), which works regardless of \
         tenant SMTP policy."
    );

    let email = match initial_email {
        Some(e) => e.to_string(),
        None => prompt("Microsoft 365 email address", None)?,
    };
    let default_name = derive_display_name_from_email(&email);
    let from_name = prompt("Display name", Some(&default_name))?;
    let label = prompt("Label (short nickname for this identity)", Some("Microsoft 365"))?;

    // Graph sendMail needs the Mail.Send scope. Microsoft's OAuth2 won't
    // issue a single token spanning both outlook.office.com and
    // graph.microsoft.com resources, AND Thunderbird's public client app
    // isn't registered for Graph resources anyway. So the Graph send path
    // uses a *separate* OAuth flow with Microsoft Graph CLI's public
    // client ID — handled by the `cohs-oauth-graph` helper. The IMAP/SMTP
    // side (if the tenant ever re-enables it) continues to use plain
    // `cohs-oauth`, which is Thunderbird-client + Outlook scopes.
    eprintln!(
        "\nBefore this identity can send mail, you must have completed the \
         Graph-scope device-code flow at least once. If you haven't, open \
         another terminal and run `cohs-oauth-graph init` now, then return \
         here."
    );
    eprintln!(
        "If you also want COHS IMAP access (not needed for Graph send), run \
         `cohs-oauth init` separately — it's a distinct token pool."
    );

    let command = prompt(
        "OAuth token command (Graph-scope)",
        Some("cohs-oauth-graph show"),
    )?;

    Ok(IdentityEntry {
        label,
        from_email: email,
        from_name,
        primary: false,
        backend: BackendConfig::Graph(GraphConfig::default()),
        auth: AuthConfig::OAuth2Command { command },
    })
}

// ---------------------------------------------------------------------------
// Branch: Generic SMTP
// ---------------------------------------------------------------------------

fn wizard_generic_smtp(initial_email: Option<&str>) -> Result<IdentityEntry> {
    let email = match initial_email {
        Some(e) => e.to_string(),
        None => prompt("Email address", None)?,
    };
    let default_name = derive_display_name_from_email(&email);
    let from_name = prompt("Display name", Some(&default_name))?;
    let label = prompt("Label (short nickname for this identity)", Some("Email"))?;

    let (default_host, default_port) = match detect_provider(&email) {
        Provider::Gmail => ("smtp.gmail.com".to_string(), 465u16),
        Provider::Microsoft365 => ("smtp.office365.com".to_string(), 587u16),
        Provider::Generic { host, port } => (host, port),
    };

    let host = prompt("SMTP host", Some(&default_host))?;
    let port: u16 = prompt("SMTP port", Some(&default_port.to_string()))?
        .parse()
        .context("Invalid port number")?;

    let default_enc = if port == 587 { "starttls" } else { "tls" };
    let enc_str = prompt("Encryption (tls = 465, starttls = 587)", Some(default_enc))?;
    let encryption = match enc_str.to_lowercase().as_str() {
        "tls" => Encryption::Tls,
        "starttls" | "start-tls" => Encryption::StartTls,
        other => bail!("Unknown encryption: {}", other),
    };

    let username = prompt("SMTP username", Some(&email))?;

    eprintln!("\nAuth mode:");
    eprintln!("  [a] Password (stored in keychain)");
    eprintln!("  [b] OAuth2 via helper command (XOAUTH2 — requires external token helper)");
    let choice = prompt("Choose [a/b]", Some("a"))?.to_lowercase();

    let (auth_mode, auth) = match choice.as_str() {
        "a" => {
            let password = prompt_password("SMTP password")?;
            if password.is_empty() {
                bail!("Password is required");
            }
            store_password("clinical-email", &username, &password)?;
            (
                AuthMode::Password,
                AuthConfig::Password {
                    keyring_service: "clinical-email".to_string(),
                    keyring_account: username.clone(),
                },
            )
        }
        "b" => {
            let command = prompt("OAuth token command", None)?;
            (AuthMode::XOAuth2, AuthConfig::OAuth2Command { command })
        }
        _ => bail!("Invalid choice: {}", choice),
    };

    Ok(IdentityEntry {
        label,
        from_email: email,
        from_name,
        primary: false,
        backend: BackendConfig::Smtp(SmtpConfig {
            host,
            port,
            encryption,
            username,
            auth_mode,
        }),
        auth,
    })
}

// ---------------------------------------------------------------------------
// Branch: Advanced (all fields manual)
// ---------------------------------------------------------------------------

fn wizard_advanced() -> Result<IdentityEntry> {
    let email = prompt("Email (from address)", None)?;
    let from_name = prompt("Display name", Some(&derive_display_name_from_email(&email)))?;
    let label = prompt("Label", Some("Custom"))?;

    eprintln!("\nBackend:");
    eprintln!("  [s] SMTP");
    eprintln!("  [g] Microsoft Graph");
    let backend_choice = prompt("Choose [s/g]", Some("s"))?.to_lowercase();

    let backend = match backend_choice.as_str() {
        "s" => {
            let host = prompt("SMTP host", None)?;
            let port: u16 = prompt("SMTP port", Some("465"))?
                .parse()
                .context("Invalid port")?;
            let enc_str = prompt("Encryption (tls / starttls)", Some("tls"))?;
            let encryption = match enc_str.to_lowercase().as_str() {
                "tls" => Encryption::Tls,
                "starttls" | "start-tls" => Encryption::StartTls,
                other => bail!("Unknown encryption: {}", other),
            };
            let username = prompt("SMTP username", Some(&email))?;
            let auth_str = prompt("SMTP auth mode (password / xoauth2)", Some("password"))?;
            let auth_mode = match auth_str.to_lowercase().as_str() {
                "password" => AuthMode::Password,
                "xoauth2" | "oauth" | "oauth2" => AuthMode::XOAuth2,
                other => bail!("Unknown auth mode: {}", other),
            };
            BackendConfig::Smtp(SmtpConfig {
                host,
                port,
                encryption,
                username,
                auth_mode,
            })
        }
        "g" => BackendConfig::Graph(GraphConfig::default()),
        _ => bail!("Invalid backend choice: {}", backend_choice),
    };

    eprintln!("\nAuth source:");
    eprintln!("  [p] Password from keychain");
    eprintln!("  [c] OAuth2 command");
    let auth_choice = prompt("Choose [p/c]", Some("p"))?.to_lowercase();

    let auth = match auth_choice.as_str() {
        "p" => {
            let keyring_service =
                prompt("Keyring service", Some("clinical-email"))?;
            let keyring_account = prompt("Keyring account", Some(&email))?;
            // Ask the user if they want to store a password now. If no, they
            // can `security add-generic-password ...` themselves later.
            if prompt_yes_no("Store a password for this account now?", true)? {
                let password = prompt_password("Password")?;
                if !password.is_empty() {
                    store_password(&keyring_service, &keyring_account, &password)?;
                }
            }
            AuthConfig::Password {
                keyring_service,
                keyring_account,
            }
        }
        "c" => {
            let command = prompt("OAuth token command", None)?;
            AuthConfig::OAuth2Command { command }
        }
        _ => bail!("Invalid auth choice: {}", auth_choice),
    };

    Ok(IdentityEntry {
        label,
        from_email: email,
        from_name,
        primary: false,
        backend,
        auth,
    })
}

// ---------------------------------------------------------------------------
// Top-level entry point
// ---------------------------------------------------------------------------

/// Interactive setup wizard — configure a new email identity.
pub fn init_config() -> Result<()> {
    println!("=== PracticeForge email setup ===\n");
    println!("What kind of email account do you want to add?\n");
    println!("  [1] Gmail / Google Workspace");
    println!("  [2] Microsoft 365 (Exchange Online)");
    println!("  [3] Generic SMTP (any other provider)");
    println!("  [4] Advanced — enter all fields manually");
    println!();

    let choice = prompt("Enter choice [1-4]", Some("3"))?;

    // Email-first optimisation: if the user pastes an address as the
    // *first* response to this prompt (mistaking the flow), we could
    // detect the provider from it and jump branches. For now we stick
    // with menu-first; the spec calls that out as a known-acceptable
    // simplification and the branches themselves ask for the address
    // right away.
    let mut entry = match choice.trim() {
        "1" => wizard_gmail(None)?,
        "2" => wizard_m365(None)?,
        "3" => wizard_generic_smtp(None)?,
        "4" => wizard_advanced()?,
        other => bail!("Invalid choice: {}", other),
    };

    // Primary-identity prompt. Default depends on whether any identities
    // already exist in config.
    let existing = existing_identity_count();
    let default_primary = existing == 0;
    let primary_question = if existing == 0 {
        "Is this your primary identity?"
    } else {
        "Make this the new primary identity (demoting the current one)?"
    };
    entry.primary = prompt_yes_no(primary_question, default_primary)?;

    // ---- OTP verification --------------------------------------------------
    eprintln!("\nSending verification code to {}...", entry.from_email);
    let send_result = send_verification(&entry);
    match send_result {
        Ok(code) => {
            eprintln!("✓ Code sent. Check your inbox.");

            let mut verified = false;
            for attempt in 1..=3 {
                let entered = prompt(
                    &format!("Enter the 6-digit code (attempt {}/3)", attempt),
                    None,
                )?;
                if entered.trim() == code {
                    verified = true;
                    break;
                }
                eprintln!("Incorrect code.");
            }

            if !verified {
                if prompt_yes_no(
                    "Verification failed. Save the identity anyway? (you can verify later)",
                    false,
                )? {
                    eprintln!("Saving unverified identity.");
                } else {
                    bail!("Verification failed — config not saved.");
                }
            } else {
                eprintln!("\n✓ Email verified.");
            }
        }
        Err(e) => {
            eprintln!("\n✗ Failed to send verification code: {e}");
            eprintln!("\nHint: {}", failure_hint(&entry));
            if !prompt_yes_no(
                "\nSave the identity anyway? (you can fix credentials and re-test later)",
                false,
            )? {
                bail!("Aborted — config not saved.");
            }
        }
    }

    // ---- Persist -----------------------------------------------------------
    let path = write_entry_to_config(&entry)?;

    println!("\n✓ Email setup complete.");
    println!("  Label:  {}", entry.label);
    println!("  From:   {} <{}>", entry.from_name, entry.from_email);
    match &entry.backend {
        BackendConfig::Smtp(s) => {
            println!("  Server: {}:{} ({:?})", s.host, s.port, s.encryption);
        }
        BackendConfig::Graph(_) => {
            println!("  Server: Microsoft Graph");
        }
    }
    match &entry.auth {
        AuthConfig::Password { keyring_service, keyring_account } => {
            println!(
                "  Auth:   password (keychain service={}, account={})",
                keyring_service, keyring_account
            );
        }
        AuthConfig::OAuth2Command { command } => {
            println!("  Auth:   OAuth2 token via `{command}`");
        }
        AuthConfig::KeychainM365 => {
            println!("  Auth:   M365 OAuth (in-Rust refresh from keystore)");
        }
    }
    println!("  Primary: {}", if entry.primary { "yes" } else { "no" });
    println!("  Config:  {}", path.display());

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- detect_provider ---------------------------------------------------

    #[test]
    fn detect_gmail_canonical() {
        assert_eq!(detect_provider("alice@gmail.com"), Provider::Gmail);
    }

    #[test]
    fn detect_gmail_alternate() {
        assert_eq!(detect_provider("bob@googlemail.com"), Provider::Gmail);
    }

    #[test]
    fn detect_m365_outlook() {
        assert_eq!(detect_provider("x@outlook.com"), Provider::Microsoft365);
        assert_eq!(detect_provider("x@hotmail.com"), Provider::Microsoft365);
    }

    #[test]
    fn detect_m365_onmicrosoft_subdomain() {
        // Any tenant-named *.onmicrosoft.com should be M365.
        assert_eq!(
            detect_provider("user@mytenant.onmicrosoft.com"),
            Provider::Microsoft365,
        );
    }

    #[test]
    fn detect_cohs_falls_through_to_generic() {
        // COHS is an M365 tenant but the custom domain doesn't reveal that.
        // We must NOT auto-claim M365 here — the user selects the M365
        // branch from the menu explicitly. Generic is the correct default.
        match detect_provider("will.napier@changeofharleystreet.com") {
            Provider::Generic { host, port } => {
                assert_eq!(host, "mail.changeofharleystreet.com");
                assert_eq!(port, 465);
            }
            other => panic!("expected Generic, got {:?}", other),
        }
    }

    #[test]
    fn detect_custom_domain_uses_cpanel_convention() {
        match detect_provider("will@willnapier.com") {
            Provider::Generic { host, port } => {
                assert_eq!(host, "mail.willnapier.com");
                assert_eq!(port, 465);
            }
            other => panic!("expected Generic, got {:?}", other),
        }
    }

    #[test]
    fn detect_malformed_email_returns_generic() {
        // No `@` → treat as domain-ish, build a mail.* default. Harmless;
        // the wizard lets the user override, and this path is only hit
        // if the user typed a malformed address.
        match detect_provider("not-an-email") {
            Provider::Generic { .. } => {}
            other => panic!("expected Generic, got {:?}", other),
        }
    }

    // --- derive_display_name_from_email ------------------------------------

    #[test]
    fn derive_simple_local_part() {
        assert_eq!(derive_display_name_from_email("alice@example.com"), "Alice");
    }

    #[test]
    fn derive_dotted_local_part() {
        assert_eq!(
            derive_display_name_from_email("will.napier@example.com"),
            "Will Napier"
        );
    }

    #[test]
    fn derive_underscore_local_part() {
        assert_eq!(
            derive_display_name_from_email("will_napier@example.com"),
            "Will Napier"
        );
    }

    // --- append_identity ---------------------------------------------------

    fn sample_entry() -> IdentityEntry {
        IdentityEntry {
            label: "Napier Psychology".to_string(),
            from_email: "will@willnapier.com".to_string(),
            from_name: "Will Napier".to_string(),
            primary: true,
            backend: BackendConfig::Smtp(SmtpConfig {
                host: "mail.willnapier.com".to_string(),
                port: 465,
                encryption: Encryption::Tls,
                username: "will@willnapier.com".to_string(),
                auth_mode: AuthMode::Password,
            }),
            auth: AuthConfig::Password {
                keyring_service: "clinical-email".to_string(),
                keyring_account: "will@willnapier.com".to_string(),
            },
        }
    }

    fn graph_entry() -> IdentityEntry {
        IdentityEntry {
            label: "Change of Harley Street".to_string(),
            from_email: "will.napier@changeofharleystreet.com".to_string(),
            from_name: "Will Napier".to_string(),
            primary: false,
            backend: BackendConfig::Graph(GraphConfig::default()),
            auth: AuthConfig::OAuth2Command {
                command: "cohs-oauth-graph show".to_string(),
            },
        }
    }

    #[test]
    fn append_to_empty_produces_identities_array() {
        let out = append_identity("", &sample_entry()).unwrap();
        // Must parse as TOML and expose the identity under [email].identities.
        let parsed: toml::Value = out.parse().unwrap();
        let identities = parsed["email"]["identities"].as_array().unwrap();
        assert_eq!(identities.len(), 1);
        assert_eq!(identities[0]["from_email"].as_str().unwrap(), "will@willnapier.com");
        assert_eq!(identities[0]["backend"]["type"].as_str().unwrap(), "smtp");
        assert_eq!(identities[0]["auth"]["type"].as_str().unwrap(), "password");
    }

    #[test]
    fn append_promotes_legacy_flat_email_section() {
        let legacy = r#"
[email]
smtp_server = "mail.old.example.com"
smtp_port = 587
username = "old@example.com"
from_email = "old@example.com"
from_name = "Old User"
signature = ""
"#;
        let out = append_identity(legacy, &sample_entry()).unwrap();
        let parsed: toml::Value = out.parse().unwrap();
        let identities = parsed["email"]["identities"].as_array().unwrap();
        // Legacy promoted + new entry = 2 total.
        assert_eq!(identities.len(), 2);
        // Legacy came first → still primary=false on itself *after* the new
        // entry claimed primary; new entry is primary.
        let legacy_entry = identities
            .iter()
            .find(|i| i["from_email"].as_str() == Some("old@example.com"))
            .unwrap();
        assert_eq!(legacy_entry["backend"]["host"].as_str().unwrap(), "mail.old.example.com");
        assert_eq!(legacy_entry["backend"]["port"].as_integer().unwrap(), 587);
        // 587 should have been promoted to STARTTLS.
        assert_eq!(legacy_entry["backend"]["encryption"].as_str().unwrap(), "start-tls");

        let new_entry = identities
            .iter()
            .find(|i| i["from_email"].as_str() == Some("will@willnapier.com"))
            .unwrap();
        assert_eq!(new_entry.get("primary").and_then(|v| v.as_bool()), Some(true));

        // Legacy must have been demoted — either `primary = false` or the
        // field is absent (we skip serialising `false` to keep configs tidy).
        assert!(
            matches!(
                legacy_entry.get("primary").and_then(|v| v.as_bool()),
                None | Some(false)
            ),
            "legacy entry should not still be primary"
        );
    }

    #[test]
    fn append_to_existing_new_shape_keeps_both() {
        // First insert, then second insert — second inserts should see the
        // first and produce an array of two.
        let first = append_identity("", &sample_entry()).unwrap();
        let second = append_identity(&first, &graph_entry()).unwrap();
        let parsed: toml::Value = second.parse().unwrap();
        let identities = parsed["email"]["identities"].as_array().unwrap();
        assert_eq!(identities.len(), 2);

        let graph_row = identities
            .iter()
            .find(|i| i["backend"]["type"].as_str() == Some("graph"))
            .expect("graph identity missing");
        assert_eq!(graph_row["auth"]["type"].as_str().unwrap(), "oauth2_command");
        assert_eq!(
            graph_row["auth"]["command"].as_str().unwrap(),
            "cohs-oauth-graph show"
        );
    }

    #[test]
    fn append_demotes_existing_primary_when_new_is_primary() {
        let first = append_identity("", &sample_entry()).unwrap(); // primary=true
        // New primary entry
        let mut new_primary = graph_entry();
        new_primary.primary = true;
        let merged = append_identity(&first, &new_primary).unwrap();
        let parsed: toml::Value = merged.parse().unwrap();
        let identities = parsed["email"]["identities"].as_array().unwrap();

        let primaries: Vec<_> = identities
            .iter()
            .filter(|i| i.get("primary").and_then(|v| v.as_bool()) == Some(true))
            .collect();
        assert_eq!(primaries.len(), 1, "exactly one primary must remain");
        // The graph entry took over as primary.
        assert_eq!(
            primaries[0]["from_email"].as_str().unwrap(),
            "will.napier@changeofharleystreet.com"
        );
    }

    #[test]
    fn append_preserves_unrelated_sections() {
        let existing = r#"
[voice]
endpoint = "http://localhost:11434"
model = "clinical-voice-q4"
"#;
        let out = append_identity(existing, &sample_entry()).unwrap();
        let parsed: toml::Value = out.parse().unwrap();
        // Unrelated section must survive the merge.
        assert_eq!(
            parsed["voice"]["endpoint"].as_str().unwrap(),
            "http://localhost:11434"
        );
        assert_eq!(parsed["voice"]["model"].as_str().unwrap(), "clinical-voice-q4");
        // And the new identity is there.
        assert_eq!(parsed["email"]["identities"].as_array().unwrap().len(), 1);
    }
}
