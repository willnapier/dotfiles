//! Auth — the credential layer, kept separate from the transport layer.
//!
//! A `MailTransport` does not hardcode how it gets its credential. It asks
//! a [`TokenSource`]. This lets us compose:
//!
//! - SMTP + password-from-keychain (legacy shape)
//! - SMTP + XOAUTH2-from-command (e.g. Gmail via oauth2ms)
//! - Graph + OAuth-from-command (our COHS case: token from `cohs-oauth show`)
//! - Any future backend + any future credential store
//!
//! The trait is simple: return a credential string on demand. What that
//! string IS depends on the auth scheme — for `AUTH PLAIN` it's a password,
//! for `AUTH XOAUTH2` it's an access token. Backends know which scheme they
//! use; the `TokenSource` just supplies the bytes.

use anyhow::Result;

pub mod password;
pub mod oauth_command;

pub use password::KeychainPasswordSource;
pub use oauth_command::CommandTokenSource;

/// Credential provider. The term "token" is used generically — for password
/// backends it's a password, for OAuth backends it's an access token.
///
/// Implementations must be fast to call (expect invocation per-send or
/// per-connection). Long-running refresh logic belongs outside this trait
/// (e.g. a launchd timer calling `cohs-oauth refresh` independently).
pub trait TokenSource: Send + Sync {
    fn access_token(&self) -> Result<String>;
}
