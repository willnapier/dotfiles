//! In-Rust OAuth `TokenSource` — refresh on read.
//!
//! Replaces the [`CommandTokenSource`] pattern that shells out to
//! `cohs-oauth-graph show` for the COHS M365 case. The Python helper has
//! been the long-standing way to expose a freshly-refreshed access token
//! to consumers like `GraphTransport`. This module does the same thing
//! in-process so the binary works on Windows (where Python is not
//! installed by default) and removes ~50–150 ms of subprocess latency
//! per token fetch on Mac/Linux.
//!
//! ## How it stays correct
//!
//! 1. Read the cached access token + its persisted expiry from the
//!    keystore (both written by [`crate::email::m365_oauth::store_tokens`]
//!    after every successful poll/refresh).
//! 2. If the access token is missing, OR no expiry is recorded, OR the
//!    expiry is within `refresh_buffer_secs` of now — invoke the
//!    refresh callback. The callback is responsible for hitting the
//!    OAuth provider's `/token` endpoint and persisting the result back
//!    into the keystore.
//! 3. Re-read the access token. If it's still missing, error clearly —
//!    the refresh "succeeded" without writing tokens, which indicates a
//!    bug rather than a network failure.
//!
//! ## Why proactive over reactive
//!
//! A reactive design ("always return cached; on 401 from API, refresh
//! and retry") pushes complexity onto every caller. With proactive
//! refresh the [`TokenSource`] contract stays as it is: callers get a
//! valid token or an error, never a maybe-expired one.
//!
//! ## Concurrency
//!
//! Two threads racing on `access_token()` could each trigger a refresh.
//! That's wasteful but safe — OAuth refresh is idempotent (the refresh
//! token stays valid; each call mints a new access token). A `Mutex`
//! around the refresh call would serialise this; not yet needed in
//! practice.

use anyhow::{anyhow, Context, Result};

use super::TokenSource;

/// OAuth access-token source backed by the OS keystore + a refresh
/// callback.
///
/// Construct via [`for_m365`](Self::for_m365) for the COHS Microsoft 365
/// case, or via [`new`](Self::new) when wiring a different provider.
pub struct KeychainOAuthTokenSource {
    service: String,
    access_account: String,
    expiry_account: String,
    /// Provider-specific refresh. Must hit the OAuth `/token` endpoint
    /// and persist the new access token (and ideally a new expiry) back
    /// to the same `service`/`access_account` keystore slot before
    /// returning Ok.
    refresh: fn() -> Result<()>,
    /// Returns the current Unix timestamp in seconds. Production wires
    /// this to `chrono::Utc::now().timestamp()`; tests inject a fake.
    now_secs: fn() -> i64,
    /// Refresh proactively if `expires_at` is within this many seconds
    /// of now. Five minutes covers normal HTTP roundtrip + clock skew.
    refresh_buffer_secs: i64,
}

impl KeychainOAuthTokenSource {
    /// Generic constructor — used by tests and any future non-M365 OAuth
    /// provider that wants the same refresh-on-read pattern.
    pub fn new(
        service: impl Into<String>,
        access_account: impl Into<String>,
        expiry_account: impl Into<String>,
        refresh: fn() -> Result<()>,
        now_secs: fn() -> i64,
        refresh_buffer_secs: i64,
    ) -> Self {
        Self {
            service: service.into(),
            access_account: access_account.into(),
            expiry_account: expiry_account.into(),
            refresh,
            now_secs,
            refresh_buffer_secs,
        }
    }

    /// Pre-wired for the COHS Microsoft 365 case — keystore service,
    /// account, expiry account and refresh function all hardcoded to
    /// the constants in [`crate::email::m365_oauth`].
    pub fn for_m365() -> Self {
        Self {
            service: crate::email::m365_oauth::KEYCHAIN_SERVICE.to_string(),
            access_account: crate::email::m365_oauth::KEY_ACCESS.to_string(),
            expiry_account: crate::email::m365_oauth::KEY_EXPIRES_AT.to_string(),
            refresh: crate::email::m365_oauth::refresh,
            now_secs: || chrono::Utc::now().timestamp(),
            refresh_buffer_secs: 300,
        }
    }

    /// True if the cached access token is present AND its expiry is
    /// known AND we're more than `refresh_buffer_secs` away from it.
    /// Any other state returns false → refresh.
    fn is_fresh(&self) -> Result<bool> {
        let access = crate::keystore::get(&self.service, &self.access_account)
            .with_context(|| {
                format!(
                    "reading {}/{} from keystore",
                    self.service, self.access_account
                )
            })?;
        if access.is_none() {
            return Ok(false);
        }

        let expiry = crate::keystore::get(&self.service, &self.expiry_account)
            .with_context(|| {
                format!(
                    "reading {}/{} from keystore",
                    self.service, self.expiry_account
                )
            })?;
        let Some(expiry_str) = expiry else {
            return Ok(false);
        };

        let expires_at: i64 = expiry_str.trim().parse().with_context(|| {
            format!(
                "malformed expires_at in keystore at {}/{}: {expiry_str:?}",
                self.service, self.expiry_account
            )
        })?;

        Ok((self.now_secs)() < expires_at - self.refresh_buffer_secs)
    }
}

impl TokenSource for KeychainOAuthTokenSource {
    fn access_token(&self) -> Result<String> {
        if !self.is_fresh()? {
            (self.refresh)().with_context(|| {
                format!(
                    "refreshing OAuth token for {}/{}",
                    self.service, self.access_account
                )
            })?;
        }

        crate::keystore::get(&self.service, &self.access_account)?.ok_or_else(|| {
            anyhow!(
                "no access token in keystore at {}/{} after refresh — \
                 refresh callback returned Ok but did not persist a token",
                self.service,
                self.access_account,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keystore;

    // Each test uses a unique service name so parallel test execution
    // and any concurrent process activity stay isolated.

    fn cleanup(svc: &str) {
        let _ = keystore::delete(svc, "access");
        let _ = keystore::delete(svc, "expires");
    }

    #[test]
    fn returns_cached_token_when_fresh() {
        let svc = "test-keychain-oauth-fresh";
        cleanup(svc);
        keystore::set(svc, "access", "cached-token").unwrap();
        // Expires far in the future.
        keystore::set(svc, "expires", "2_000_000_000".replace('_', "").as_str()).unwrap();

        fn must_not_call() -> Result<()> {
            panic!("refresh must not be called when token is fresh");
        }
        let src = KeychainOAuthTokenSource::new(
            svc,
            "access",
            "expires",
            must_not_call,
            || 1_700_000_000,
            300,
        );
        let token = src.access_token().expect("access_token should succeed");
        assert_eq!(token, "cached-token");

        cleanup(svc);
    }

    #[test]
    fn refreshes_when_token_expired() {
        let svc = "test-keychain-oauth-expired";
        cleanup(svc);
        keystore::set(svc, "access", "stale-token").unwrap();
        keystore::set(svc, "expires", "1700000000").unwrap();

        fn fake_refresh() -> Result<()> {
            keystore::set("test-keychain-oauth-expired", "access", "fresh-token")?;
            keystore::set("test-keychain-oauth-expired", "expires", "2000000000")?;
            Ok(())
        }
        let src = KeychainOAuthTokenSource::new(
            svc,
            "access",
            "expires",
            fake_refresh,
            // After the stored expiry → not fresh → refresh fires.
            || 1_800_000_000,
            300,
        );
        let token = src.access_token().expect("access_token should succeed");
        assert_eq!(token, "fresh-token");

        cleanup(svc);
    }

    #[test]
    fn refreshes_when_within_buffer() {
        let svc = "test-keychain-oauth-buffer";
        cleanup(svc);
        keystore::set(svc, "access", "soon-expired-token").unwrap();
        // Expires at 1_700_000_300; buffer is 300; now is 1_700_000_001.
        // expires_at - buffer = 1_700_000_000; now (1_700_000_001) is
        // NOT < 1_700_000_000 → not fresh → refresh.
        keystore::set(svc, "expires", "1700000300").unwrap();

        fn fake_refresh() -> Result<()> {
            keystore::set("test-keychain-oauth-buffer", "access", "buffer-refreshed")?;
            keystore::set("test-keychain-oauth-buffer", "expires", "1700003900")?;
            Ok(())
        }
        let src = KeychainOAuthTokenSource::new(
            svc,
            "access",
            "expires",
            fake_refresh,
            || 1_700_000_001,
            300,
        );
        let token = src.access_token().expect("access_token should succeed");
        assert_eq!(token, "buffer-refreshed");

        cleanup(svc);
    }

    #[test]
    fn refreshes_when_no_expiry_stored() {
        let svc = "test-keychain-oauth-no-exp";
        cleanup(svc);
        keystore::set(svc, "access", "legacy-token").unwrap();
        // Deliberately no expiry entry — simulates an entry written by
        // the Python helper or by m365_oauth before this Phase C work.

        fn fake_refresh() -> Result<()> {
            keystore::set("test-keychain-oauth-no-exp", "access", "post-legacy-refresh")?;
            keystore::set("test-keychain-oauth-no-exp", "expires", "2000000000")?;
            Ok(())
        }
        let src = KeychainOAuthTokenSource::new(
            svc,
            "access",
            "expires",
            fake_refresh,
            || 1_700_000_000,
            300,
        );
        let token = src.access_token().expect("access_token should succeed");
        assert_eq!(token, "post-legacy-refresh");

        cleanup(svc);
    }

    #[test]
    fn refreshes_when_no_access_token_stored() {
        let svc = "test-keychain-oauth-no-access";
        cleanup(svc);
        // Deliberately store nothing.

        fn fake_refresh() -> Result<()> {
            keystore::set("test-keychain-oauth-no-access", "access", "first-token")?;
            keystore::set("test-keychain-oauth-no-access", "expires", "2000000000")?;
            Ok(())
        }
        let src = KeychainOAuthTokenSource::new(
            svc,
            "access",
            "expires",
            fake_refresh,
            || 1_700_000_000,
            300,
        );
        let token = src.access_token().expect("access_token should succeed");
        assert_eq!(token, "first-token");

        cleanup(svc);
    }

    #[test]
    fn errors_when_refresh_does_not_persist() {
        let svc = "test-keychain-oauth-no-persist";
        cleanup(svc);

        fn buggy_refresh() -> Result<()> {
            // Returns Ok without writing anything — simulates a
            // refresh implementation bug.
            Ok(())
        }
        let src = KeychainOAuthTokenSource::new(
            svc,
            "access",
            "expires",
            buggy_refresh,
            || 1_700_000_000,
            300,
        );
        let err = src
            .access_token()
            .expect_err("should error when no token after refresh");
        let msg = format!("{err}");
        assert!(
            msg.contains("after refresh") || msg.contains("did not persist"),
            "error should explain the buggy-refresh case, got: {msg}"
        );

        cleanup(svc);
    }

    #[test]
    fn malformed_expiry_propagates_as_error() {
        let svc = "test-keychain-oauth-bad-expiry";
        cleanup(svc);
        keystore::set(svc, "access", "tok").unwrap();
        keystore::set(svc, "expires", "not-a-number").unwrap();

        fn must_not_call() -> Result<()> {
            panic!("refresh must not be called when expiry is malformed");
        }
        let src = KeychainOAuthTokenSource::new(
            svc,
            "access",
            "expires",
            must_not_call,
            || 1_700_000_000,
            300,
        );
        let err = src.access_token().expect_err("should error on malformed expiry");
        let msg = format!("{err}");
        assert!(
            msg.contains("malformed expires_at"),
            "error should call out malformed expiry, got: {msg}"
        );

        cleanup(svc);
    }
}
