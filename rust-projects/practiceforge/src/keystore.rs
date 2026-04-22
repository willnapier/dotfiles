//! Cross-platform secret storage.
//!
//! Single API over three OS keystores:
//! - **macOS**: Keychain Services via the `keyring` crate (`apple-native`).
//! - **Windows**: Credential Manager via the `keyring` crate (`windows-native`).
//! - **Linux**: libsecret via the `secret-service` crate, **directly** —
//!   bypassing `keyring` so we can keep our existing `service`+`account`
//!   attribute schema. The `keyring` crate hardcodes `username` and
//!   `target=default` attributes which would not match entries written by
//!   the prior `secret-tool ... service X account Y` calls.
//!
//! All three backends agree on the public surface:
//!
//! ```ignore
//! keystore::set("clinical-email", "will@willnapier.com", "secret")?;
//! let v = keystore::get("clinical-email", "will@willnapier.com")?;
//! keystore::delete("clinical-email", "will@willnapier.com")?;
//! ```

use anyhow::Result;

#[cfg(target_os = "linux")]
mod backend {
    use anyhow::{Context, Result};
    use secret_service::blocking::SecretService;
    use secret_service::EncryptionType;
    use std::collections::HashMap;

    fn attrs<'a>(service: &'a str, account: &'a str) -> HashMap<&'a str, &'a str> {
        let mut m = HashMap::new();
        m.insert("service", service);
        m.insert("account", account);
        m
    }

    pub fn get(service: &str, account: &str) -> Result<Option<String>> {
        let ss = SecretService::connect(EncryptionType::Dh)
            .context("connecting to secret-service")?;
        let coll = ss.get_default_collection().context("opening default collection")?;
        let items = coll
            .search_items(attrs(service, account))
            .context("searching libsecret")?;
        let Some(item) = items.first() else { return Ok(None) };
        let secret = item.get_secret().context("reading secret bytes")?;
        let s = String::from_utf8(secret).context("secret is not UTF-8")?;
        Ok(Some(s))
    }

    pub fn set(service: &str, account: &str, value: &str) -> Result<()> {
        let ss = SecretService::connect(EncryptionType::Dh)
            .context("connecting to secret-service")?;
        let coll = ss.get_default_collection().context("opening default collection")?;
        let label = format!("{service}:{account}");
        coll.create_item(
            &label,
            attrs(service, account),
            value.as_bytes(),
            true,
            "text/plain",
        )
        .context("writing libsecret item")?;
        Ok(())
    }

    pub fn delete(service: &str, account: &str) -> Result<()> {
        let ss = SecretService::connect(EncryptionType::Dh)
            .context("connecting to secret-service")?;
        let coll = ss.get_default_collection().context("opening default collection")?;
        let items = coll
            .search_items(attrs(service, account))
            .context("searching libsecret")?;
        for item in items {
            item.delete().context("deleting libsecret item")?;
        }
        Ok(())
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod backend {
    use anyhow::{Context, Result};

    pub fn get(service: &str, account: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(service, account).context("creating keyring entry")?;
        match entry.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e).context("reading keyring entry"),
        }
    }

    pub fn set(service: &str, account: &str, value: &str) -> Result<()> {
        let entry = keyring::Entry::new(service, account).context("creating keyring entry")?;
        entry.set_password(value).context("writing keyring entry")
    }

    pub fn delete(service: &str, account: &str) -> Result<()> {
        let entry = keyring::Entry::new(service, account).context("creating keyring entry")?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e).context("deleting keyring entry"),
        }
    }
}

pub fn get(service: &str, account: &str) -> Result<Option<String>> {
    backend::get(service, account)
}

pub fn set(service: &str, account: &str, value: &str) -> Result<()> {
    backend::set(service, account, value)
}

pub fn delete(service: &str, account: &str) -> Result<()> {
    backend::delete(service, account)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let service = "pf-keystore-test";
        let account = "round-trip-test";
        let secret = "s3cret-value-123";

        let _ = delete(service, account);

        set(service, account, secret).expect("set should succeed");
        let got = get(service, account).expect("get should succeed");
        assert_eq!(got.as_deref(), Some(secret));

        delete(service, account).expect("delete should succeed");
        let after = get(service, account).expect("get after delete should succeed");
        assert_eq!(after, None);
    }

    #[test]
    fn missing_returns_none() {
        let got = get("pf-keystore-test", "definitely-not-present-xyz").expect("get ok");
        assert_eq!(got, None);
    }
}
