//! Password credential — read from OS keychain.
//!
//! Ported from `email::legacy::load_password` in Phase 1.

use anyhow::{bail, Context, Result};
use std::process::Command;

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
        let output = if cfg!(target_os = "macos") {
            Command::new("security")
                .args([
                    "find-generic-password",
                    "-s",
                    &self.service,
                    "-a",
                    &self.account,
                    "-w",
                ])
                .output()
                .context("Failed to read macOS keychain")?
        } else {
            Command::new("secret-tool")
                .args([
                    "lookup",
                    "service",
                    &self.service,
                    "account",
                    &self.account,
                ])
                .output()
                .context("Failed to read secret-service")?
        };

        if !output.status.success() {
            bail!(
                "No password found in keychain for service='{}' account='{}'.\n\
                 Store it with:\n  \
                 security add-generic-password -s {} -a '{}' -w '<password>'",
                self.service,
                self.account,
                self.service,
                self.account
            );
        }

        let secret = String::from_utf8(output.stdout)
            .context("Keychain returned non-UTF8 bytes")?
            .trim_end_matches(&['\r', '\n'][..])
            .to_string();

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

    #[test]
    #[cfg(target_os = "macos")]
    fn keychain_round_trip() {
        // Use a unique service to avoid collisions with real keychain entries.
        let service = "test-pf-phase1";
        let account = "phase1-test";
        let password = "testpw123";

        // Clean up any stale entry first (ignore failures).
        let _ = Command::new("security")
            .args(["delete-generic-password", "-s", service, "-a", account])
            .output();

        // Add the entry.
        let add = Command::new("security")
            .args([
                "add-generic-password",
                "-s",
                service,
                "-a",
                account,
                "-w",
                password,
            ])
            .status()
            .expect("security add-generic-password should run");
        assert!(add.success(), "failed to add keychain entry for test");

        // Read back via our TokenSource.
        let src = KeychainPasswordSource::new(service, account);
        let got = src.access_token().expect("should read back password");
        assert_eq!(got, password);

        // Clean up.
        let _ = Command::new("security")
            .args(["delete-generic-password", "-s", service, "-a", account])
            .output();
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn keychain_missing_entry_errors() {
        let src = KeychainPasswordSource::new(
            "test-pf-phase1-definitely-not-present",
            "no-such-account",
        );
        let result = src.access_token();
        assert!(result.is_err(), "expected error for missing keychain entry");
    }
}
