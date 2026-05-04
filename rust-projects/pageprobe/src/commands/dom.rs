//! `pageprobe dom <selector>` — query DOM via CSS selector and report.
//!
//! Default `--html` returns the outer HTML of the first match. `--text`
//! returns just `textContent`. `--attrs` returns the element's attributes
//! as a JSON object. With `--all`, returns an array of every match.
//!
//! Implemented via `Runtime.evaluate` rather than the full CDP DOM domain
//! — simpler and avoids cross-domain id juggling. The JS expression is
//! wrapped in JSON.stringify so we get a value back that's trivial to
//! parse on the Rust side.
use anyhow::{Context, Result, anyhow};
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;

use crate::{cdp, state};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Html,
    Text,
    Attrs,
}

impl Mode {
    /// Parses the mode flags `--html`, `--text`, `--attrs`. Exactly one
    /// must be set; if none, defaults to `Html`. If multiple, errors.
    pub fn from_flags(html: bool, text: bool, attrs: bool) -> Result<Self> {
        let count = [html, text, attrs].iter().filter(|b| **b).count();
        if count > 1 {
            return Err(anyhow!(
                "pass at most one of --html, --text, --attrs (default is --html)"
            ));
        }
        if text {
            Ok(Mode::Text)
        } else if attrs {
            Ok(Mode::Attrs)
        } else {
            Ok(Mode::Html)
        }
    }
}

pub async fn run(
    selector: String,
    html: bool,
    text: bool,
    attrs: bool,
    all: bool,
) -> Result<()> {
    let mode = Mode::from_flags(html, text, attrs)?;
    let expr = build_expression(&selector, mode, all);

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

    let params = EvaluateParams {
        expression: expr,
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

    let resp = page.execute(params).await.context("Runtime.evaluate")?;

    // Drop closes the WebSocket; we deliberately do NOT call
    // `browser.close()` (that would shut Chrome down).
    handle.abort();
    drop(browser);

    if let Some(exc) = &resp.result.exception_details {
        return Err(anyhow!(
            "selector evaluation failed: {}",
            exc.exception
                .as_ref()
                .and_then(|e| e.description.clone())
                .unwrap_or_else(|| exc.text.clone())
        ));
    }

    // Result is a JSON-stringified value. Decode and pretty-print.
    let raw = resp
        .result
        .result
        .value
        .as_ref()
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Runtime.evaluate returned no string value"))?;

    let parsed: serde_json::Value = serde_json::from_str(raw)
        .with_context(|| format!("parsing eval result: {raw}"))?;

    if parsed.is_null() {
        if all {
            // empty array, not null — render as []
            println!("[]");
        } else {
            return Err(anyhow!("no element matched selector {selector:?}"));
        }
        return Ok(());
    }

    print_result(&parsed, mode, all);
    Ok(())
}

/// Builds the JS expression that performs the DOM query and serialises the
/// chosen output back as a string via `JSON.stringify`.
pub fn build_expression(selector: &str, mode: Mode, all: bool) -> String {
    // We escape the selector via JSON.stringify equivalent so quotes etc.
    // round-trip safely.
    let sel_lit = serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".into());

    let extract = match mode {
        Mode::Html => "el.outerHTML",
        Mode::Text => "el.textContent",
        Mode::Attrs => {
            "(() => { \
                const o = {}; \
                for (const a of el.attributes) o[a.name] = a.value; \
                return o; \
            })()"
        }
    };

    if all {
        format!(
            "JSON.stringify((() => {{ \
                const els = Array.from(document.querySelectorAll({sel_lit})); \
                return els.map(el => {extract}); \
            }})())"
        )
    } else {
        format!(
            "JSON.stringify((() => {{ \
                const el = document.querySelector({sel_lit}); \
                if (!el) return null; \
                return {extract}; \
            }})())"
        )
    }
}

fn print_result(value: &serde_json::Value, mode: Mode, all: bool) {
    match (mode, all) {
        (Mode::Text, false) => {
            // String value — print without JSON-quoting.
            if let Some(s) = value.as_str() {
                println!("{s}");
            } else {
                println!("{value}");
            }
        }
        (Mode::Html, false) => {
            if let Some(s) = value.as_str() {
                println!("{s}");
            } else {
                println!("{value}");
            }
        }
        (Mode::Attrs, false) => {
            // JSON object pretty-print.
            println!(
                "{}",
                serde_json::to_string_pretty(value)
                    .unwrap_or_else(|_| value.to_string())
            );
        }
        (_, true) => {
            // Array — pretty-print as JSON.
            println!(
                "{}",
                serde_json::to_string_pretty(value)
                    .unwrap_or_else(|_| value.to_string())
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_from_flags_default() {
        assert_eq!(Mode::from_flags(false, false, false).unwrap(), Mode::Html);
    }

    #[test]
    fn mode_from_flags_explicit() {
        assert_eq!(Mode::from_flags(true, false, false).unwrap(), Mode::Html);
        assert_eq!(Mode::from_flags(false, true, false).unwrap(), Mode::Text);
        assert_eq!(Mode::from_flags(false, false, true).unwrap(), Mode::Attrs);
    }

    #[test]
    fn mode_from_flags_rejects_combos() {
        assert!(Mode::from_flags(true, true, false).is_err());
        assert!(Mode::from_flags(true, false, true).is_err());
        assert!(Mode::from_flags(true, true, true).is_err());
    }

    #[test]
    fn build_expression_first_html() {
        let e = build_expression("h1", Mode::Html, false);
        assert!(e.contains("querySelector(\"h1\")"));
        assert!(e.contains("el.outerHTML"));
        assert!(e.contains("if (!el) return null;"));
    }

    #[test]
    fn build_expression_all_text() {
        let e = build_expression(".row", Mode::Text, true);
        assert!(e.contains("querySelectorAll(\".row\")"));
        assert!(e.contains("el.textContent"));
        assert!(e.contains("els.map"));
    }

    #[test]
    fn build_expression_attrs() {
        let e = build_expression("[data-id]", Mode::Attrs, false);
        assert!(e.contains("querySelector(\"[data-id]\")"));
        assert!(e.contains("for (const a of el.attributes)"));
    }

    #[test]
    fn build_expression_escapes_selector() {
        // Selector containing a double-quote should be JSON-escaped.
        let e = build_expression("a[href=\"x\"]", Mode::Html, false);
        assert!(e.contains("a[href=\\\"x\\\"]"), "got: {e}");
    }
}
