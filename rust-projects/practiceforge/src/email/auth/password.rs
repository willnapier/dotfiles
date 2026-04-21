//! Password credential — read from OS keychain.
//!
//! Phase 0 stub. Phase 1 will port the working keychain lookup from
//! `email::legacy::load_password` into this impl.

use anyhow::Result;

use super::TokenSource;

/// Reads a password from the OS keychain.
///
/// macOS: `security find-generic-password -s <service> -a <account> -w`
/// Linux: `secret-tool lookup service <service> account <account>`
pub struct KeychainPasswordSource {
    pub service: String,
    pub account: String,
}

impl KeychainPasswordSource {
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self { service: service.into(), account: account.into() }
    }
}

impl TokenSource for KeychainPasswordSource {
    fn access_token(&self) -> Result<String> {
        // Phase 1: port from `email::legacy::load_password`.
        todo!("Phase 1: port keychain lookup from legacy::load_password")
    }
}
