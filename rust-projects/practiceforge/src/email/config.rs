//! Config loader — parse `~/.config/practiceforge/config.toml` into
//! fully-typed [`Identity`] values.
//!
//! Two TOML shapes are supported during the Phase 3/4 migration:
//!
//! 1. **New tagged shape** — each `[[email.identities]]` entry has a
//!    `[email.identities.backend]` subtable (tagged `type = "smtp" | "graph"`)
//!    and a `[email.identities.auth]` subtable (tagged
//!    `type = "password" | "oauth2_command"`). This is what the Phase 3b
//!    wizard writes.
//! 2. **Legacy flat shape** — the pre-refactor format with fields
//!    `smtp_server` / `smtp_port` / `username` directly on the identity
//!    entry and no `backend` / `auth` subtables. Synthesised into SMTP +
//!    keychain-password on read.
//!
//! Shape detection is per-identity, not per-file, so mixed files are OK.
//!
//! Legacy was retired in Phase 4; these are the only `load_identities` /
//! `find_identity` in the crate.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::email::backends::{
    AuthConfig, BackendConfig, Encryption, IdentityConfig, SmtpConfig,
    smtp::AuthMode,
};

/// A fully configured send-identity from config.toml.
#[derive(Debug, Clone)]
pub struct Identity {
    pub label: String,
    pub from_email: String,
    pub from_name: String,
    pub primary: bool,
    pub config: IdentityConfig,
}

// ---------------------------------------------------------------------------
// Raw TOML shape — mirrors what serde can parse directly. The transformation
// into `Identity` lives in `raw_to_identity`, which is where legacy-shape
// detection happens.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize)]
struct RawRoot {
    #[serde(default)]
    email: Option<RawEmail>,
}

#[derive(Debug, Deserialize, Serialize)]
struct RawEmail {
    #[serde(default)]
    identities: Vec<RawIdentity>,
}

/// A single `[[email.identities]]` entry. Holds both new-shape and
/// legacy-shape fields as `Option`s; the presence pattern tells us which
/// shape we're dealing with.
#[derive(Debug, Deserialize, Serialize)]
struct RawIdentity {
    label: Option<String>,
    from_email: String,
    from_name: Option<String>,
    #[serde(default)]
    primary: bool,

    // New shape — tagged subtables.
    backend: Option<BackendConfig>,
    auth: Option<AuthConfig>,

    // Legacy shape — flat fields. Only consulted when `backend` / `auth` are
    // absent.
    smtp_server: Option<String>,
    smtp_port: Option<u16>,
    username: Option<String>,
}

fn default_keyring_service() -> String {
    "clinical-email".to_string()
}

/// Transform a raw parsed entry into a public `Identity`. This is where the
/// legacy-shape synthesis lives.
///
/// Detection heuristic: if both `backend` AND `auth` are present, use the
/// new shape verbatim. Otherwise, require the three legacy fields
/// (`smtp_server`, `username`, and a port — defaulted to 465 if absent) and
/// synthesise a modern SMTP+password config.
///
/// We require *both* new-shape fields rather than either, to avoid silently
/// mixing a new `backend` with a synthesised `auth` (or vice versa) — that
/// would almost certainly be a config mistake worth failing on.
fn raw_to_identity(raw: RawIdentity) -> Result<Identity> {
    let from_email = raw.from_email.clone();
    let from_name = raw.from_name.clone().unwrap_or_else(|| from_email.clone());
    let label = raw.label.clone().unwrap_or_default();

    let config = match (raw.backend, raw.auth) {
        (Some(backend), Some(auth)) => IdentityConfig { backend, auth },
        (None, None) => {
            // Legacy shape — synthesise.
            let smtp_server = raw
                .smtp_server
                .context("identity missing both new-shape (backend/auth) and legacy (smtp_server) fields")?;
            let username = raw
                .username
                .clone()
                .unwrap_or_else(|| from_email.clone());
            let port = raw.smtp_port.unwrap_or(465);
            let encryption = if port == 465 {
                Encryption::Tls
            } else {
                Encryption::StartTls
            };
            IdentityConfig {
                backend: BackendConfig::Smtp(SmtpConfig {
                    host: smtp_server,
                    port,
                    encryption,
                    username: username.clone(),
                    auth_mode: AuthMode::Password,
                }),
                auth: AuthConfig::Password {
                    keyring_service: default_keyring_service(),
                    keyring_account: username,
                },
            }
        }
        (Some(_), None) | (None, Some(_)) => {
            anyhow::bail!(
                "identity {} has partial new-shape config — both [backend] and [auth] must be present, or neither (legacy shape)",
                from_email
            );
        }
    };

    Ok(Identity {
        label,
        from_email,
        from_name,
        primary: raw.primary,
        config,
    })
}

/// Parse a TOML string into a vector of identities. Factored out so tests
/// never touch the filesystem.
///
/// - Returns `Ok(vec![])` when no `[[email.identities]]` is present at all.
/// - Returns `Err(..)` on malformed TOML or on shape inconsistency within
///   an identity (partial new shape).
/// - Promotes the first identity to primary if none is explicitly marked.
fn parse_identities(s: &str) -> Result<Vec<Identity>> {
    let root: RawRoot = toml::from_str(s).context("parsing practiceforge email config")?;

    let raw_identities = root
        .email
        .map(|e| e.identities)
        .unwrap_or_default();

    let mut identities: Vec<Identity> = raw_identities
        .into_iter()
        .map(raw_to_identity)
        .collect::<Result<_>>()?;

    if !identities.is_empty() && !identities.iter().any(|i| i.primary) {
        identities[0].primary = true;
    }

    Ok(identities)
}

fn config_path() -> PathBuf {
    // Use the crate-wide config_dir (which pins to ~/.config on macOS
    // rather than ~/Library/Application Support) so the email module
    // reads from the same path the rest of the crate writes to.
    crate::config::config_dir().join("config.toml")
}

/// Load all identities from `~/.config/practiceforge/config.toml`.
///
/// Returns `[]` if the file is missing or unparseable — consistent with the
/// legacy loader's "fail soft" behaviour, which callers already rely on
/// (they treat empty-vec as "no email configured"). Unlike
/// [`parse_identities`], this swallows errors; use `parse_identities`
/// directly in tests or when you want a surfaced error.
pub fn load_identities() -> Vec<Identity> {
    let path = config_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    parse_identities(&content).unwrap_or_default()
}

/// Find an identity by its `from_email` address.
pub fn find_identity(from_email: &str) -> Option<Identity> {
    load_identities()
        .into_iter()
        .find(|i| i.from_email == from_email)
}

/// Return the primary identity, if any are configured.
///
/// Helper used by call sites that want "the default from-address" without
/// knowing a specific email up front — CLI `email test`, billing reminders,
/// reschedule offer letters.
pub fn primary_identity() -> Option<Identity> {
    let ids = load_identities();
    ids.iter()
        .find(|i| i.primary)
        .cloned()
        .or_else(|| ids.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEW_SHAPE_SMTP: &str = r#"
[[email.identities]]
label = "Napier Psychology"
from_email = "will@willnapier.com"
from_name = "Will Napier"
primary = true

[email.identities.backend]
type = "smtp"
host = "smtp.gmail.com"
port = 465
encryption = "tls"
username = "will@willnapier.com"
auth_mode = "password"

[email.identities.auth]
type = "password"
keyring_service = "clinical-email"
keyring_account = "will@willnapier.com"
"#;

    const NEW_SHAPE_GRAPH: &str = r#"
[[email.identities]]
label = "Change of Harley Street"
from_email = "will.napier@changeofharleystreet.com"
from_name = "Will Napier"

[email.identities.backend]
type = "graph"

[email.identities.auth]
type = "oauth2_command"
command = "cohs-oauth show"
"#;

    const LEGACY_FLAT: &str = r#"
[[email.identities]]
label = "Napier Psychology"
from_email = "will@willnapier.com"
from_name = "Will Napier"
primary = true
smtp_server = "smtp.gmail.com"
smtp_port = 465
username = "will@willnapier.com"
"#;

    const LEGACY_FLAT_587: &str = r#"
[[email.identities]]
from_email = "bob@example.com"
smtp_server = "mail.example.com"
smtp_port = 587
username = "bob"
"#;

    #[test]
    fn parses_new_shape_smtp_password() {
        let ids = parse_identities(NEW_SHAPE_SMTP).expect("parse");
        assert_eq!(ids.len(), 1);
        let id = &ids[0];
        assert_eq!(id.label, "Napier Psychology");
        assert_eq!(id.from_email, "will@willnapier.com");
        assert_eq!(id.from_name, "Will Napier");
        assert!(id.primary);

        match &id.config.backend {
            BackendConfig::Smtp(cfg) => {
                assert_eq!(cfg.host, "smtp.gmail.com");
                assert_eq!(cfg.port, 465);
                assert_eq!(cfg.encryption, Encryption::Tls);
                assert_eq!(cfg.username, "will@willnapier.com");
                assert_eq!(cfg.auth_mode, AuthMode::Password);
            }
            _ => panic!("expected SMTP backend"),
        }
        match &id.config.auth {
            AuthConfig::Password { keyring_service, keyring_account } => {
                assert_eq!(keyring_service, "clinical-email");
                assert_eq!(keyring_account, "will@willnapier.com");
            }
            _ => panic!("expected Password auth"),
        }
    }

    #[test]
    fn parses_new_shape_graph_oauth2() {
        let ids = parse_identities(NEW_SHAPE_GRAPH).expect("parse");
        assert_eq!(ids.len(), 1);
        let id = &ids[0];
        assert_eq!(id.from_email, "will.napier@changeofharleystreet.com");
        // No explicit primary → first becomes primary.
        assert!(id.primary);

        match &id.config.backend {
            BackendConfig::Graph(cfg) => {
                // Defaults should apply for omitted subfields.
                assert_eq!(cfg.base_url, "https://graph.microsoft.com/v1.0");
                assert!(cfg.save_to_sent_items);
            }
            _ => panic!("expected Graph backend"),
        }
        match &id.config.auth {
            AuthConfig::OAuth2Command { command } => {
                assert_eq!(command, "cohs-oauth show");
            }
            _ => panic!("expected OAuth2Command auth"),
        }
    }

    #[test]
    fn parses_legacy_flat_shape_port_465_implies_tls() {
        let ids = parse_identities(LEGACY_FLAT).expect("parse");
        assert_eq!(ids.len(), 1);
        let id = &ids[0];
        match &id.config.backend {
            BackendConfig::Smtp(cfg) => {
                assert_eq!(cfg.host, "smtp.gmail.com");
                assert_eq!(cfg.port, 465);
                assert_eq!(cfg.encryption, Encryption::Tls);
                assert_eq!(cfg.auth_mode, AuthMode::Password);
            }
            _ => panic!("legacy synthesis should produce SMTP backend"),
        }
        match &id.config.auth {
            AuthConfig::Password { keyring_service, keyring_account } => {
                assert_eq!(keyring_service, "clinical-email");
                assert_eq!(keyring_account, "will@willnapier.com");
            }
            _ => panic!("legacy synthesis should produce Password auth"),
        }
    }

    #[test]
    fn parses_legacy_flat_shape_port_587_implies_starttls() {
        let ids = parse_identities(LEGACY_FLAT_587).expect("parse");
        assert_eq!(ids.len(), 1);
        match &ids[0].config.backend {
            BackendConfig::Smtp(cfg) => {
                assert_eq!(cfg.port, 587);
                assert_eq!(cfg.encryption, Encryption::StartTls);
            }
            _ => panic!("expected SMTP backend"),
        }
    }

    #[test]
    fn parses_mixed_file_new_plus_legacy() {
        // Two identities — first new-shape Graph, second legacy flat SMTP.
        let mixed = format!("{}\n{}", NEW_SHAPE_GRAPH, LEGACY_FLAT);
        let ids = parse_identities(&mixed).expect("parse mixed");
        assert_eq!(ids.len(), 2);

        assert!(matches!(ids[0].config.backend, BackendConfig::Graph(_)));
        assert!(matches!(ids[1].config.backend, BackendConfig::Smtp(_)));

        // First one in the file has no `primary`; second has `primary = true`.
        // Since at least one is primary, no promotion happens — first stays false.
        assert!(!ids[0].primary);
        assert!(ids[1].primary);
    }

    #[test]
    fn empty_config_returns_empty_vec() {
        let ids = parse_identities("").expect("parse empty");
        assert!(ids.is_empty());

        let ids = parse_identities("[other]\nkey = 1\n").expect("parse no-email");
        assert!(ids.is_empty());

        let ids = parse_identities("[email]\n").expect("parse email-no-identities");
        assert!(ids.is_empty());
    }

    #[test]
    fn malformed_toml_returns_error() {
        let result = parse_identities("this is not valid toml = = =");
        assert!(result.is_err());
    }

    #[test]
    fn partial_new_shape_is_rejected() {
        // backend present, auth absent → should error, not silently synthesise.
        let bad = r#"
[[email.identities]]
from_email = "x@y.com"

[email.identities.backend]
type = "smtp"
host = "h"
port = 465
encryption = "tls"
username = "u"
auth_mode = "password"
"#;
        let result = parse_identities(bad);
        assert!(result.is_err(), "partial new shape should error");
    }

    #[test]
    fn first_identity_promoted_when_none_marked_primary() {
        let no_primary = r#"
[[email.identities]]
from_email = "a@x.com"
smtp_server = "s"
username = "u"

[[email.identities]]
from_email = "b@x.com"
smtp_server = "s"
username = "u"
"#;
        let ids = parse_identities(no_primary).expect("parse");
        assert_eq!(ids.len(), 2);
        assert!(ids[0].primary);
        assert!(!ids[1].primary);
    }

    #[test]
    fn find_by_from_email_on_parsed_list() {
        // Exercise the same filter logic find_identity uses, without
        // hitting the real filesystem.
        let ids = parse_identities(&format!("{}\n{}", NEW_SHAPE_SMTP, NEW_SHAPE_GRAPH))
            .expect("parse");

        let found = ids
            .iter()
            .find(|i| i.from_email == "will.napier@changeofharleystreet.com");
        assert!(found.is_some());
        assert!(matches!(found.unwrap().config.backend, BackendConfig::Graph(_)));

        let missing = ids.iter().find(|i| i.from_email == "nobody@nowhere.com");
        assert!(missing.is_none());
    }

    #[test]
    fn from_name_defaults_to_from_email_when_absent() {
        let toml = r#"
[[email.identities]]
from_email = "bare@example.com"
smtp_server = "s"
username = "u"
"#;
        let ids = parse_identities(toml).expect("parse");
        assert_eq!(ids[0].from_name, "bare@example.com");
    }
}
