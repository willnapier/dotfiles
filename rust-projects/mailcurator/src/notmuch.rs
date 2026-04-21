// notmuch subprocess wrapper — thin layer over the `notmuch` CLI.
//
// We shell out rather than use bindings because:
//   - no extra build-time dependency on libnotmuch dev headers
//   - notmuch CLI is stable and widely available
//   - our call pattern is batch-oriented, so subprocess overhead is negligible

use anyhow::{Context, Result};
use std::process::Command;

/// Count messages matching a notmuch query.
pub fn count(query: &str) -> Result<u64> {
    let output = Command::new("notmuch")
        .args(["count", query])
        .output()
        .with_context(|| format!("spawning `notmuch count {query}`"))?;
    if !output.status.success() {
        anyhow::bail!(
            "notmuch count failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let s = String::from_utf8_lossy(&output.stdout);
    let n = s.trim().parse::<u64>()
        .with_context(|| format!("parsing notmuch count output: {s:?}"))?;
    Ok(n)
}

/// Apply a tag change to all messages matching the query.
/// `changes` is a slice of "+tag" or "-tag" strings.
/// Returns unit; notmuch itself is silent on success.
pub fn tag(query: &str, changes: &[&str]) -> Result<()> {
    let mut cmd = Command::new("notmuch");
    cmd.arg("tag");
    for ch in changes {
        cmd.arg(ch);
    }
    cmd.arg("--");
    cmd.arg(query);
    let status = cmd
        .status()
        .with_context(|| format!("spawning `notmuch tag {changes:?} -- {query}`"))?;
    if !status.success() {
        anyhow::bail!("notmuch tag failed (exit {:?})", status.code());
    }
    Ok(())
}

/// Convenience: apply + and - tags from separate slices.
pub fn apply_tag_changes(query: &str, add: &[&str], remove: &[&str]) -> Result<()> {
    let mut changes: Vec<String> = Vec::with_capacity(add.len() + remove.len());
    for t in add {
        changes.push(format!("+{t}"));
    }
    for t in remove {
        changes.push(format!("-{t}"));
    }
    let refs: Vec<&str> = changes.iter().map(|s| s.as_str()).collect();
    tag(query, &refs)
}
