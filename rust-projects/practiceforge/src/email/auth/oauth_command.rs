//! Command-based OAuth token source.
//!
//! Runs an external command (e.g. `cohs-oauth show`) whose stdout is the
//! current access token. The command is responsible for refresh logic; this
//! source just captures whatever it prints.
//!
//! Phase 0 stub. Phase 2 will implement.

use anyhow::Result;

use super::TokenSource;

/// Fetches an access token by running a command and reading its stdout.
///
/// The command must print the token as its only stdout output (trimmed).
/// Non-zero exit code or empty output is an error.
pub struct CommandTokenSource {
    /// Shell command to execute. Example: `"cohs-oauth show"`.
    /// Parsed as a command + args (POSIX-style).
    pub command: String,
}

impl CommandTokenSource {
    pub fn new(command: impl Into<String>) -> Self {
        Self { command: command.into() }
    }
}

impl TokenSource for CommandTokenSource {
    fn access_token(&self) -> Result<String> {
        // Phase 2: implement subprocess run + stdout capture.
        todo!("Phase 2: run command, capture stdout, return trimmed token")
    }
}
