//! OAuth shim — delegates entirely to `pizauth show gmail`.
//!
//! The token is short-lived (~1h). Callers should re-fetch via
//! [`access_token`] before each batch of API calls; pizauth caches
//! and refreshes transparently. We never call Google's OAuth
//! endpoints directly — that's pizauth's job.
//!
//! ## Why subprocess instead of pizauth's IPC
//!
//! pizauth exposes a Unix socket as well as the `show` CLI; using
//! the CLI keeps gmpull free of pizauth as a Rust dependency and
//! matches the contract every other tool in the mail stack uses
//! (mbsync, msmtp, practiceforge::email::pizauth). Cost is one
//! `fork+exec` per token fetch, which at ~10ms is rounding error
//! against a Gmail HTTP round-trip.

use anyhow::{Context, Result, anyhow};
use std::process::Command;

/// Fetch the current access token from pizauth. The pizauth daemon
/// will refresh transparently if the cached token is near expiry.
///
/// Returns the bare token (no `Bearer ` prefix) — caller adds that
/// via `reqwest::RequestBuilder::bearer_auth`.
pub fn access_token() -> Result<String> {
    let output = Command::new("pizauth")
        .args(["show", "gmail"])
        .output()
        .context("running `pizauth show gmail` (is pizauth installed and in PATH?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "pizauth show gmail failed (exit {:?}): {}",
            output.status.code(),
            stderr.trim()
        ));
    }

    let token = String::from_utf8(output.stdout)
        .context("pizauth output was not valid UTF-8")?
        .trim()
        .to_string();

    if token.is_empty() {
        return Err(anyhow!(
            "pizauth show gmail returned empty token — try `pizauth refresh gmail`"
        ));
    }

    Ok(token)
}
