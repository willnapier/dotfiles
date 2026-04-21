//! SMTP backend — sends via `lettre` over STARTTLS (587) or implicit TLS (465).
//!
//! Auth modes supported (Phase 1 will implement):
//! - `AUTH PLAIN` / `AUTH LOGIN` with a password (legacy mode — Gmail with
//!   app-password, Exchange on-prem, any generic host).
//! - `AUTH XOAUTH2` with an OAuth2 access token (Gmail modern, any tenant
//!   that has enabled SMTP AUTH).
//!
//! The choice between auth modes is driven by the `AuthMode` variant in
//! [`SmtpConfig`]. Both take a [`TokenSource`] — what differs is how the
//! returned string is handed to `lettre` (as password vs as XOAUTH2 token).
//!
//! Phase 0 stub.

use anyhow::Result;
use std::sync::Arc;

use crate::email::{Envelope, MailTransport, TokenSource};

/// Encryption posture for the SMTP connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encryption {
    /// Implicit TLS from connect (port 465).
    Tls,
    /// Plaintext then upgrade with STARTTLS (port 587).
    StartTls,
}

/// How to authenticate to the SMTP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    Password,
    XOAuth2,
}

/// Configuration for [`SmtpTransport`].
#[derive(Debug, Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub encryption: Encryption,
    pub username: String,
    pub auth_mode: AuthMode,
    // `TokenSource` carried at runtime; config itself is serializable, so
    // the live transport holds the TokenSource separately (see SmtpTransport::new).
}

pub struct SmtpTransport {
    config: SmtpConfig,
    token_source: Arc<dyn TokenSource>,
    display_name: String,
}

impl SmtpTransport {
    pub fn new(config: SmtpConfig, token_source: Arc<dyn TokenSource>) -> Self {
        let display_name = format!("SMTP ({}:{})", config.host, config.port);
        Self { config, token_source, display_name }
    }
}

impl MailTransport for SmtpTransport {
    fn send(&self, _envelope: &Envelope) -> Result<()> {
        // Phase 1: port from `email::legacy::send_email_with_password`.
        // Handle both AuthMode::Password and AuthMode::XOAuth2 — lettre
        // supports XOAUTH2 via `Mechanism::Xoauth2`.
        let _ = (&self.config, &self.token_source);
        todo!("Phase 1: port SMTP send from legacy module behind this trait")
    }

    fn name(&self) -> &str {
        &self.display_name
    }
}
