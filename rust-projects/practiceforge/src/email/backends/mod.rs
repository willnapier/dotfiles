//! Backends — concrete [`MailTransport`] implementations + dispatch.
//!
//! The trait lives in [`crate::email::transport`]. Each backend is
//! self-contained: knows its wire protocol, delegates credential acquisition
//! to a [`TokenSource`]. Adding a new backend (SES, Mailgun, JMAP, ...) is
//! one file here plus a match arm in [`transport_for`].
//!
//! ### Config shape
//!
//! An identity has two halves:
//! - **[`BackendConfig`]** — where and how to deliver (SMTP / Graph / ...).
//! - **[`AuthConfig`]** — where the credential comes from
//!   (keychain password, shell-out to OAuth helper).
//!
//! Bundled into [`IdentityConfig`], which [`transport_for`] consumes to
//! produce a ready-to-send `Box<dyn MailTransport>`.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub mod smtp;
pub mod graph;

pub use smtp::{AuthMode, Encryption, SmtpConfig, SmtpTransport};
pub use graph::{GraphConfig, GraphTransport};

use crate::email::auth::{CommandTokenSource, KeychainPasswordSource, TokenSource};
use crate::email::MailTransport;

/// Which backend delivers mail for this identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum BackendConfig {
    /// SMTP submission — port 465 implicit TLS or 587 STARTTLS.
    Smtp(SmtpConfig),
    /// Microsoft Graph `/me/sendMail` — for tenants where SMTP AUTH is
    /// disabled but Graph is available.
    Graph(GraphConfig),
}

/// Where the credential for this identity comes from.
///
/// Deliberately separate from [`BackendConfig`] so the same auth strategy
/// can be paired with different backends (e.g. OAuth2 command works for
/// both SMTP XOAUTH2 and Graph).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthConfig {
    /// Password retrieved from OS keychain (macOS Keychain / libsecret).
    Password {
        keyring_service: String,
        keyring_account: String,
    },
    /// Access token produced by shelling out to a command (e.g.
    /// `"cohs-oauth show"`). The command is responsible for OAuth refresh.
    //
    // Explicit rename: `rename_all = "snake_case"` would turn `OAuth2Command`
    // into `o_auth2_command`, which doesn't match the spec's `oauth2_command`.
    #[serde(rename = "oauth2_command")]
    OAuth2Command { command: String },
}

/// A complete send-identity configuration: what backend, what auth.
///
/// Wizard and config-loader build these; [`transport_for`] consumes them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityConfig {
    pub backend: BackendConfig,
    pub auth: AuthConfig,
}

/// Build a live [`MailTransport`] for the given identity config.
///
/// Constructs the appropriate [`TokenSource`] from `auth`, then hands it to
/// the concrete backend constructor. The result is trait-object boxed so
/// callers can hold different backends uniformly.
pub fn transport_for(identity: &IdentityConfig) -> Result<Box<dyn MailTransport>> {
    let source = build_token_source(&identity.auth);
    Ok(match &identity.backend {
        BackendConfig::Smtp(cfg) => Box::new(SmtpTransport::new(cfg.clone(), source)),
        BackendConfig::Graph(cfg) => Box::new(GraphTransport::new(cfg.clone(), source)),
    })
}

/// Construct a `TokenSource` from its config. Infallible today — both
/// variants just wrap the config fields into structs. Returning a value
/// rather than a `Result` keeps `transport_for` simpler; if a future
/// backend needs fallible construction (e.g. validating a command exists),
/// wrap this in `Result` then.
fn build_token_source(auth: &AuthConfig) -> Arc<dyn TokenSource> {
    match auth {
        AuthConfig::Password { keyring_service, keyring_account } => {
            Arc::new(KeychainPasswordSource::new(
                keyring_service.clone(),
                keyring_account.clone(),
            ))
        }
        AuthConfig::OAuth2Command { command } => {
            Arc::new(CommandTokenSource::new(command.clone()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::email::backends::graph::GraphConfig;
    use crate::email::backends::smtp::{AuthMode, Encryption, SmtpConfig};

    #[test]
    fn dispatch_smtp_password() {
        let identity = IdentityConfig {
            backend: BackendConfig::Smtp(SmtpConfig {
                host: "smtp.example.com".into(),
                port: 465,
                encryption: Encryption::Tls,
                username: "u@example.com".into(),
                auth_mode: AuthMode::Password,
            }),
            auth: AuthConfig::Password {
                keyring_service: "test-dispatch".into(),
                keyring_account: "u@example.com".into(),
            },
        };
        let transport = transport_for(&identity).expect("dispatch should succeed");
        assert!(transport.name().starts_with("SMTP"));
    }

    #[test]
    fn dispatch_smtp_xoauth2() {
        let identity = IdentityConfig {
            backend: BackendConfig::Smtp(SmtpConfig {
                host: "smtp.gmail.com".into(),
                port: 465,
                encryption: Encryption::Tls,
                username: "u@gmail.com".into(),
                auth_mode: AuthMode::XOAuth2,
            }),
            auth: AuthConfig::OAuth2Command { command: "echo fake-token".into() },
        };
        let transport = transport_for(&identity).expect("dispatch should succeed");
        assert!(transport.name().contains("smtp.gmail.com"));
    }

    #[test]
    fn dispatch_graph() {
        let identity = IdentityConfig {
            backend: BackendConfig::Graph(GraphConfig::default()),
            auth: AuthConfig::OAuth2Command { command: "cohs-oauth show".into() },
        };
        let transport = transport_for(&identity).expect("dispatch should succeed");
        assert_eq!(transport.name(), "Microsoft Graph");
    }
}
