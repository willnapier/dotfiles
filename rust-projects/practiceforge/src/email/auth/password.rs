//! Password credential — read from OS keychain.
//!
//! Backed by `crate::keystore` (which abstracts Keychain Services on macOS,
//! Credential Manager on Windows, libsecret on Linux). Schema is unchanged
//! from the prior `Command::new("security" | "secret-tool")` implementation
//! — same `service`+`account` attributes, so existing entries are read
//! transparently.

use anyhow::{bail, Context, Result};

use super::TokenSource;

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
        let secret = crate::keystore::get(&self.service, &self.account)
            .context("reading password from keystore")?;

        let Some(secret) = secret else {
            bail!(
                "No password found in keychain for service='{}' account='{}'.\n\
                 Store it with the practiceforge email wizard, or directly:\n  \
                 macOS:  security add-generic-password -s {} -a '{}' -w '<password>'\n  \
                 Linux:  secret-tool store --label '{}' service {} account '{}'",
                self.service,
                self.account,
                self.service,
                self.account,
                self.service,
                self.service,
                self.account,
            );
        };

        if secret.is_empty() {
            bail!(
                "Keychain returned empty password for service='{}' account='{}'",
                self.service,
                self.account
            );
        }

        Ok(secret)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keystore;

    #[test]
    fn keychain_round_trip() {
        let service = "test-pf-password-source";
        let account = "round-trip";
        let password = "testpw123";

        let _ = keystore::delete(service, account);
        keystore::set(service, account, password).expect("seed");

        let src = KeychainPasswordSource::new(service, account);
        let got = src.access_token().expect("should read back password");
        assert_eq!(got, password);

        let _ = keystore::delete(service, account);
    }

    #[test]
    fn keychain_missing_entry_errors() {
        let src = KeychainPasswordSource::new(
            "test-pf-password-source",
            "definitely-not-present-xyz",
        );
        let result = src.access_token();
        assert!(result.is_err(), "expected error for missing keychain entry");
    }
}
