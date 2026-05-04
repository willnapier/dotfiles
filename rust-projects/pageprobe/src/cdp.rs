//! Thin wrapper around `chromiumoxide` for the bits pageprobe needs.
//!
//! Subcommands open a fresh CDP WebSocket per invocation: `connect()` returns
//! a `(Browser, JoinHandle)` pair where the handler must be polled for the
//! lifetime of the connection. Drop the handle (or call `disconnect`) to tear
//! it down.
use anyhow::{Context, Result, anyhow};
use chromiumoxide::Browser;
use chromiumoxide::cdp::browser_protocol::target::{TargetId, TargetInfo};
use chromiumoxide::page::Page;
use futures::StreamExt;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::chrome;

/// Resolves the WebSocket debugger URL from `/json/version`.
pub async fn ws_url(port: u16) -> Result<String> {
    let v = chrome::fetch_version(port).await.with_context(|| {
        format!("could not reach Chrome on port {port}; is `pageprobe start` running?")
    })?;
    let url = v
        .get("webSocketDebuggerUrl")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("webSocketDebuggerUrl missing from /json/version"))?;
    Ok(url.to_string())
}

/// Connects to the running debug-Chrome on `port`. Returns the connected
/// `Browser` plus the background-event handler JoinHandle.
pub async fn connect(port: u16) -> Result<(Browser, JoinHandle<()>)> {
    let url = ws_url(port).await?;
    let (browser, mut handler) = Browser::connect(&url)
        .await
        .with_context(|| format!("CDP connect to {url}"))?;
    let handle = tokio::spawn(async move {
        // Drain the handler until disconnection. Errors are non-fatal here;
        // the per-command Browser will surface failures itself.
        while let Some(_event) = handler.next().await {}
    });
    Ok((browser, handle))
}

/// Lists all open tabs (target type "page") on the connected browser.
pub async fn list_pages(browser: &mut Browser) -> Result<Vec<TargetInfo>> {
    let targets = browser.fetch_targets().await?;
    let pages = targets
        .into_iter()
        .filter(|t| t.r#type == "page")
        .collect::<Vec<_>>();
    Ok(pages)
}

/// Resolves a `Page` by tab id, polling briefly so we don't lose to the
/// race between `Browser::connect` returning and the Handler receiving
/// `Target.targetCreated` events for pre-existing tabs.
///
/// The handler queues `Target.setDiscoverTargets(true)` on construction
/// but we have to give it a moment to hear back. This polls for up to
/// ~2 seconds at 50ms intervals.
pub async fn page_for_tab(browser: &Browser, tab_id: &str) -> Result<Page> {
    let target_id = TargetId::from(tab_id.to_string());
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(page) = browser.get_page(target_id.clone()).await {
            return Ok(page);
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "attached tab id no longer matches any open tab; run `pageprobe attach` again"
            ));
        }
        sleep(Duration::from_millis(50)).await;
    }
}

/// Truncates `s` to `max` chars, appending "..." if it was longer.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

/// Returns a short, human-typeable id (first 8 chars of the CDP target id).
pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Picks a single target whose id, url, or title matches `pattern`
/// (case-insensitive substring match). Errors if zero or multiple match
/// — except that an exact id (or short-id prefix) match wins outright.
pub fn pick_target<'a>(
    targets: &'a [TargetInfo],
    pattern: &str,
) -> Result<&'a TargetInfo> {
    // Exact / prefix match on target id wins.
    for t in targets {
        if t.target_id.as_ref() == pattern || t.target_id.as_ref().starts_with(pattern) {
            return Ok(t);
        }
    }
    let needle = pattern.to_lowercase();
    let matches: Vec<&TargetInfo> = targets
        .iter()
        .filter(|t| {
            t.url.to_lowercase().contains(&needle)
                || t.title.to_lowercase().contains(&needle)
        })
        .collect();
    match matches.len() {
        0 => Err(anyhow!(
            "no tab matched pattern '{pattern}' (try `pageprobe tabs` to list)"
        )),
        1 => Ok(matches[0]),
        n => {
            let mut hint = String::new();
            for t in matches.iter().take(5) {
                hint.push_str(&format!(
                    "\n  {}  {}",
                    short_id(t.target_id.as_ref()),
                    truncate(&t.url, 70)
                ));
            }
            Err(anyhow!(
                "{n} tabs matched pattern '{pattern}'; pass a target id to disambiguate.{hint}"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("abc", 10), "abc");
    }

    #[test]
    fn truncate_long_string_shortened() {
        assert_eq!(truncate("abcdefghijkl", 8), "abcde...");
    }

    #[test]
    fn short_id_takes_eight() {
        assert_eq!(short_id("0123456789abcdef"), "01234567");
    }

    #[test]
    fn short_id_handles_short_input() {
        assert_eq!(short_id("abc"), "abc");
    }
}
