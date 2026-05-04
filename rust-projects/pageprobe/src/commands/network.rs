//! `pageprobe network` — captures recent HTTP traffic on the attached tab.
//!
//! v0.1 limitation: CDP only emits Network.* events while a session is
//! attached. Because pageprobe doesn't run a daemon, we capture events
//! around a forced reload of the attached tab (`Page.reload`). For
//! click-driven measurement, click in the browser, then re-run this
//! command — but be aware events you see come from the post-reload state.
use anyhow::{Context, Result, anyhow};
use chromiumoxide::cdp::browser_protocol::network::{
    EnableParams, EventLoadingFinished, EventRequestWillBeSent, EventResponseReceived,
};
use chromiumoxide::cdp::browser_protocol::page::ReloadParams;
use chromiumoxide::cdp::browser_protocol::target::TargetId;
use futures::StreamExt;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::{Instant, sleep};

use crate::{cdp, state};

const QUIESCENT_MS: u64 = 1500;
const MAX_WAIT_MS: u64 = 4000;

#[derive(Default, Clone, Debug)]
struct Entry {
    method: String,
    url: String,
    mime: String,
    status: Option<i64>,
    request_ts: Option<f64>,
    response_ts: Option<f64>,
    finish_ts: Option<f64>,
    encoded_size: Option<f64>,
}

#[derive(Serialize)]
struct OutRow {
    method: String,
    url: String,
    status: Option<i64>,
    mime: String,
    total_ms: Option<u64>,
    ttfb_ms: Option<u64>,
    size_bytes: Option<u64>,
    request_id: String,
}

pub async fn run(last: usize, json: bool) -> Result<()> {
    let s = state::load()?;
    let port = s.port_or_default();
    let tab_id = s
        .attached_tab_id
        .clone()
        .ok_or_else(|| anyhow!("no tab attached. Run `pageprobe attach <pattern>` first."))?;

    let (mut browser, handle) = cdp::connect(port).await?;
    let target_id = TargetId::from(tab_id.clone());
    let page = browser
        .pages()
        .await?
        .into_iter()
        .find(|p| p.target_id().as_ref() == tab_id.as_str())
        .ok_or_else(|| {
            anyhow!("attached tab id no longer matches any open tab; run `pageprobe attach` again")
        })?;

    let _ = target_id; // Suppresses unused warning if API path ever shifts.

    page.execute(EnableParams::default())
        .await
        .context("Network.enable")?;

    // Subscribe BEFORE reloading so we don't miss events.
    let mut req_stream = page
        .event_listener::<EventRequestWillBeSent>()
        .await
        .context("subscribe Network.requestWillBeSent")?;
    let mut resp_stream = page
        .event_listener::<EventResponseReceived>()
        .await
        .context("subscribe Network.responseReceived")?;
    let mut fin_stream = page
        .event_listener::<EventLoadingFinished>()
        .await
        .context("subscribe Network.loadingFinished")?;

    let entries: Arc<Mutex<HashMap<String, Entry>>> = Arc::new(Mutex::new(HashMap::new()));
    let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let entries_req = entries.clone();
    let order_req = order.clone();
    let req_task = tokio::spawn(async move {
        while let Some(ev) = req_stream.next().await {
            let id = ev.request_id.as_ref().to_string();
            let mut map = entries_req.lock().await;
            let e = map.entry(id.clone()).or_default();
            e.method = ev.request.method.clone();
            e.url = ev.request.url.clone();
            e.request_ts = Some(*ev.timestamp.inner());
            drop(map);
            let mut ord = order_req.lock().await;
            if !ord.contains(&id) {
                ord.push(id);
            }
        }
    });

    let entries_resp = entries.clone();
    let resp_task = tokio::spawn(async move {
        while let Some(ev) = resp_stream.next().await {
            let id = ev.request_id.as_ref().to_string();
            let mut map = entries_resp.lock().await;
            let e = map.entry(id).or_default();
            e.status = Some(ev.response.status);
            e.mime = ev.response.mime_type.clone();
            e.response_ts = Some(*ev.timestamp.inner());
        }
    });

    let entries_fin = entries.clone();
    let fin_task = tokio::spawn(async move {
        while let Some(ev) = fin_stream.next().await {
            let id = ev.request_id.as_ref().to_string();
            let mut map = entries_fin.lock().await;
            let e = map.entry(id).or_default();
            e.finish_ts = Some(*ev.timestamp.inner());
            e.encoded_size = Some(ev.encoded_data_length);
        }
    });

    // Trigger reload so we have something to capture.
    page.execute(ReloadParams {
        ignore_cache: Some(true),
        script_to_evaluate_on_load: None,
        loader_id: None,
    })
    .await
    .context("Page.reload")?;

    // Wait for traffic to settle: stop after QUIESCENT_MS without new
    // events, or after MAX_WAIT_MS total.
    let start = Instant::now();
    let mut last_count = 0usize;
    let mut last_change = Instant::now();
    loop {
        sleep(Duration::from_millis(150)).await;
        let map = entries.lock().await;
        let count = map.len();
        drop(map);
        if count != last_count {
            last_count = count;
            last_change = Instant::now();
        }
        if last_change.elapsed() >= Duration::from_millis(QUIESCENT_MS) {
            break;
        }
        if start.elapsed() >= Duration::from_millis(MAX_WAIT_MS) {
            break;
        }
    }

    req_task.abort();
    resp_task.abort();
    fin_task.abort();

    let map = entries.lock().await;
    let ord = order.lock().await;

    let mut rows: Vec<OutRow> = Vec::new();
    for id in ord.iter() {
        if let Some(e) = map.get(id) {
            let total_ms = match (e.request_ts, e.finish_ts.or(e.response_ts)) {
                (Some(a), Some(b)) => Some(((b - a) * 1000.0).max(0.0) as u64),
                _ => None,
            };
            let ttfb_ms = match (e.request_ts, e.response_ts) {
                (Some(a), Some(b)) => Some(((b - a) * 1000.0).max(0.0) as u64),
                _ => None,
            };
            let size_bytes = e.encoded_size.map(|b| b.max(0.0) as u64);
            rows.push(OutRow {
                method: e.method.clone(),
                url: e.url.clone(),
                status: e.status,
                mime: e.mime.clone(),
                total_ms,
                ttfb_ms,
                size_bytes,
                request_id: id.clone(),
            });
        }
    }

    // Trim to last N (oldest-first preserved).
    if rows.len() > last {
        let drop_n = rows.len() - last;
        rows.drain(0..drop_n);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else if rows.is_empty() {
        println!("(no requests captured — page may have rendered from cache; try again)");
    } else {
        for r in &rows {
            let status = r.status.map(|s| s.to_string()).unwrap_or_else(|| "—".into());
            let mime = if r.mime.is_empty() { "?".to_string() } else { r.mime.clone() };
            let total = r
                .total_ms
                .map(|m| format!("{m}ms"))
                .unwrap_or_else(|| "—".into());
            let ttfb = r
                .ttfb_ms
                .map(|m| format!("TTFB {m}ms"))
                .unwrap_or_else(|| "TTFB —".into());
            let size = r
                .size_bytes
                .map(format_bytes)
                .unwrap_or_else(|| "—".into());
            let url = cdp::truncate(&r.url, 60);
            println!(
                "{:<6} {:<60}  {:>3}  {:<22}  {:>7} ({}, {})",
                r.method, url, status, mime, total, ttfb, size
            );
        }
    }

    let _ = browser.close().await;
    handle.abort();
    Ok(())
}

fn format_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if n >= MB {
        format!("{:.1}MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{}KB", n / KB)
    } else {
        format!("{n}B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_thresholds() {
        assert_eq!(format_bytes(0), "0B");
        assert_eq!(format_bytes(1023), "1023B");
        assert_eq!(format_bytes(1024), "1KB");
        assert_eq!(format_bytes(2 * 1024 * 1024), "2.0MB");
    }
}
