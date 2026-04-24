// Claude CLI wrapper — used by the eval/label/improve subcommands.
//
// Shells out to `claude -p` (Claude Code's non-interactive mode). Matches
// the pattern practiceforge uses: the user has already authenticated via
// `claude auth login`, so no API key is needed in config. Outputs are
// returned as plain text for the caller to parse.
//
// The prompts are constructed by eval.rs; this module only handles the
// subprocess invocation.

use anyhow::{Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};

/// Send a prompt to Claude via `claude -p` and return the response text.
/// The prompt is passed on stdin to avoid argv-length limits.
pub fn ask(prompt: &str) -> Result<String> {
    let mut child = Command::new("claude")
        .arg("-p")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning `claude -p` — is Claude Code installed and authenticated?")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes())
            .context("writing prompt to claude stdin")?;
        // Drop stdin so claude sees EOF and starts processing.
    }

    let output = child.wait_with_output().context("waiting for claude to finish")?;
    if !output.status.success() {
        anyhow::bail!(
            "claude exited with status {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Check that the claude CLI is available and authenticated. Cheap smoke test.
pub fn probe() -> Result<()> {
    let output = Command::new("claude")
        .arg("--version")
        .output()
        .context("spawning `claude --version`")?;
    if !output.status.success() {
        anyhow::bail!(
            "claude --version failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}
