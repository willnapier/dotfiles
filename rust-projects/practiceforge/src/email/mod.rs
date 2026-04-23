//! Email — vendor-neutral mail transport abstraction.
//!
//! - [`transport`]: the [`MailTransport`] trait, [`Envelope`], [`Body`],
//!   [`Mailbox`], [`Attachment`] types. Every backend speaks this contract.
//! - [`auth`]: the [`TokenSource`] trait and its impls — the credential layer
//!   separate from the transport layer. This split lets SMTP use either a
//!   password or an XOAUTH2 token, and lets Graph use any OAuth2 token
//!   producer (keychain-stored, command-based, etc.).
//! - [`backends`]: concrete `MailTransport` impls — SMTP, Graph, others to
//!   come. `backends::transport_for(&identity)` dispatches to the right one.
//! - [`config`]: parses `~/.config/practiceforge/config.toml` into
//!   [`Identity`] values. Both the new tagged shape and the pre-Phase-4
//!   flat shape are accepted.
//! - [`wizard`]: interactive multi-backend setup.
//!
//! The crate-level convenience entry point is [`send_as`] — a thin facade
//! that wraps "look up identity → build envelope → dispatch via backend"
//! so callers don't have to know about the trait plumbing.

pub mod transport;
pub mod auth;
pub mod backends;
pub mod config;
pub mod wizard;
pub mod m365_oauth;
pub mod m365_imap_oauth;
pub mod gmail_oauth;
pub mod gmail_push_tags;
pub mod gmail_pull;

pub use transport::{Attachment, Body, Envelope, MailTransport, Mailbox};
pub use auth::TokenSource;
pub use config::{find_identity, load_identities, primary_identity, Identity};

use anyhow::{Context, Result};
use std::path::Path;

use crate::email::backends::transport_for;

/// Send an email as the identity identified by `from_email`.
///
/// Thin facade over `find_identity` + `transport_for` + `Envelope::builder`
/// + `transport.send`. Replaces the legacy `send_email` / `send_email_from`
/// entry points with a single signature that covers all previous call
/// shapes:
///
/// - Plain text body → pass `Body::Text(..)`.
/// - HTML body (invoice emails) → pass `Body::Html(..)`.
/// - Optional attachment → `Some(&Path)`; content type is inferred from
///   extension (currently `.pdf` only — other callers didn't need more).
/// - Optional CC list → slice of bare email addresses.
///
/// Returns `Err` with clear context when no identity is configured for
/// `from_email`, when the backend fails, or when the attachment can't be
/// read.
pub fn send_as(
    from_email: &str,
    to_email: &str,
    to_name: &str,
    subject: &str,
    body: Body,
    attachment: Option<&Path>,
    cc: Option<&[String]>,
) -> Result<()> {
    let identity = find_identity(from_email)
        .ok_or_else(|| anyhow::anyhow!("No email identity configured for {}", from_email))?;

    let from = Mailbox::with_name(&identity.from_email, &identity.from_name);
    let to = if to_name.is_empty() {
        Mailbox::new(to_email)
    } else {
        Mailbox::with_name(to_email, to_name)
    };

    let mut builder = Envelope::builder(from)
        .to(to)
        .subject(subject)
        .body(body);

    if let Some(addrs) = cc {
        for addr in addrs {
            let trimmed = addr.trim();
            if !trimmed.is_empty() {
                builder = builder.cc(Mailbox::new(trimmed));
            }
        }
    }

    if let Some(path) = attachment {
        let data = std::fs::read(path)
            .with_context(|| format!("Could not read attachment {}", path.display()))?;
        let filename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let content_type = content_type_for(path);
        builder = builder.attachment(Attachment {
            filename,
            content_type,
            data,
        });
    }

    let envelope = builder.build();
    let transport = transport_for(&identity.config)?;
    transport.send(&envelope)
}

/// Guess a MIME type from file extension. Deliberately minimal — the only
/// attachment callers in this crate produce PDFs. Extend as new callers
/// appear.
fn content_type_for(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("pdf") => "application/pdf".into(),
        _ => "application/octet-stream".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::email::backends::{
        AuthConfig, BackendConfig, Encryption, IdentityConfig, SmtpConfig,
        smtp::AuthMode,
    };

    /// Build an SMTP+password IdentityConfig by hand, and verify that
    /// `transport_for` returns something whose `name()` reflects the chosen
    /// host/port. This covers the facade's transport-dispatch path without
    /// needing a real SMTP server.
    #[test]
    fn facade_dispatches_smtp_password_transport() {
        let identity = IdentityConfig {
            backend: BackendConfig::Smtp(SmtpConfig {
                host: "smtp.example.org".into(),
                port: 587,
                encryption: Encryption::StartTls,
                username: "u@example.org".into(),
                auth_mode: AuthMode::Password,
            }),
            auth: AuthConfig::Password {
                keyring_service: "facade-test".into(),
                keyring_account: "u@example.org".into(),
            },
        };
        let transport = transport_for(&identity).expect("dispatch should succeed");
        assert!(transport.name().contains("smtp.example.org"));
        assert!(transport.name().contains("587"));
    }

    /// `send_as` for a from_email with no matching identity should surface
    /// a clear "No email identity configured" error — not a generic failure.
    /// Uses a highly unlikely address so we don't collide with a real
    /// config.toml if one exists on the test machine.
    #[test]
    fn send_as_unknown_identity_reports_missing_config() {
        let result = send_as(
            "definitely-not-configured-xyz@nowhere.invalid",
            "to@example.com",
            "",
            "subject",
            Body::Text("body".into()),
            None,
            None,
        );
        let err = result.expect_err("expected missing-identity error");
        let msg = format!("{}", err);
        assert!(
            msg.contains("No email identity configured"),
            "expected missing-identity message, got: {}",
            msg
        );
    }
}
