//! Microsoft Graph backend — sends via `POST /v1.0/me/sendMail`.
//!
//! For M365 tenants where `SmtpClientAuthenticationDisabled = $true`
//! blocks SMTP submission entirely, Graph is the alternative: same OAuth
//! token (with `Mail.Send` scope added), HTTP POST of a JSON message
//! object, and Exchange handles submission.
//!
//! The COHS (Change of Harley Street) tenant is our first such case.

use anyhow::{anyhow, Context, Result};
use data_encoding::BASE64;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

use crate::email::{Body, Envelope, MailTransport, Mailbox, TokenSource};

/// Configuration for [`GraphTransport`].
///
/// Uses the `/common` endpoint by default so any M365 tenant works with
/// the same binary. Override via `tenant` for single-tenant apps if needed.
#[derive(Debug, Clone)]
pub struct GraphConfig {
    /// Base URL — `https://graph.microsoft.com/v1.0` by default.
    pub base_url: String,
    /// Save a copy to Sent Items (usually yes).
    pub save_to_sent_items: bool,
}

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            base_url: "https://graph.microsoft.com/v1.0".to_string(),
            save_to_sent_items: true,
        }
    }
}

pub struct GraphTransport {
    config: GraphConfig,
    token_source: Arc<dyn TokenSource>,
    http: reqwest::blocking::Client,
}

impl GraphTransport {
    pub fn new(config: GraphConfig, token_source: Arc<dyn TokenSource>) -> Self {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest blocking client should build");
        Self { config, token_source, http }
    }
}

/// Build the Graph `/me/sendMail` JSON payload from an [`Envelope`].
///
/// Extracted as a pure function so it can be unit-tested without making
/// network calls. The Graph endpoint expects:
/// `{"message": {...}, "saveToSentItems": bool}`.
///
/// Body handling:
/// - `Body::Text(s)`   → `contentType: "Text"`.
/// - `Body::Html(s)`   → `contentType: "HTML"`.
/// - `Body::Multi{..}` → **HTML is chosen**. Graph's `/me/sendMail` JSON
///   variant has only a single body field; it cannot express
///   `multipart/alternative`. HTML wins because most modern clients prefer
///   it. Future work: swap to the raw-MIME `sendMail` variant
///   (`Content-Type: text/plain`, base64 MIME body) when a caller needs a
///   true text+HTML alternative.
///
/// The `from` field is included when present. Graph ignores it unless the
/// authenticated user has Send-As rights on the address; harmless either
/// way, useful when shared-mailbox sends are configured.
pub fn envelope_to_graph_json(env: &Envelope, save_to_sent: bool) -> Value {
    let (content_type, content) = match &env.body {
        Body::Text(s) => ("Text", s.clone()),
        Body::Html(s) => ("HTML", s.clone()),
        // See comment above for the text-vs-HTML tradeoff.
        Body::Multi { html, .. } => ("HTML", html.clone()),
    };

    let mut message = json!({
        "subject": env.subject,
        "body": { "contentType": content_type, "content": content },
        "toRecipients":  recipients_json(&env.to),
        "ccRecipients":  recipients_json(&env.cc),
        "bccRecipients": recipients_json(&env.bcc),
        "from": mailbox_json(&env.from),
    });

    if !env.attachments.is_empty() {
        let atts: Vec<Value> = env
            .attachments
            .iter()
            .map(|a| {
                json!({
                    "@odata.type": "#microsoft.graph.fileAttachment",
                    "name": a.filename,
                    "contentType": a.content_type,
                    "contentBytes": BASE64.encode(&a.data),
                })
            })
            .collect();
        message["attachments"] = Value::Array(atts);
    }

    json!({ "message": message, "saveToSentItems": save_to_sent })
}

fn mailbox_json(m: &Mailbox) -> Value {
    let mut addr = json!({ "address": m.email });
    if let Some(name) = &m.name {
        addr["name"] = Value::String(name.clone());
    }
    json!({ "emailAddress": addr })
}

fn recipients_json(list: &[Mailbox]) -> Value {
    Value::Array(list.iter().map(mailbox_json).collect())
}

impl MailTransport for GraphTransport {
    fn send(&self, envelope: &Envelope) -> Result<()> {
        let token = self
            .token_source
            .access_token()
            .context("fetching access token for Microsoft Graph")?;

        let payload = envelope_to_graph_json(envelope, self.config.save_to_sent_items);
        let url = format!("{}/me/sendMail", self.config.base_url.trim_end_matches('/'));

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&token)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .with_context(|| format!("POST {url}"))?;

        let status = resp.status();
        if status.as_u16() == 202 {
            return Ok(());
        }

        // Non-202 — surface a helpful diagnostic. Body may or may not exist.
        let body = resp.text().unwrap_or_else(|_| "<no body>".to_string());
        let preview: String = body.chars().take(800).collect();

        let hint = match status.as_u16() {
            401 => " — token expired or revoked; re-run `cohs-oauth init`",
            403 => " — token missing `Mail.Send` scope; re-run `cohs-oauth init` to re-consent with updated scopes",
            _ => "",
        };

        Err(anyhow!(
            "Graph /me/sendMail failed: HTTP {}{}. Response body: {}",
            status,
            hint,
            preview
        ))
    }

    fn name(&self) -> &str {
        "Microsoft Graph"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::email::{Attachment, Body, Envelope, Mailbox};

    fn simple_envelope(body: Body) -> Envelope {
        Envelope::builder(Mailbox::with_name("from@example.com", "Sender"))
            .to(Mailbox::with_name("to1@example.com", "Alice"))
            .to(Mailbox::new("to2@example.com"))
            .cc(Mailbox::new("cc@example.com"))
            .bcc(Mailbox::new("bcc@example.com"))
            .subject("hello")
            .body(body)
            .build()
    }

    #[test]
    fn text_body_produces_text_content_type() {
        let env = simple_envelope(Body::Text("plain".into()));
        let v = envelope_to_graph_json(&env, true);
        assert_eq!(v["message"]["body"]["contentType"], "Text");
        assert_eq!(v["message"]["body"]["content"], "plain");
    }

    #[test]
    fn html_body_produces_html_content_type() {
        let env = simple_envelope(Body::Html("<p>hi</p>".into()));
        let v = envelope_to_graph_json(&env, true);
        assert_eq!(v["message"]["body"]["contentType"], "HTML");
        assert_eq!(v["message"]["body"]["content"], "<p>hi</p>");
    }

    #[test]
    fn multi_body_picks_html() {
        let env = simple_envelope(Body::Multi {
            text: "plain alt".into(),
            html: "<p>rich</p>".into(),
        });
        let v = envelope_to_graph_json(&env, true);
        assert_eq!(v["message"]["body"]["contentType"], "HTML");
        assert_eq!(v["message"]["body"]["content"], "<p>rich</p>");
    }

    #[test]
    fn recipients_populated_correctly() {
        let env = simple_envelope(Body::Text("x".into()));
        let v = envelope_to_graph_json(&env, true);

        let to = v["message"]["toRecipients"].as_array().unwrap();
        assert_eq!(to.len(), 2);
        assert_eq!(to[0]["emailAddress"]["address"], "to1@example.com");
        assert_eq!(to[0]["emailAddress"]["name"], "Alice");
        assert_eq!(to[1]["emailAddress"]["address"], "to2@example.com");
        // Bare-address mailbox has no name key.
        assert!(to[1]["emailAddress"].get("name").is_none());

        assert_eq!(v["message"]["ccRecipients"][0]["emailAddress"]["address"], "cc@example.com");
        assert_eq!(v["message"]["bccRecipients"][0]["emailAddress"]["address"], "bcc@example.com");
        assert_eq!(v["message"]["from"]["emailAddress"]["address"], "from@example.com");
        assert_eq!(v["message"]["from"]["emailAddress"]["name"], "Sender");
    }

    #[test]
    fn attachment_is_base64_encoded() {
        let data = b"%PDF-1.4...".to_vec();
        let expected = BASE64.encode(&data);

        let env = Envelope::builder(Mailbox::new("from@example.com"))
            .to(Mailbox::new("to@example.com"))
            .subject("with attachment")
            .body(Body::Text("see attached".into()))
            .attachment(Attachment {
                filename: "test.pdf".into(),
                content_type: "application/pdf".into(),
                data,
            })
            .build();

        let v = envelope_to_graph_json(&env, true);
        let atts = v["message"]["attachments"].as_array().unwrap();
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0]["@odata.type"], "#microsoft.graph.fileAttachment");
        assert_eq!(atts[0]["name"], "test.pdf");
        assert_eq!(atts[0]["contentType"], "application/pdf");
        assert_eq!(atts[0]["contentBytes"], expected);
    }

    #[test]
    fn save_to_sent_items_flag_respected() {
        let env = simple_envelope(Body::Text("x".into()));

        let v_true = envelope_to_graph_json(&env, true);
        assert_eq!(v_true["saveToSentItems"], true);

        let v_false = envelope_to_graph_json(&env, false);
        assert_eq!(v_false["saveToSentItems"], false);
    }

    #[test]
    fn no_attachments_field_when_empty() {
        let env = simple_envelope(Body::Text("x".into()));
        let v = envelope_to_graph_json(&env, true);
        // We only add `attachments` when non-empty — Graph tolerates either
        // shape but omitting the empty array keeps payloads minimal.
        assert!(v["message"].get("attachments").is_none());
    }
}
