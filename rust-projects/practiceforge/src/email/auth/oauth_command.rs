//! Command-based OAuth token source.
//!
//! Runs an external command (e.g. `cohs-oauth show`) whose stdout is the
//! current access token. The command is responsible for refresh logic; this
//! source just captures whatever it prints.

use anyhow::{anyhow, Context, Result};
use std::process::Command;

use super::TokenSource;

/// Fetches an access token by running a command and reading its stdout.
///
/// The command must print the token as its only stdout output (trimmed).
/// Non-zero exit code or empty output is an error.
pub struct CommandTokenSource {
    /// Shell command to execute. Example: `"cohs-oauth show"`.
    /// Parsed via whitespace split (sufficient for `cohs-oauth <subcommand>`
    /// shape; no shell metacharacters or quoted args in real use).
    pub command: String,
}

impl CommandTokenSource {
    pub fn new(command: impl Into<String>) -> Self {
        Self { command: command.into() }
    }
}

impl TokenSource for CommandTokenSource {
    fn access_token(&self) -> Result<String> {
        // Whitespace split is sufficient for `cohs-oauth show` and other
        // simple argv forms we actually use. If a future command needs
        // quoted args, upgrade to `shlex::split` (not currently in deps).
        let mut parts = self.command.split_whitespace();
        let program = parts
            .next()
            .ok_or_else(|| anyhow!("empty command string in CommandTokenSource"))?;
        let args: Vec<&str> = parts.collect();

        let output = Command::new(program)
            .args(&args)
            .output()
            .with_context(|| format!("failed to spawn token command: {}", self.command))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "token command `{}` exited with status {}: {}",
                self.command,
                output.status,
                stderr.trim()
            ));
        }

        let stdout = String::from_utf8(output.stdout)
            .with_context(|| format!("token command `{}` produced non-UTF8 stdout", self.command))?;
        let token = stdout.trim();
        if token.is_empty() {
            return Err(anyhow!(
                "token command `{}` produced empty stdout",
                self.command
            ));
        }

        Ok(token.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_stdout_trimmed() {
        // printf doesn't add a trailing newline, but we also test newline trimming.
        let src = CommandTokenSource::new("printf testtoken");
        let token = src.access_token().expect("command should succeed");
        assert_eq!(token, "testtoken");
    }

    #[test]
    fn trims_surrounding_whitespace() {
        // `echo` appends a newline — confirm we strip it.
        let src = CommandTokenSource::new("echo  hello-token");
        let token = src.access_token().expect("command should succeed");
        assert_eq!(token, "hello-token");
    }

    #[test]
    fn nonzero_exit_is_error() {
        let src = CommandTokenSource::new("false");
        let err = src.access_token().expect_err("false should fail");
        let msg = format!("{err}");
        assert!(msg.contains("false"), "error should name the command: {msg}");
        assert!(
            msg.contains("status") || msg.contains("exited"),
            "error should mention exit status: {msg}"
        );
    }

    #[test]
    fn missing_binary_is_error() {
        let src = CommandTokenSource::new("definitely-not-a-real-binary-xyzzy-42");
        let err = src.access_token().expect_err("missing binary should fail");
        let msg = format!("{err}");
        assert!(
            msg.contains("spawn") || msg.contains("definitely-not-a-real-binary-xyzzy-42"),
            "error should be informative: {msg}"
        );
    }

    #[test]
    fn empty_stdout_is_error() {
        // `true` exits 0 with no output.
        let src = CommandTokenSource::new("true");
        let err = src.access_token().expect_err("empty stdout should fail");
        let msg = format!("{err}");
        assert!(msg.contains("empty"), "error should mention empty: {msg}");
    }

    #[test]
    fn empty_command_string_is_error() {
        let src = CommandTokenSource::new("   ");
        let err = src.access_token().expect_err("empty command should fail");
        let msg = format!("{err}");
        assert!(msg.contains("empty"), "error should mention empty: {msg}");
    }
}
