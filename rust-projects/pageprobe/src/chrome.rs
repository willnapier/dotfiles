//! Helpers for launching and stopping the debug-Chrome process.
//!
//! On macOS we rely on `open -na "Google Chrome"` so that Chrome's normal
//! singleton-bundle launching is bypassed and a fresh process is spawned
//! against our dedicated user-data-dir. On Linux/other platforms we shell
//! out to `google-chrome` (best-effort fallback — pageprobe is primarily a
//! macOS tool today).
use anyhow::{Context, Result, bail};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::time::sleep;

const VERSION_TIMEOUT_SECS: u64 = 15;

/// Returns true if a Chrome debug-endpoint is reachable on `port`.
pub async fn debug_chrome_alive(port: u16) -> bool {
    fetch_version(port).await.is_ok()
}

/// Hits `http://127.0.0.1:<port>/json/version`. Returns the parsed JSON
/// blob on success.
pub async fn fetch_version(port: u16) -> Result<serde_json::Value> {
    let url = format!("http://127.0.0.1:{port}/json/version");
    let resp = reqwest::Client::new()
        .get(&url)
        .timeout(Duration::from_millis(800))
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;
    Ok(resp)
}

/// Launches Chrome with the debug-port + dedicated user-data-dir, then
/// polls `/json/version` until Chrome is ready (or we time out).
///
/// Returns the PID of the launched Chrome process.
pub async fn launch(port: u16, user_data_dir: &Path) -> Result<u32> {
    if debug_chrome_alive(port).await {
        bail!("a debug-Chrome is already responding on port {port}");
    }

    std::fs::create_dir_all(user_data_dir).with_context(|| {
        format!("creating user-data-dir {}", user_data_dir.display())
    })?;

    let pid = launch_platform(port, user_data_dir).await?;

    // Poll /json/version until the debug endpoint is ready.
    let deadline = Instant::now() + Duration::from_secs(VERSION_TIMEOUT_SECS);
    loop {
        if debug_chrome_alive(port).await {
            return Ok(pid);
        }
        if Instant::now() >= deadline {
            bail!(
                "Chrome did not respond on port {port} within {VERSION_TIMEOUT_SECS}s. \
                 Try running `pageprobe stop` then retrying."
            );
        }
        sleep(Duration::from_millis(250)).await;
    }
}

#[cfg(target_os = "macos")]
async fn launch_platform(port: u16, user_data_dir: &Path) -> Result<u32> {
    // `open -na "Google Chrome" --args ...` returns immediately and the
    // child PID is *its* PID, not Chrome's. We spawn `open` so we can wait
    // briefly, then resolve the actual Chrome PID via pgrep on the
    // user-data-dir flag (unique per launch).
    let user_data_arg = format!("--user-data-dir={}", user_data_dir.display());
    let port_arg = format!("--remote-debugging-port={port}");

    let status = Command::new("open")
        .args([
            "-na",
            "Google Chrome",
            "--args",
            &port_arg,
            &user_data_arg,
            "--no-first-run",
            "--no-default-browser-check",
        ])
        .status()
        .await
        .context("running `open -na 'Google Chrome'`")?;
    if !status.success() {
        bail!("`open -na 'Google Chrome'` exited {status}");
    }

    // Chrome takes a moment to register its process. Poll `pgrep -f` for
    // up to a few seconds looking for our user-data-dir argument.
    let needle = format!("--user-data-dir={}", user_data_dir.display());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(pid) = pgrep_first(&needle).await? {
            return Ok(pid);
        }
        if Instant::now() >= deadline {
            // Couldn't find the PID — not fatal, but state will lack it.
            // Return 0 to signal "unknown PID"; `stop` will fall back to
            // killing by pgrep.
            return Ok(0);
        }
        sleep(Duration::from_millis(150)).await;
    }
}

#[cfg(not(target_os = "macos"))]
async fn launch_platform(port: u16, user_data_dir: &Path) -> Result<u32> {
    let user_data_arg = format!("--user-data-dir={}", user_data_dir.display());
    let port_arg = format!("--remote-debugging-port={port}");
    let child = Command::new("google-chrome")
        .args([
            &port_arg,
            &user_data_arg,
            "--no-first-run",
            "--no-default-browser-check",
        ])
        .spawn()
        .context("spawning `google-chrome`")?;
    Ok(child.id().unwrap_or(0))
}


/// `pgrep -f <needle>` — returns the first matching PID if any.
async fn pgrep_first(needle: &str) -> Result<Option<u32>> {
    let out = Command::new("pgrep")
        .args(["-f", needle])
        .output()
        .await
        .context("invoking pgrep")?;
    if !out.status.success() {
        return Ok(None);
    }
    let s = String::from_utf8_lossy(&out.stdout);
    for line in s.lines() {
        if let Ok(pid) = line.trim().parse::<u32>() {
            return Ok(Some(pid));
        }
    }
    Ok(None)
}

/// Sends SIGTERM to `pid`. Falls back to SIGKILL after `grace`.
pub async fn stop(pid: u32, grace: Duration) -> Result<()> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    let nix_pid = Pid::from_raw(pid as i32);

    // SIGTERM
    let _ = kill(nix_pid, Signal::SIGTERM);

    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        // kill(pid, 0) returns Err if process is gone.
        if kill(nix_pid, None).is_err() {
            return Ok(());
        }
        sleep(Duration::from_millis(150)).await;
    }
    // Still alive — SIGKILL.
    let _ = kill(nix_pid, Signal::SIGKILL);
    Ok(())
}

/// Best-effort: kill any debug-Chrome whose command line contains the
/// given user-data-dir. Used when `state.json` lacks a PID.
pub async fn stop_by_user_data_dir(user_data_dir: &Path) -> Result<usize> {
    let needle = format!("--user-data-dir={}", user_data_dir.display());
    let out = Command::new("pgrep")
        .args(["-f", &needle])
        .output()
        .await
        .context("invoking pgrep")?;
    if !out.status.success() {
        return Ok(0);
    }
    let s = String::from_utf8_lossy(&out.stdout).to_string();
    let mut count = 0usize;
    for line in s.lines() {
        if let Ok(pid) = line.trim().parse::<u32>() {
            stop(pid, Duration::from_secs(3)).await?;
            count += 1;
        }
    }
    Ok(count)
}
