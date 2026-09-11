//! `pageprobe shot` — one-shot headless screenshot in a throwaway Chrome that
//! is always torn down.
//!
//! This is the sanctioned replacement for a hand-rolled
//! `Google Chrome --headless=new --screenshot=… --user-data-dir=$(mktemp -d)`.
//! That recipe can leave Chrome running after the PNG is written (seen
//! 2026-09-11: the zombie survived a Chrome auto-update, duplicated the app
//! switcher entry and swallowed every URL sent via `open`). Here:
//!
//! - Chrome is *our child* (chromiumoxide spawns it with `kill_on_drop`), so
//!   it dies with us on every normal and error path;
//! - the whole run sits under a hard `--timeout`; on expiry Chrome is killed;
//! - teardown is explicit — `Browser.close` → wait → SIGKILL fallback — and
//!   the temp profile is removed afterwards;
//! - background networking and component updates are off, so the profile
//!   cannot grow while we wait.
//!
//! If pageprobe itself is SIGKILLed the child can still be orphaned; that is
//! what `pageprobe reap` (and the daily health check) are for.
use anyhow::{Context, Result, anyhow, bail};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::handler::viewport::Viewport;
use chromiumoxide::page::ScreenshotParams;
use futures::StreamExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::time::{sleep, timeout};

use super::screenshot::resolve_output_path;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    target: String,
    out: Option<PathBuf>,
    width: u32,
    height: u32,
    full: bool,
    wait_ms: u64,
    timeout_secs: u64,
    quality: Option<i64>,
) -> Result<()> {
    if let Some(q) = quality
        && !(0..=100).contains(&q)
    {
        bail!("--quality must be 0-100, got {q}");
    }
    if timeout_secs == 0 {
        bail!("--timeout must be at least 1 second");
    }
    let url = normalise_target(&target)?;
    let out_path = resolve_output_path(out, quality.is_some());

    let profile = tempfile::Builder::new()
        .prefix("pageprobe-shot-")
        .tempdir()
        .context("creating temp profile dir")?;

    let config = BrowserConfig::builder()
        .new_headless_mode()
        .user_data_dir(profile.path())
        .window_size(width, height)
        // chromiumoxide emulates an 800x600 viewport unless told otherwise;
        // make the capture match the requested window.
        .viewport(Viewport {
            width,
            height,
            device_scale_factor: None,
            emulating_mobile: false,
            is_landscape: false,
            has_touch: false,
        })
        // 0 = let Chrome pick a free port; never collide with `pageprobe start`.
        .port(0)
        .args([
            "--disable-component-update",
            "--no-default-browser-check",
            "--noerrdialogs",
        ])
        .launch_timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| anyhow!("browser config: {e}"))?;

    let (mut browser, mut handler) = Browser::launch(config)
        .await
        .context("launching headless Chrome")?;
    let handle = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let outcome = timeout(
        Duration::from_secs(timeout_secs),
        capture(&browser, &url, &out_path, full, wait_ms, quality),
    )
    .await;

    // Teardown runs on every path, including timeout. Each step is itself
    // bounded so a wedged Chrome cannot hold us hostage.
    let _ = timeout(Duration::from_secs(5), browser.close()).await;
    let exited = timeout(Duration::from_secs(5), browser.wait()).await;
    if exited.is_err() {
        eprintln!("(Chrome ignored close; sending SIGKILL)");
        let _ = browser.kill().await;
    }
    handle.abort();
    drop(browser);
    if let Err(e) = profile.close() {
        eprintln!("could not remove temp profile: {e}");
    }

    match outcome {
        Ok(Ok(bytes)) => {
            println!("{}  ({} bytes)", out_path.display(), bytes);
            Ok(())
        }
        Ok(Err(e)) => Err(e),
        Err(_) => bail!(
            "timed out after {timeout_secs}s loading {url}; Chrome was killed, nothing written"
        ),
    }
}

async fn capture(
    browser: &Browser,
    url: &str,
    out_path: &Path,
    full: bool,
    wait_ms: u64,
    quality: Option<i64>,
) -> Result<usize> {
    let page = browser
        .new_page(url)
        .await
        .with_context(|| format!("opening {url}"))?;
    page.wait_for_navigation()
        .await
        .with_context(|| format!("loading {url}"))?;
    if wait_ms > 0 {
        sleep(Duration::from_millis(wait_ms)).await;
    }

    let mut builder = ScreenshotParams::builder();
    if let Some(q) = quality {
        builder = builder.format(CaptureScreenshotFormat::Jpeg).quality(q);
    } else {
        builder = builder.format(CaptureScreenshotFormat::Png);
    }
    if full {
        builder = builder.full_page(true).capture_beyond_viewport(true);
    }
    let bytes = page
        .save_screenshot(builder.build(), out_path)
        .await
        .context("Page.captureScreenshot")?;
    Ok(bytes.len())
}

/// Accepts a URL as-is, or turns a local path (absolute or relative) into a
/// `file://` URL. A bare path that does not exist is an error, not a
/// silent `about:blank` screenshot.
pub fn normalise_target(target: &str) -> Result<String> {
    let lower = target.to_ascii_lowercase();
    if ["http://", "https://", "file://", "about:", "data:"]
        .iter()
        .any(|p| lower.starts_with(p))
    {
        return Ok(target.to_string());
    }
    let path = Path::new(target);
    let abs = std::fs::canonicalize(path)
        .with_context(|| format!("{target}: not a URL and not an existing file"))?;
    url::Url::from_file_path(&abs)
        .map(|u| u.to_string())
        .map_err(|_| anyhow!("{}: cannot express as file:// URL", abs.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_pass_through_untouched() {
        for u in ["http://127.0.0.1:3457/", "https://x.y/z?q=1", "file:///tmp/a.html", "about:blank"] {
            assert_eq!(normalise_target(u).unwrap(), u);
        }
        assert_eq!(normalise_target("HTTP://X").unwrap(), "HTTP://X");
    }

    #[test]
    fn existing_path_becomes_file_url() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("page.html");
        std::fs::write(&f, "<p>hi</p>").unwrap();
        let u = normalise_target(f.to_str().unwrap()).unwrap();
        assert!(u.starts_with("file:///"), "{u}");
        assert!(u.ends_with("/page.html"), "{u}");
    }

    #[test]
    fn missing_path_is_an_error() {
        let e = normalise_target("/definitely/not/here.html").unwrap_err().to_string();
        assert!(e.contains("not a URL and not an existing file"), "{e}");
    }
}
