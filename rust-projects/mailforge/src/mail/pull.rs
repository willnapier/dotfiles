//! Bridge to the standalone `gmpull` CLI for on-demand pulls.
//!
//! Exposes a single endpoint `POST /api/pull-now` that runs
//!     `gmpull pull --resume && notmuch new`
//! synchronously and returns the parsed result as JSON. The client's
//! Ctrl+R refresh handler awaits this so the user sees the pulled
//! mail on the next page render rather than waiting up to 5 minutes
//! for the next launchd-scheduled tick.
//!
//! ## Why not `launchctl kickstart`?
//!
//! `launchctl kickstart -k gui/<uid>/com.williamnapier.gmpull` would
//! also trigger an immediate run. Two reasons we shell out directly
//! instead:
//!
//! 1. We get the exit status + stderr in-band; launchctl returns
//!    immediately with the spawned PID and no completion signal.
//! 2. We need to chain `notmuch new` after the pull so the listing
//!    re-render sees the new mail. The launchd plist already does
//!    `gmpull pull && notmuch new` as one shell command — we mirror
//!    that here.
//!
//! ## Why this is safe to spawn while a launchd-scheduled gmpull is
//! mid-run
//!
//! gmpull's filesystem dedup means concurrent writes to `~/Mail/gmail-rs/`
//! are idempotent (Maildir's tmp→cur rename pattern + filename-based
//! dedup at the start of pull). Two concurrent runs are wasteful but
//! not destructive. v0.3 might add a flock-based mutex; v0.2 leaves
//! the race-resolution at the filesystem layer.

use axum::response::Json;
use serde::Serialize;
use std::process::Command;
use std::time::Instant;

#[derive(Serialize)]
pub struct PullResult {
    pub ok: bool,
    pub took_ms: u64,
    /// Total bytes of stdout (truncated to 4kB to avoid bloat in toasts).
    pub stdout: String,
    pub error: Option<String>,
}

pub async fn pull_now_post() -> Json<PullResult> {
    let started = Instant::now();
    // Run the same shell command the launchd plist uses, so the
    // post-pull notmuch reindex matches what scheduled ticks do.
    // PATH is set so `gmpull` and `notmuch` resolve when invoked from
    // the mailforge daemon's shell context.
    let out = Command::new("/bin/sh")
        .arg("-c")
        .arg("/Users/williamnapier/.local/bin/gmpull pull --resume && /opt/homebrew/bin/notmuch new")
        .env("PATH", "/opt/homebrew/bin:/Users/williamnapier/.local/bin:/usr/local/bin:/usr/bin:/bin")
        .env("HOME", "/Users/williamnapier")
        .env("NOTMUCH_CONFIG", "/Users/williamnapier/Mail/.notmuch-config")
        .output();

    let elapsed_ms = started.elapsed().as_millis() as u64;

    match out {
        Ok(o) if o.status.success() => {
            let stdout = truncate(&String::from_utf8_lossy(&o.stdout), 4096);
            Json(PullResult {
                ok: true,
                took_ms: elapsed_ms,
                stdout,
                error: None,
            })
        }
        Ok(o) => {
            let stderr = truncate(&String::from_utf8_lossy(&o.stderr), 4096);
            let stdout = truncate(&String::from_utf8_lossy(&o.stdout), 4096);
            Json(PullResult {
                ok: false,
                took_ms: elapsed_ms,
                stdout,
                error: Some(stderr),
            })
        }
        Err(e) => Json(PullResult {
            ok: false,
            took_ms: elapsed_ms,
            stdout: String::new(),
            error: Some(format!("spawn failed: {e}")),
        }),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut t = s[..max].to_string();
        t.push_str("…[truncated]");
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_passes_through() {
        assert_eq!(truncate("hello", 100), "hello");
    }

    #[test]
    fn truncate_long_appends_marker() {
        let s = "x".repeat(5000);
        let t = truncate(&s, 4096);
        assert!(t.len() > 4096);
        assert!(t.ends_with("…[truncated]"));
    }
}
