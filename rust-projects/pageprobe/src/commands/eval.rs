//! `pageprobe eval` — runs a JS expression in the attached tab and prints
//! the value.
//!
//! Default behaviour: `Runtime.evaluate` with `returnByValue: true` so that
//! primitives, plain objects, and arrays come back as JSON. With `--await`
//! we set `awaitPromise: true` so a returned promise resolves before the
//! call returns. With `--json` the raw `RemoteObject` is emitted (handy when
//! you need to see the type / unserializable-value path).
//!
//! When the expression is `-`, JS is read from stdin — useful when the
//! expression contains shell-tricky characters.
use anyhow::{Context, Result, anyhow};
use chromiumoxide::cdp::js_protocol::runtime::{
    EvaluateParams, RemoteObject, RemoteObjectType,
};
use std::io::Read;

use crate::{cdp, state};

pub async fn run(expression: String, json: bool, await_promise: bool) -> Result<()> {
    let expr = if expression == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("reading expression from stdin")?;
        let trimmed = buf.trim().to_string();
        if trimmed.is_empty() {
            return Err(anyhow!("no expression provided on stdin"));
        }
        trimmed
    } else {
        expression
    };

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
        silent: None,
        context_id: None,
        return_by_value: Some(true),
        generate_preview: None,
        user_gesture: None,
        await_promise: Some(await_promise),
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

    // Always tear down before producing user-visible output so we don't
    // leave the handler task running, regardless of which path we take.
    let _ = browser.close().await;
    handle.abort();

    if let Some(exc) = &resp.result.exception_details {
        let msg = exc
            .exception
            .as_ref()
            .and_then(|e| e.description.clone())
            .unwrap_or_else(|| exc.text.clone());
        eprintln!("evaluation error: {msg}");
        std::process::exit(1);
    }

    let result = &resp.result.result;
    if json {
        println!("{}", serde_json::to_string_pretty(result)?);
    } else {
        println!("{}", render_remote_object(result));
    }

    Ok(())
}

/// Best-effort human-readable rendering of a `RemoteObject`. Primitives
/// print inline; objects and arrays pretty-print as JSON; functions and
/// undefined fall back to the `description` field.
fn render_remote_object(obj: &RemoteObject) -> String {
    if let Some(value) = &obj.value {
        return match obj.r#type {
            RemoteObjectType::String => value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string()),
            RemoteObjectType::Number | RemoteObjectType::Boolean | RemoteObjectType::Bigint => {
                value.to_string()
            }
            RemoteObjectType::Object => serde_json::to_string_pretty(value)
                .unwrap_or_else(|_| value.to_string()),
            _ => value.to_string(),
        };
    }
    if let Some(desc) = &obj.description {
        return desc.clone();
    }
    if let Some(unser) = &obj.unserializable_value {
        return format!("{:?}", unser);
    }
    match obj.r#type {
        RemoteObjectType::Undefined => "undefined".to_string(),
        _ => format!("{:?}", obj.r#type),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chromiumoxide::cdp::js_protocol::runtime::{RemoteObject, RemoteObjectType};

    fn make_object(t: RemoteObjectType, value: Option<serde_json::Value>) -> RemoteObject {
        RemoteObject {
            r#type: t,
            subtype: None,
            class_name: None,
            value,
            unserializable_value: None,
            description: None,
            deep_serialized_value: None,
            object_id: None,
            preview: None,
            custom_preview: None,
        }
    }

    #[test]
    fn render_string_unwraps_quotes() {
        let obj = make_object(
            RemoteObjectType::String,
            Some(serde_json::Value::String("hello".into())),
        );
        assert_eq!(render_remote_object(&obj), "hello");
    }

    #[test]
    fn render_number_inline() {
        let obj = make_object(
            RemoteObjectType::Number,
            Some(serde_json::json!(42)),
        );
        assert_eq!(render_remote_object(&obj), "42");
    }

    #[test]
    fn render_boolean_inline() {
        let obj = make_object(
            RemoteObjectType::Boolean,
            Some(serde_json::json!(true)),
        );
        assert_eq!(render_remote_object(&obj), "true");
    }

    #[test]
    fn render_undefined_label() {
        let obj = make_object(RemoteObjectType::Undefined, None);
        assert_eq!(render_remote_object(&obj), "undefined");
    }

    #[test]
    fn render_object_pretty_prints_json() {
        let obj = make_object(
            RemoteObjectType::Object,
            Some(serde_json::json!({"a": 1, "b": "two"})),
        );
        let rendered = render_remote_object(&obj);
        assert!(rendered.contains("\"a\""), "got: {rendered}");
        assert!(rendered.contains("\"two\""), "got: {rendered}");
    }

    #[test]
    fn render_function_uses_description() {
        let mut obj = make_object(RemoteObjectType::Function, None);
        obj.description = Some("function f() { ... }".into());
        assert_eq!(render_remote_object(&obj), "function f() { ... }");
    }
}
