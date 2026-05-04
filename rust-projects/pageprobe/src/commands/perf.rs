//! `pageprobe perf` — `Performance.getMetrics` plus paint-timing values
//! pulled via `performance.getEntriesByType`.
//!
//! CDP's `Performance.getMetrics` returns counters and durations (TaskDuration,
//! ScriptDuration, JSHeapUsedSize, NodeCount, etc.). For navigation timing
//! and paint events (FCP, LCP, DOMContentLoaded, load) we evaluate the
//! standard browser performance APIs in the page — they're more reliable
//! than the CDP-internal navigation history for the values reporters
//! actually want.
use anyhow::{Context, Result, anyhow};
use chromiumoxide::cdp::browser_protocol::performance::{
    EnableParams as PerfEnableParams, GetMetricsParams,
};
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use serde::{Deserialize, Serialize};

use crate::{cdp, state};

/// Categorisation of metric units for human-readable rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Ms,
    Bytes,
    Count,
}

impl Unit {
    /// Returns the unit appropriate for a CDP `Performance.getMetrics`
    /// metric name. Names ending in `Duration` are seconds (we convert
    /// to ms); names containing `Size` are bytes; everything else is a
    /// count (or a timestamp — see `is_epoch`).
    pub fn classify(name: &str) -> Unit {
        if name.ends_with("Duration") {
            Unit::Ms
        } else if name.contains("Size") {
            Unit::Bytes
        } else {
            Unit::Count
        }
    }
}

#[derive(Serialize, Debug)]
struct PerfRow {
    name: String,
    unit: String,
    value: f64,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
struct PaintTiming {
    dom_content_loaded_ms: Option<f64>,
    load_ms: Option<f64>,
    first_contentful_paint_ms: Option<f64>,
    largest_contentful_paint_ms: Option<f64>,
}

pub async fn run(json: bool) -> Result<()> {
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

    page.execute(PerfEnableParams::default())
        .await
        .context("Performance.enable")?;

    let metrics_resp = page
        .execute(GetMetricsParams::default())
        .await
        .context("Performance.getMetrics")?;
    let metrics = &metrics_resp.result.metrics;

    // Pull paint timing via the browser's performance API. This is wrapped
    // in a small JS expression that returns a serializable object.
    let paint_expr = r#"
        JSON.stringify((() => {
            const out = {};
            const nav = performance.getEntriesByType('navigation')[0];
            if (nav) {
                if (nav.domContentLoadedEventEnd > 0) {
                    out.domContentLoadedMs = nav.domContentLoadedEventEnd - nav.startTime;
                }
                if (nav.loadEventEnd > 0) {
                    out.loadMs = nav.loadEventEnd - nav.startTime;
                }
            }
            const fcp = performance.getEntriesByName('first-contentful-paint')[0];
            if (fcp) out.firstContentfulPaintMs = fcp.startTime;
            try {
                const lcps = performance.getEntriesByType('largest-contentful-paint');
                if (lcps && lcps.length > 0) {
                    out.largestContentfulPaintMs = lcps[lcps.length - 1].renderTime
                        || lcps[lcps.length - 1].loadTime
                        || lcps[lcps.length - 1].startTime;
                }
            } catch (e) { /* LCP not supported in this context */ }
            return out;
        })());
    "#;

    let eval_params = EvaluateParams {
        expression: paint_expr.to_string(),
        object_group: None,
        include_command_line_api: None,
        silent: Some(true),
        context_id: None,
        return_by_value: Some(true),
        generate_preview: None,
        user_gesture: None,
        await_promise: Some(false),
        throw_on_side_effect: None,
        timeout: None,
        disable_breaks: None,
        repl_mode: None,
        allow_unsafe_eval_blocked_by_csp: None,
        unique_context_id: None,
        serialization_options: None,
        eval_as_function_fallback: None,
    };
    let paint_resp = page
        .execute(eval_params)
        .await
        .context("Runtime.evaluate (paint timing)")?;
    let paint_timing: PaintTiming = paint_resp
        .result
        .result
        .value
        .as_ref()
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    // Drop closes the WebSocket; we deliberately do NOT call
    // `browser.close()` (that would shut Chrome down).
    handle.abort();
    drop(browser);

    let rows: Vec<PerfRow> = metrics
        .iter()
        .map(|m| {
            let unit = Unit::classify(&m.name);
            // CDP returns Duration metrics as seconds; convert to ms.
            let value = if unit == Unit::Ms { m.value * 1000.0 } else { m.value };
            PerfRow {
                name: m.name.clone(),
                unit: unit_label(unit).into(),
                value,
            }
        })
        .collect();

    if json {
        let payload = serde_json::json!({
            "metrics": rows,
            "timing": paint_timing,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("(no metrics returned)");
    } else {
        for r in &rows {
            let formatted = format_value(r.value, &r.unit);
            println!("{:<32} {:>20}", r.name, formatted);
        }
    }

    let mut printed_separator = false;
    let mut print_paint = |label: &str, val: Option<f64>| {
        if let Some(v) = val {
            if !printed_separator {
                println!();
                printed_separator = true;
            }
            println!("{:<32} {:>20}", label, format!("{:.0}ms", v));
        }
    };
    print_paint("DOMContentLoaded", paint_timing.dom_content_loaded_ms);
    print_paint("load", paint_timing.load_ms);
    print_paint("firstContentfulPaint", paint_timing.first_contentful_paint_ms);
    print_paint("largestContentfulPaint", paint_timing.largest_contentful_paint_ms);

    Ok(())
}

fn unit_label(u: Unit) -> &'static str {
    match u {
        Unit::Ms => "ms",
        Unit::Bytes => "bytes",
        Unit::Count => "count",
    }
}

fn format_value(value: f64, unit: &str) -> String {
    match unit {
        "ms" => format!("{:.0}ms", value),
        "bytes" => format_bytes(value as u64),
        "count" => format_count(value),
        other => format!("{value} {other}"),
    }
}

fn format_count(value: f64) -> String {
    let n = value.round() as i64;
    // Group thousands with commas.
    let mut s = n.abs().to_string();
    let bytes = s.as_bytes().to_vec();
    s.clear();
    for (i, &c) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            s.push(',');
        }
        s.push(c as char);
    }
    if n < 0 {
        format!("-{s}")
    } else {
        s
    }
}

fn format_bytes(n: u64) -> String {
    let mut s = n.to_string();
    let bytes = s.as_bytes().to_vec();
    s.clear();
    for (i, &c) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            s.push(',');
        }
        s.push(c as char);
    }
    format!("{s} bytes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_duration() {
        assert_eq!(Unit::classify("TaskDuration"), Unit::Ms);
        assert_eq!(Unit::classify("ScriptDuration"), Unit::Ms);
        assert_eq!(Unit::classify("LayoutDuration"), Unit::Ms);
    }

    #[test]
    fn classify_size() {
        assert_eq!(Unit::classify("JSHeapUsedSize"), Unit::Bytes);
        assert_eq!(Unit::classify("JSHeapTotalSize"), Unit::Bytes);
    }

    #[test]
    fn classify_count() {
        assert_eq!(Unit::classify("DocumentCount"), Unit::Count);
        assert_eq!(Unit::classify("NodeCount"), Unit::Count);
        assert_eq!(Unit::classify("Timestamp"), Unit::Count);
    }

    #[test]
    fn format_count_groups_thousands() {
        assert_eq!(format_count(0.0), "0");
        assert_eq!(format_count(123.0), "123");
        assert_eq!(format_count(1234.0), "1,234");
        assert_eq!(format_count(1234567.0), "1,234,567");
    }

    #[test]
    fn format_bytes_groups_thousands() {
        assert_eq!(format_bytes(0), "0 bytes");
        assert_eq!(format_bytes(1024), "1,024 bytes");
        assert_eq!(format_bytes(28_341_888), "28,341,888 bytes");
    }

    #[test]
    fn format_value_dispatch() {
        assert_eq!(format_value(142.4, "ms"), "142ms");
        assert_eq!(format_value(28_341_888.0, "bytes"), "28,341,888 bytes");
        assert_eq!(format_value(1234.0, "count"), "1,234");
    }
}
