//! `pageprobe console` — captures recent console messages on the attached
//! tab. Same temporary-attach pattern as `pageprobe network`: we trigger
//! a reload and capture output for a few seconds. Click-driven console
//! noise won't be visible — re-run after triggering it in the browser.
use anyhow::{Context, Result, anyhow};
use chromiumoxide::cdp::browser_protocol::page::ReloadParams;
use chromiumoxide::cdp::js_protocol::runtime::{
    EnableParams as RuntimeEnableParams, EventConsoleApiCalled, EventExceptionThrown,
};
use futures::StreamExt;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::{Instant, sleep};

use crate::{cdp, state};

const QUIESCENT_MS: u64 = 1500;
const MAX_WAIT_MS: u64 = 4000;

#[derive(Serialize, Clone)]
struct ConsoleRow {
    timestamp: f64,
    level: String,
    message: String,
    source: Option<String>,
    line: Option<u32>,
}

pub async fn run(last: usize, json: bool) -> Result<()> {
    let s = state::load()?;
    let port = s.port_or_default();
    let tab_id = s
        .attached_tab_id
        .clone()
        .ok_or_else(|| anyhow!("no tab attached. Run `pageprobe attach <pattern>` first."))?;

    let (mut browser, handle) = cdp::connect(port).await?;
    let page = browser
        .pages()
        .await?
        .into_iter()
        .find(|p| p.target_id().as_ref() == tab_id.as_str())
        .ok_or_else(|| {
            anyhow!("attached tab id no longer matches any open tab; run `pageprobe attach` again")
        })?;

    page.execute(RuntimeEnableParams::default())
        .await
        .context("Runtime.enable")?;

    let mut console_stream = page
        .event_listener::<EventConsoleApiCalled>()
        .await
        .context("subscribe Runtime.consoleAPICalled")?;
    let mut exc_stream = page
        .event_listener::<EventExceptionThrown>()
        .await
        .context("subscribe Runtime.exceptionThrown")?;

    let rows: Arc<Mutex<Vec<ConsoleRow>>> = Arc::new(Mutex::new(Vec::new()));

    let rows_console = rows.clone();
    let console_task = tokio::spawn(async move {
        while let Some(ev) = console_stream.next().await {
            let level = format!("{:?}", ev.r#type).to_lowercase();
            let message = ev
                .args
                .iter()
                .map(|a| {
                    if let Some(v) = a.value.as_ref() {
                        v.to_string().trim_matches('"').to_string()
                    } else if let Some(d) = a.description.as_ref() {
                        d.clone()
                    } else {
                        format!("[{:?}]", a.r#type)
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            let (source, line) = ev
                .stack_trace
                .as_ref()
                .and_then(|st| st.call_frames.first())
                .map(|f| (Some(f.url.clone()), Some(f.line_number as u32)))
                .unwrap_or((None, None));
            let mut guard = rows_console.lock().await;
            guard.push(ConsoleRow {
                timestamp: *ev.timestamp.inner(),
                level,
                message,
                source,
                line,
            });
        }
    });

    let rows_exc = rows.clone();
    let exc_task = tokio::spawn(async move {
        while let Some(ev) = exc_stream.next().await {
            let message = ev
                .exception_details
                .exception
                .as_ref()
                .and_then(|e| e.description.clone())
                .unwrap_or_else(|| ev.exception_details.text.clone());
            let source = ev.exception_details.url.clone();
            let line = Some(ev.exception_details.line_number as u32);
            let mut guard = rows_exc.lock().await;
            guard.push(ConsoleRow {
                timestamp: *ev.timestamp.inner(),
                level: "exception".into(),
                message,
                source,
                line,
            });
        }
    });

    page.execute(ReloadParams {
        ignore_cache: Some(true),
        script_to_evaluate_on_load: None,
        loader_id: None,
    })
    .await
    .context("Page.reload")?;

    let start = Instant::now();
    let mut last_count = 0usize;
    let mut last_change = Instant::now();
    loop {
        sleep(Duration::from_millis(150)).await;
        let count = rows.lock().await.len();
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

    console_task.abort();
    exc_task.abort();

    let mut out = rows.lock().await.clone();
    out.sort_by(|a, b| a.timestamp.partial_cmp(&b.timestamp).unwrap_or(std::cmp::Ordering::Equal));
    if out.len() > last {
        let drop_n = out.len() - last;
        out.drain(0..drop_n);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else if out.is_empty() {
        println!("(no console output captured)");
    } else {
        for r in &out {
            let where_ = match (&r.source, r.line) {
                (Some(s), Some(l)) => format!("{}:{}", cdp::truncate(s, 50), l),
                (Some(s), None) => cdp::truncate(s, 50),
                _ => "—".into(),
            };
            println!(
                "{:>14.3}  {:<10}  {:<50}  {}",
                r.timestamp, r.level, where_, r.message
            );
        }
    }

    // Drop closes the WebSocket; we deliberately do NOT call
    // `browser.close()` (that sends `Browser.close` and shuts Chrome down).
    handle.abort();
    drop(browser);
    Ok(())
}
