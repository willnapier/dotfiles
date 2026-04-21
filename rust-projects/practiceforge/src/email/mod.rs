//! Email — vendor-neutral mail transport abstraction.
//!
//! Phase 0 scaffolding (2026-04-21). The module exposes:
//!
//! - [`transport`]: the [`MailTransport`] trait, [`Envelope`], [`Body`],
//!   [`Mailbox`], [`Attachment`] types. Every backend speaks this contract.
//! - [`auth`]: the [`TokenSource`] trait and its impls — the credential layer
//!   separate from the transport layer. This split lets SMTP use either a
//!   password or an XOAUTH2 token, and lets Graph use any OAuth2 token
//!   producer (keychain-stored, command-based, etc.).
//! - [`backends`]: concrete `MailTransport` impls — SMTP, Graph, others to
//!   come. `backends::transport_for(&identity)` dispatches to the right one.
//!
//! The existing `legacy` submodule contains the pre-refactor SMTP-only
//! implementation. Its API is re-exported here unchanged so all current
//! callers keep working. Phases 1–3 migrate behaviour into the new
//! backends and finally retire `legacy`.

pub mod transport;
pub mod auth;
pub mod backends;

// Legacy API — untouched during migration. Callers continue to work.
mod legacy;
pub use legacy::*;

// New API — currently stubs; Phase 1+ fill them in.
pub use transport::{Attachment, Body, Envelope, MailTransport, Mailbox};
pub use auth::TokenSource;
