//! Frontier LLM backends for clinical note generation.
//!
//! The Anthropic backend is the production path. The Ollama backend
//! remains in `admin_dashboard/handlers.rs` in archive posture — it
//! still compiles and is selected when `[ai] backend = "ollama"`.

use anyhow::{bail, Context, Result};
use futures_util::{Stream, StreamExt};
use serde_json::json;

const ANTHROPIC_API: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
// Prompt-caching beta — cache the system prompt across all notes in a session.
const ANTHROPIC_BETA: &str = "prompt-caching-2024-07-31";

// Conservative defaults — enough for a 300-word note with formulation.
const DEFAULT_MAX_TOKENS: u32 = 700;

pub struct AnthropicBackend {
    pub api_key: String,
    pub model: String,
    client: reqwest::Client,
}

impl AnthropicBackend {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            client: reqwest::Client::new(),
        }
    }

    fn base_headers(&self) -> reqwest::header::HeaderMap {
        use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
        let mut h = HeaderMap::new();
        h.insert("x-api-key", HeaderValue::from_str(&self.api_key).unwrap());
        h.insert("anthropic-version", HeaderValue::from_static(ANTHROPIC_VERSION));
        h.insert("anthropic-beta", HeaderValue::from_static(ANTHROPIC_BETA));
        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        h
    }

    /// Non-streaming generation. Returns the complete note text.
    pub async fn generate(&self, system: &str, prompt: &str) -> Result<String> {
        let body = json!({
            "model": self.model,
            "max_tokens": DEFAULT_MAX_TOKENS,
            "system": [{
                "type": "text",
                "text": system,
                "cache_control": {"type": "ephemeral"}
            }],
            "messages": [{"role": "user", "content": prompt}]
        });

        let resp = self.client
            .post(ANTHROPIC_API)
            .headers(self.base_headers())
            .json(&body)
            .send()
            .await
            .context("Anthropic API request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            bail!("Anthropic API returned {}: {}", status, err);
        }

        let val: serde_json::Value = resp.json().await
            .context("Failed to parse Anthropic response")?;

        val["content"][0]["text"]
            .as_str()
            .context("No text in Anthropic response")
            .map(|s| s.to_string())
    }

    /// Streaming generation. Returns a `'static` stream of text token strings.
    ///
    /// Takes owned `String` arguments so the returned stream is `'static` and
    /// can be boxed for use in Axum SSE handlers. Parses Anthropic's SSE format
    /// (`content_block_delta` events) and yields each text chunk as it arrives.
    pub async fn generate_stream(
        &self,
        system: String,
        prompt: String,
    ) -> Result<impl Stream<Item = String> + 'static> {
        let body = json!({
            "model": self.model,
            "max_tokens": DEFAULT_MAX_TOKENS,
            "stream": true,
            "system": [{
                "type": "text",
                "text": system,
                "cache_control": {"type": "ephemeral"}
            }],
            "messages": [{"role": "user", "content": prompt}]
        });

        let resp = self.client
            .post(ANTHROPIC_API)
            .headers(self.base_headers())
            .json(&body)
            .send()
            .await
            .context("Anthropic API streaming request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            bail!("Anthropic API returned {}: {}", status, err);
        }

        // Anthropic sends proper SSE lines: "data: {json}\n\n"
        // We look for content_block_delta events and extract the text field.
        let stream = resp.bytes_stream().map(|chunk| {
            let chunk = chunk.unwrap_or_default();
            let text = String::from_utf8_lossy(&chunk);
            let mut tokens = String::new();

            for line in text.lines() {
                let line = line.trim();
                if !line.starts_with("data: ") {
                    continue;
                }
                let json_str = &line[6..];
                if let Ok(obj) = serde_json::from_str::<serde_json::Value>(json_str) {
                    if obj["type"].as_str() == Some("content_block_delta") {
                        if let Some(tok) = obj["delta"]["text"].as_str() {
                            tokens.push_str(tok);
                        }
                    }
                }
            }
            tokens
        });

        Ok(stream)
    }
}

/// Load the Anthropic backend from config + secrets, or return None if not configured.
pub fn load_anthropic_backend() -> Option<AnthropicBackend> {
    let ai = crate::config::load_ai_config();
    if ai.backend.as_deref() != Some("anthropic") {
        return None;
    }
    let secrets = crate::billing::secrets::BillingSecrets::load().ok()?;
    let api_key = secrets.ai.api_key?;
    if api_key.is_empty() {
        return None;
    }
    let model = ai.model
        .unwrap_or_else(|| "claude-haiku-4-5-20251001".to_string());
    Some(AnthropicBackend::new(api_key, model))
}
