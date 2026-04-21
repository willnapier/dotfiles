//! Backends — concrete [`MailTransport`] implementations.
//!
//! Dispatch lives here: [`transport_for`] takes a configured identity
//! (described by `BackendConfig`) and returns the right boxed transport.
//!
//! Each backend is self-contained: it knows its own wire protocol and
//! delegates credential acquisition to a [`TokenSource`]. Adding a new
//! backend (Amazon SES, Mailgun, JMAP, ...) means creating one file here
//! and updating the `match` arm in `transport_for`.

use anyhow::Result;

pub mod smtp;
pub mod graph;

pub use smtp::SmtpTransport;
pub use graph::GraphTransport;

use crate::email::MailTransport;

/// Which backend to use for an identity. Phase 1 will add the parser that
/// builds `BackendConfig` from the `[email.identities.backend]` TOML table
/// (with backwards-compatible fallback to the flat legacy shape).
#[derive(Debug, Clone)]
pub enum BackendConfig {
    /// SMTP submission — port 465 implicit TLS or 587 STARTTLS.
    Smtp(smtp::SmtpConfig),
    /// Microsoft Graph `/me/sendMail` — for tenants where SMTP AUTH is
    /// disabled but Graph is available.
    Graph(graph::GraphConfig),
}

/// Dispatch: given a backend config, return the matching transport.
///
/// Phase 0 stub — returns an error until Phase 1/2 fill in the impls.
pub fn transport_for(config: &BackendConfig) -> Result<Box<dyn MailTransport>> {
    match config {
        BackendConfig::Smtp(_) => todo!("Phase 1: return Box::new(SmtpTransport::new(cfg))"),
        BackendConfig::Graph(_) => todo!("Phase 2: return Box::new(GraphTransport::new(cfg))"),
    }
}
