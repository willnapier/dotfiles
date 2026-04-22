//! Frontier LLM backends for clinical note generation.
//!
//! Two production paths:
//! - `anthropic`: direct API access (pay-per-token, requires api_key in secrets.toml)
//! - `claude-cli`: uses the `claude -p` subprocess (Claude subscription, no API key needed)
//!
//! The Ollama backend remains in `admin_dashboard/handlers.rs` in archive posture —
//! still compiles and is selected when `[ai] backend = "ollama"`.
//!
//! Use `load_backend()` to get whichever is configured. Use `AiBackend` for
//! the unified interface both handlers call.

use anyhow::{bail, Context, Result};
use futures_util::{Stream, StreamExt};
use futures_util::stream::BoxStream;
use serde_json::json;

const ANTHROPIC_API: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
// Prompt-caching beta — cache the system prompt across all notes in a session.
const ANTHROPIC_BETA: &str = "prompt-caching-2024-07-31";

// Enough for a 400-word note with formulation at Opus verbosity; Haiku/Sonnet use far fewer.
const DEFAULT_MAX_TOKENS: u32 = 900;

// ─── Anthropic API backend ────────────────────────────────────────────────────

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

    /// Streaming generation. Parses Anthropic's SSE `content_block_delta` events.
    ///
    /// `model_override` lets a single request pin a different model than the
    /// backend's configured default — used by the A/B compare view to run
    /// Haiku/Sonnet/Opus variants through the same subscription-free path.
    pub async fn generate_stream(
        &self,
        system: String,
        prompt: String,
        model_override: Option<String>,
    ) -> Result<impl Stream<Item = String> + 'static> {
        let model = model_override.as_deref().unwrap_or(self.model.as_str());
        let body = json!({
            "model": model,
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

// ─── Claude CLI backend (subscription) ───────────────────────────────────────

/// Generates notes by shelling out to `claude -p`. Uses the existing Claude
/// Code auth (OAuth subscription) — no API key required in PracticeForge config.
pub struct ClaudeCliBackend {
    pub model: Option<String>,
}

impl ClaudeCliBackend {
    pub fn new(model: Option<String>) -> Self {
        Self { model }
    }

    pub async fn generate(&self, system: &str, prompt: &str) -> Result<String> {
        use tokio::io::AsyncWriteExt;

        let mut cmd = tokio::process::Command::new("claude");
        cmd.args(["-p", "--no-session-persistence", "--tools", ""])
            .arg("--system-prompt").arg(system);
        if let Some(m) = &self.model {
            cmd.arg("--model").arg(m);
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());

        let mut child = cmd.spawn().context("Failed to spawn claude CLI — is it on PATH?")?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes()).await?;
        }

        let out = child.wait_with_output().await
            .context("claude CLI process failed")?;

        if !out.status.success() {
            bail!("claude CLI exited {}", out.status);
        }

        Ok(String::from_utf8(out.stdout)?.trim().to_string())
    }

    /// Streams stdout from `claude -p` as raw byte chunks. The CLI streams
    /// tokens progressively even when stdout is piped, so this gives near-token
    /// granularity without needing stream-json parsing.
    ///
    /// `model_override` wins over the backend's configured default; it's
    /// threaded through by `/api/generate-stream` so the A/B compare view can
    /// pit Haiku, Sonnet and Opus against each other in one session.
    pub async fn generate_stream(
        &self,
        system: String,
        prompt: String,
        model_override: Option<String>,
    ) -> Result<BoxStream<'static, String>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut cmd = tokio::process::Command::new("claude");
        cmd.args(["-p", "--no-session-persistence", "--tools", ""])
            .arg("--system-prompt").arg(&system);
        let effective_model = model_override.as_deref().or(self.model.as_deref());
        if let Some(m) = effective_model {
            cmd.arg("--model").arg(m);
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());

        let mut child = cmd.spawn().context("Failed to spawn claude CLI")?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes()).await?;
        }

        let stdout = child.stdout.take().context("no stdout from claude CLI")?;

        // Detach child wait so the process isn't killed when child is dropped.
        tokio::spawn(async move { let _ = child.wait().await; });

        let stream = futures_util::stream::unfold(stdout, |mut out| async move {
            let mut buf = vec![0u8; 256];
            match out.read(&mut buf).await {
                Ok(0) | Err(_) => None,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    Some((text, out))
                }
            }
        }).boxed();

        Ok(stream)
    }
}

/// Load the Claude CLI backend if `[ai] backend = "claude-cli"` and `claude` is on PATH.
pub fn load_claude_cli_backend() -> Option<ClaudeCliBackend> {
    let ai = crate::config::load_ai_config();
    if ai.backend.as_deref() != Some("claude-cli") {
        return None;
    }
    // Verify the binary exists before committing to this path.
    if std::process::Command::new("claude").arg("--version").output().is_err() {
        return None;
    }
    Some(ClaudeCliBackend::new(ai.model))
}

// ─── Unified backend enum ─────────────────────────────────────────────────────

/// Unified interface over all AI backends. Handlers call `load_backend()` and
/// use this type — they don't need to know which backend is active.
pub enum AiBackend {
    Anthropic(AnthropicBackend),
    ClaudeCli(ClaudeCliBackend),
}

impl AiBackend {
    pub async fn generate(&self, system: &str, prompt: &str) -> Result<String> {
        match self {
            AiBackend::Anthropic(b) => b.generate(system, prompt).await,
            AiBackend::ClaudeCli(b) => b.generate(system, prompt).await,
        }
    }

    pub async fn generate_stream(
        &self,
        system: String,
        prompt: String,
        model_override: Option<String>,
    ) -> Result<BoxStream<'static, String>> {
        match self {
            AiBackend::Anthropic(b) => b.generate_stream(system, prompt, model_override).await.map(|s| s.boxed()),
            AiBackend::ClaudeCli(b) => b.generate_stream(system, prompt, model_override).await,
        }
    }

    pub fn backend_name(&self) -> &'static str {
        match self {
            AiBackend::Anthropic(_) => "anthropic",
            AiBackend::ClaudeCli(_) => "claude-cli",
        }
    }
}

/// Return whichever backend is configured, in priority order: Anthropic → Claude CLI → None.
pub fn load_backend() -> Option<AiBackend> {
    if let Some(b) = load_anthropic_backend() {
        return Some(AiBackend::Anthropic(b));
    }
    if let Some(b) = load_claude_cli_backend() {
        return Some(AiBackend::ClaudeCli(b));
    }
    None
}
