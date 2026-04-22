//! On-device voice dictation via whisper.cpp (large-v3-turbo).
//!
//! Browser captures audio at the device's native sample rate, downsamples to
//! 16 kHz mono Float32 in an AudioWorklet, and streams it over a WebSocket
//! binary channel. This module maintains a rolling PCM buffer and invokes
//! whisper every time the buffer has grown enough to yield useful new text.
//!
//! ## Privacy
//! No audio ever leaves the local machine. There is no cloud fallback — a
//! failing local inference surfaces an error, it does not degrade to a
//! network service.
//!
//! ## Vocabulary
//! Per-practitioner vocabulary feeds whisper's `initial_prompt`, biasing the
//! decoder toward domain-specific terms (ACT/CBS terminology, client
//! initials, etc.). See [`load_vocab`] / [`save_vocab`]. The file lives at
//! `<schedules_dir>/<practitioner_id>/dictation-vocab.md` when a
//! practitioner is known, otherwise at
//! `<config_dir>/dictation-vocab.md` as a shared fallback.
//!
//! ## Model
//! `ggml-large-v3-turbo.bin` is downloaded on first use into
//! `<cache_dir>/practiceforge/whisper/` and subsequently loaded from disk.

use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;
use std::sync::Arc;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Target sample rate fed to whisper. The AudioWorklet in the browser
/// downsamples to this rate before transmission.
pub const SAMPLE_RATE: u32 = 16_000;

/// Rolling window fed to whisper on each decode, in seconds. Large enough
/// to give whisper context, small enough that decodes are snappy.
const DECODE_WINDOW_SECS: f32 = 10.0;

/// Minimum PCM length (seconds) before emitting a first partial.
const MIN_DECODE_SECS: f32 = 1.2;

/// How often to re-decode while streaming, in seconds of new audio.
const DECODE_STRIDE_SECS: f32 = 0.8;

/// Conservative upper bound on how much PCM we retain in the session buffer.
/// Whisper itself uses the last [`DECODE_WINDOW_SECS`]; anything earlier is
/// kept only so finalize() can replay the full recording if desired.
const MAX_BUFFER_SECS: f32 = 120.0;

/// Whisper's initial_prompt token limit (approx). Truncate vocab to stay
/// under this; whisper will otherwise silently ignore excess tokens.
const INITIAL_PROMPT_MAX_CHARS: usize = 900; // ~224 tokens, conservatively

/// Model filename.
const MODEL_FILENAME: &str = "ggml-large-v3-turbo.bin";

/// Upstream download URL.
const MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin";

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Directory where the whisper model binary lives.
pub fn model_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("practiceforge")
        .join("whisper")
}

/// Full path to the model binary.
pub fn model_path() -> PathBuf {
    model_cache_dir().join(MODEL_FILENAME)
}

/// Path to a practitioner's dictation vocab file, or the shared fallback if
/// no practitioner is supplied.
///
/// Convention:
/// - practitioner known → `<schedules_dir>/<practitioner_id>/dictation-vocab.md`
/// - no practitioner    → `<config_dir>/dictation-vocab.md`
pub fn vocab_path(practitioner_id: Option<&str>) -> PathBuf {
    if let Some(prac) = practitioner_id {
        let prac = prac.trim();
        if !prac.is_empty() {
            let sched = crate::scheduling::SchedulingConfig::default();
            let schedules_dir = shellexpand::tilde(&sched.schedules_dir).to_string();
            return PathBuf::from(schedules_dir)
                .join(prac)
                .join("dictation-vocab.md");
        }
    }
    // Fallback — shared vocab in the practiceforge config dir. Uses
    // crate::config::config_dir() (not dirs::config_dir) to preserve the
    // ~/.config/practiceforge convention on macOS.
    crate::config::config_dir().join("dictation-vocab.md")
}

// ---------------------------------------------------------------------------
// Vocabulary (initial_prompt) load / save
// ---------------------------------------------------------------------------

/// The default vocabulary seed — ACT/CBS therapy terminology plus placeholders
/// for client initials. Plain markdown, practitioner-editable.
const DEFAULT_VOCAB: &str = concat!(
    "# Dictation vocabulary\n\n",
    "This text is fed to whisper as its `initial_prompt`. It biases the\n",
    "decoder toward therapy vocabulary and names that would otherwise be\n",
    "mis-transcribed. Keep under about 900 characters (approx 224 tokens).\n\n",
    "ACT/CBS terminology: defusion, cognitive fusion, creative hopelessness,\n",
    "values, committed action, workability, pliance, tracking, augmenting,\n",
    "psychological flexibility, hexaflex, self-as-context, self-as-content,\n",
    "self-as-process, present-moment awareness, acceptance, willingness,\n",
    "experiential avoidance, mindfulness, relational frame, deictic framing.\n\n",
    "Related modalities: compassion-focused therapy, schema therapy,\n",
    "functional analytic psychotherapy, dialectical behaviour therapy,\n",
    "behavioural activation, exposure and response prevention.\n\n",
    "Client initials: (add commonly-seen initials here, e.g. AB, CD, EF)\n",
);

/// Read the vocab body for a practitioner. If no file exists, a seed file is
/// written at the resolved path (best effort) and its contents returned.
///
/// The returned string is truncated to [`INITIAL_PROMPT_MAX_CHARS`] with a
/// `tracing::warn`-style `eprintln` when truncation happens.
pub fn load_vocab(practitioner_id: Option<&str>) -> String {
    let path = vocab_path(practitioner_id);

    let body = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => {
            // Seed a sensible default. Best-effort — if the parent dir
            // can't be created we still return the default text.
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, DEFAULT_VOCAB);
            DEFAULT_VOCAB.to_string()
        }
    };

    // Strip a leading H1 ("# Title") — it's a display label, not prompt
    // content — and trim surrounding whitespace.
    let stripped = strip_leading_h1(&body);

    if stripped.chars().count() > INITIAL_PROMPT_MAX_CHARS {
        eprintln!(
            "[dictation] vocab at {} is {} chars; truncating to {} for whisper initial_prompt",
            path.display(),
            stripped.chars().count(),
            INITIAL_PROMPT_MAX_CHARS,
        );
        let mut truncated = String::with_capacity(INITIAL_PROMPT_MAX_CHARS);
        for (i, c) in stripped.chars().enumerate() {
            if i >= INITIAL_PROMPT_MAX_CHARS {
                break;
            }
            truncated.push(c);
        }
        truncated
    } else {
        stripped
    }
}

/// Overwrite the vocab file for this practitioner.
pub fn save_vocab(practitioner_id: Option<&str>, body: &str) -> Result<()> {
    let path = vocab_path(practitioner_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dictation vocab dir {}", parent.display()))?;
    }
    std::fs::write(&path, body)
        .with_context(|| format!("write dictation vocab {}", path.display()))?;
    Ok(())
}

/// Strip a leading markdown H1 (`# …\n`) from the front of the string.
fn strip_leading_h1(s: &str) -> String {
    let trimmed = s.trim_start();
    if let Some(rest) = trimmed.strip_prefix("# ") {
        if let Some(nl) = rest.find('\n') {
            return rest[nl + 1..].trim().to_string();
        }
        // H1 only, nothing else.
        return String::new();
    }
    trimmed.trim().to_string()
}

// ---------------------------------------------------------------------------
// Model download
// ---------------------------------------------------------------------------

/// Ensure the whisper model binary is on disk. Downloads it if absent,
/// invoking `progress` periodically with an integer percentage (0–100).
///
/// Returns the path to the model on success.
pub async fn ensure_model<F>(progress: F) -> Result<PathBuf>
where
    F: Fn(u8) + Send + 'static,
{
    let path = model_path();
    if path.exists() {
        // Sanity — at least 100 MB. A truncated download would fall below.
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() >= 100 * 1024 * 1024 {
                progress(100);
                return Ok(path);
            }
        }
    }

    std::fs::create_dir_all(model_cache_dir())
        .with_context(|| format!("create {}", model_cache_dir().display()))?;

    // Download to a .part file, then rename, to avoid leaving half files.
    let part = path.with_extension("part");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60 * 30))
        .build()
        .context("build reqwest client")?;

    let resp = client
        .get(MODEL_URL)
        .send()
        .await
        .with_context(|| format!("GET {}", MODEL_URL))?
        .error_for_status()
        .context("whisper model HTTP error")?;

    let total = resp.content_length().unwrap_or(0);

    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let mut file = tokio::fs::File::create(&part)
        .await
        .with_context(|| format!("create {}", part.display()))?;

    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut last_pct: u8 = 0;

    progress(0);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("download chunk")?;
        file.write_all(&chunk).await.context("write chunk")?;
        downloaded += chunk.len() as u64;
        if total > 0 {
            let pct = ((downloaded * 100) / total).min(99) as u8;
            if pct > last_pct {
                last_pct = pct;
                progress(pct);
            }
        }
    }
    file.flush().await.ok();
    drop(file);

    tokio::fs::rename(&part, &path)
        .await
        .with_context(|| format!("rename {} -> {}", part.display(), path.display()))?;

    progress(100);
    Ok(path)
}

// ---------------------------------------------------------------------------
// DictationSession — per-websocket whisper wrapper
// ---------------------------------------------------------------------------

/// One active dictation stream. Owns the rolling PCM buffer and the
/// whisper context. Construct via [`DictationSession::new`], feed PCM
/// samples with [`feed`](Self::feed), and call
/// [`finalize`](Self::finalize) once the user stops speaking.
pub struct DictationSession {
    context: Arc<WhisperContext>,
    /// Rolling mono Float32 PCM at [`SAMPLE_RATE`].
    buffer: Vec<f32>,
    /// Initial-prompt text biasing whisper's decoder.
    prompt: String,
    /// Total audio we've already emitted text for, so the next feed() only
    /// returns the delta.
    emitted_text: String,
    /// Buffer length at the last decode, so we throttle decodes.
    last_decode_len: usize,
}

impl DictationSession {
    /// Create a new session. `model_path` must point at an existing
    /// whisper-cpp ggml binary. `prompt` is the initial_prompt text fed to
    /// whisper on every decode.
    pub fn new(model_path: &std::path::Path, prompt: String) -> Result<Self> {
        let mut ctx_params = WhisperContextParameters::default();
        // `use_gpu` defaults to true on builds with the metal feature;
        // be explicit so behaviour is obvious.
        ctx_params.use_gpu(true);

        let ctx = WhisperContext::new_with_params(
            model_path.to_string_lossy().as_ref(),
            ctx_params,
        )
        .map_err(|e| anyhow!("whisper init failed: {e}"))?;

        Ok(Self {
            context: Arc::new(ctx),
            buffer: Vec::with_capacity(SAMPLE_RATE as usize * 30),
            prompt,
            emitted_text: String::new(),
            last_decode_len: 0,
        })
    }

    /// Append new PCM samples and, if enough new audio has accumulated,
    /// re-decode the trailing window and return any new text.
    ///
    /// The returned `String` is the *cumulative* partial transcript for
    /// this session (what should go into the textarea), not just the
    /// delta. Callers can compare with what they previously rendered to
    /// know what changed.
    pub fn feed(&mut self, chunk: &[f32]) -> Option<String> {
        self.buffer.extend_from_slice(chunk);

        // Cap buffer at MAX_BUFFER_SECS — discard oldest audio. We keep
        // emitted_text across this boundary so the user never sees text
        // disappear.
        let max_samples = (MAX_BUFFER_SECS * SAMPLE_RATE as f32) as usize;
        if self.buffer.len() > max_samples {
            let drop = self.buffer.len() - max_samples;
            self.buffer.drain(0..drop);
            self.last_decode_len = self.last_decode_len.saturating_sub(drop);
        }

        let min_samples = (MIN_DECODE_SECS * SAMPLE_RATE as f32) as usize;
        let stride = (DECODE_STRIDE_SECS * SAMPLE_RATE as f32) as usize;

        if self.buffer.len() < min_samples {
            return None;
        }
        if self.buffer.len().saturating_sub(self.last_decode_len) < stride {
            return None;
        }

        let text = self.decode_window().ok()?;
        self.last_decode_len = self.buffer.len();

        if text == self.emitted_text {
            return None;
        }
        self.emitted_text = text.clone();
        Some(text)
    }

    /// Flush whatever remains in the buffer and return the best-effort
    /// final transcript.
    pub fn finalize(&mut self) -> String {
        // One last decode over the whole remaining window regardless of
        // stride, so short clips still get a transcription.
        if self.buffer.is_empty() {
            return self.emitted_text.clone();
        }
        if let Ok(text) = self.decode_window() {
            self.emitted_text = text;
        }
        self.emitted_text.clone()
    }

    /// Run whisper over the trailing [`DECODE_WINDOW_SECS`] of audio and
    /// return the text.
    fn decode_window(&self) -> Result<String> {
        let window_samples = (DECODE_WINDOW_SECS * SAMPLE_RATE as f32) as usize;
        let start = self.buffer.len().saturating_sub(window_samples);
        let window = &self.buffer[start..];

        let mut state = self
            .context
            .create_state()
            .map_err(|e| anyhow!("whisper create_state: {e}"))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(2);
        params.set_translate(false);
        params.set_language(Some("en"));
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_print_special(false);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_single_segment(false);
        // initial_prompt biases decoding toward practitioner vocab.
        if !self.prompt.is_empty() {
            params.set_initial_prompt(&self.prompt);
        }

        state
            .full(params, window)
            .map_err(|e| anyhow!("whisper full: {e}"))?;

        let n = state.full_n_segments();
        let mut out = String::new();
        for i in 0..n {
            if let Some(seg) = state.get_segment(i) {
                if let Ok(s) = seg.to_str_lossy() {
                    out.push_str(&s);
                }
            }
        }
        Ok(out.trim().to_string())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_leading_h1_removes_heading() {
        assert_eq!(strip_leading_h1("# Title\nbody"), "body");
        assert_eq!(strip_leading_h1("  # Title\n\nbody\n"), "body");
    }

    #[test]
    fn strip_leading_h1_keeps_plain_body() {
        assert_eq!(strip_leading_h1("just text"), "just text");
        assert_eq!(strip_leading_h1("not # a heading"), "not # a heading");
    }

    #[test]
    fn vocab_path_practitioner_under_schedules() {
        let p = vocab_path(Some("william"));
        let s = p.to_string_lossy();
        assert!(s.ends_with("william/dictation-vocab.md"), "{s}");
    }

    #[test]
    fn vocab_path_fallback_under_config() {
        let p = vocab_path(None);
        assert!(p.ends_with("dictation-vocab.md"));
        let s = p.to_string_lossy();
        assert!(s.contains("practiceforge"), "{s}");
    }

    #[test]
    fn vocab_path_empty_practitioner_falls_back() {
        let p = vocab_path(Some(""));
        let s = p.to_string_lossy();
        // Same location as None case.
        assert!(s.contains("practiceforge"), "{s}");
        assert!(!s.contains("schedules"), "{s}");
    }
}
