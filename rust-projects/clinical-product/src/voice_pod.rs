//! Voice pod lifecycle management.
//!
//! High-level API for managing a practitioner's voice inference pod:
//! status, start, stop, and auto-start-on-demand with idle timeout.
//!
//! Safety: this module will ONLY touch the pod whose ID is configured in
//! `voice-config.toml` under `[pod] pod_id`. If that is empty or
//! `managed = false`, all management operations are no-ops and `clinical
//! note` assumes the pod is managed externally.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

use crate::runpod::{Client, Pod};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const MAX_START_WAIT: Duration = Duration::from_secs(300); // 5 min
const DEFAULT_IDLE_TIMEOUT_MIN: u32 = 15;
const OLLAMA_HEALTH_TIMEOUT: Duration = Duration::from_secs(5);
const RESTART_WAIT: Duration = Duration::from_secs(120);
const RESET_WAIT: Duration = Duration::from_secs(180);

/// Parsed `[pod]` section from voice-config.toml.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PodConfig {
    #[serde(default)]
    pub managed: bool,
    #[serde(default)]
    pub pod_id: String,
    #[serde(default)]
    pub network_volume_id: String,
    #[serde(default)]
    pub gpu_type: Option<String>,
    #[serde(default)]
    pub data_center_id: Option<String>,
    #[serde(default)]
    pub container_image: Option<String>,
    #[serde(rename = "idle_timeout_minutes", default)]
    pub idle_timeout_minutes: Option<u32>,
    #[serde(default)]
    pub name: Option<String>,
}

/// Runtime state persisted between invocations.
///
/// Kept in a separate file from the config because it mutates on every
/// `clinical note` invocation. Stored at `~/.config/clinical-product/voice-state.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PodState {
    /// ISO-8601 timestamp of the last successful generation.
    #[serde(default)]
    pub last_activity: Option<String>,
    /// ISO-8601 timestamp of the last pod state check (for cache).
    #[serde(default)]
    pub last_check: Option<String>,
    /// Cached `desired_status` from the last check.
    #[serde(default)]
    pub last_known_status: Option<String>,
}

impl PodConfig {
    pub fn idle_timeout(&self) -> Duration {
        Duration::from_secs(
            (self.idle_timeout_minutes.unwrap_or(DEFAULT_IDLE_TIMEOUT_MIN) as u64) * 60,
        )
    }

    pub fn has_pod(&self) -> bool {
        self.managed && !self.pod_id.is_empty()
    }
}

/// Load `[pod]` section from `~/.config/clinical-product/voice-config.toml`.
pub fn load_pod_config() -> Result<PodConfig> {
    let path = config_path();
    if !path.exists() {
        return Ok(PodConfig::default());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let value: toml::Value = toml::from_str(&content).context("Invalid TOML")?;
    let pod = value.get("pod");
    if pod.is_none() {
        return Ok(PodConfig::default());
    }
    let cfg: PodConfig = pod
        .unwrap()
        .clone()
        .try_into()
        .context("Failed to parse [pod] section")?;
    Ok(cfg)
}

pub fn config_path() -> PathBuf {
    home_config_dir().join("voice-config.toml")
}

pub fn state_path() -> PathBuf {
    home_config_dir().join("voice-state.toml")
}

fn home_config_dir() -> PathBuf {
    if let Some(home) = dirs::home_dir() {
        home.join(".config").join("clinical-product")
    } else {
        PathBuf::from(".")
    }
}

pub fn load_state() -> PodState {
    let path = state_path();
    if !path.exists() {
        return PodState::default();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_state(state: &PodState) -> Result<()> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let content = toml::to_string_pretty(state).context("Failed to serialize state")?;
    std::fs::write(&path, content).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

/// Update the `last_activity` timestamp to now.
pub fn record_activity() -> Result<()> {
    let mut state = load_state();
    state.last_activity = Some(Utc::now().to_rfc3339());
    save_state(&state)
}

/// Check whether the stored `last_activity` is older than `idle_timeout`.
pub fn is_idle(state: &PodState, idle_timeout: Duration) -> bool {
    let Some(ref ts) = state.last_activity else {
        return false; // no activity recorded — can't be idle
    };
    let Ok(last) = DateTime::parse_from_rfc3339(ts) else {
        return false;
    };
    let elapsed = Utc::now().signed_duration_since(last.with_timezone(&Utc));
    elapsed.num_seconds() as u64 > idle_timeout.as_secs()
}

/// Get the current pod state from the RunPod API.
pub async fn status(client: &Client, config: &PodConfig) -> Result<Option<Pod>> {
    if !config.has_pod() {
        return Ok(None);
    }
    let pod = client.get_pod(&config.pod_id).await?;
    Ok(Some(pod))
}

/// Start the configured pod if not already running. Waits for it to become
/// ready (polling), up to `MAX_START_WAIT`.
///
/// Returns `Ok(true)` if a start was actually issued; `Ok(false)` if the
/// pod was already running and no action was needed.
pub async fn ensure_running(client: &Client, config: &PodConfig) -> Result<bool> {
    if !config.has_pod() {
        bail!(
            "No managed pod configured. Set [pod] managed=true and pod_id in {}",
            config_path().display()
        );
    }

    let pod = client.get_pod(&config.pod_id).await?;
    if pod.is_running() {
        return Ok(false);
    }

    eprintln!("[voice-pod] Starting pod {}...", config.pod_id);
    client.start_pod(&config.pod_id).await?;

    // Poll until running or timeout
    let start = std::time::Instant::now();
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        let pod = client.get_pod(&config.pod_id).await?;
        if pod.is_running() {
            eprintln!(
                "[voice-pod] Pod running (took {:.0}s).",
                start.elapsed().as_secs_f64()
            );
            return Ok(true);
        }
        if start.elapsed() > MAX_START_WAIT {
            bail!(
                "Pod did not reach RUNNING within {}s. Current: {}",
                MAX_START_WAIT.as_secs(),
                pod.desired_status
            );
        }
        eprintln!(
            "[voice-pod] Status: {} ({:.0}s elapsed)",
            pod.desired_status,
            start.elapsed().as_secs_f64()
        );
    }
}

/// Stop the configured pod if running.
pub async fn ensure_stopped(client: &Client, config: &PodConfig) -> Result<bool> {
    if !config.has_pod() {
        return Ok(false);
    }

    let pod = client.get_pod(&config.pod_id).await?;
    if pod.is_stopped() {
        return Ok(false);
    }

    eprintln!("[voice-pod] Stopping pod {}...", config.pod_id);
    client.stop_pod(&config.pod_id).await?;
    Ok(true)
}

/// Probe whether Ollama is healthy on the given pod by hitting `/api/tags`
/// via the RunPod proxy URL. Returns `Ok(true)` if the endpoint responds
/// within 5 seconds, `Ok(false)` on timeout or HTTP error.
pub async fn probe_ollama_health(pod_id: &str) -> Result<bool> {
    let url = format!(
        "https://{}-8888.proxy.runpod.net/api/tags",
        pod_id
    );
    let http = reqwest::Client::builder()
        .timeout(OLLAMA_HEALTH_TIMEOUT)
        .build()?;

    match http.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => Ok(true),
        Ok(resp) => {
            eprintln!(
                "[voice-pod] Ollama health probe returned HTTP {}",
                resp.status()
            );
            Ok(false)
        }
        Err(e) => {
            eprintln!("[voice-pod] Ollama health probe failed: {}", e);
            Ok(false)
        }
    }
}

/// Poll for pod RUNNING status + Ollama health within a deadline.
/// Returns `true` if both are healthy before the deadline.
async fn wait_for_healthy(
    client: &Client,
    pod_id: &str,
    deadline: Duration,
) -> bool {
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > deadline {
            return false;
        }
        tokio::time::sleep(POLL_INTERVAL).await;

        // First check the pod is actually RUNNING
        match client.get_pod(pod_id).await {
            Ok(pod) if pod.is_running() => {}
            Ok(pod) => {
                eprintln!(
                    "[voice-pod] Waiting for RUNNING, currently {} ({:.0}s elapsed)",
                    pod.desired_status,
                    start.elapsed().as_secs_f64()
                );
                continue;
            }
            Err(e) => {
                eprintln!("[voice-pod] Error checking pod status: {}", e);
                continue;
            }
        }

        // Pod is running — probe Ollama
        match probe_ollama_health(pod_id).await {
            Ok(true) => return true,
            _ => {
                eprintln!(
                    "[voice-pod] Pod RUNNING but Ollama not yet healthy ({:.0}s elapsed)",
                    start.elapsed().as_secs_f64()
                );
            }
        }
    }
}

/// Called before a generation request. Ensures the pod is running AND Ollama
/// is healthy, with a multi-step recovery ladder if not:
///
///   Step 0: Idle check + ensure pod RUNNING (existing behaviour)
///   Step 1: Probe Ollama health — if healthy, proceed
///   Step 2: Restart pod via RunPod API, wait up to 120s, re-probe
///   Step 3: Hard reset pod (stop + start cycle), wait up to 180s, re-probe
///   Step 4: All recovery failed — return error
pub async fn prepare_for_request(client: &Client, config: &PodConfig) -> Result<()> {
    if !config.has_pod() {
        return Ok(()); // externally managed pod — nothing to do
    }

    // --- Step 0: idle check + ensure RUNNING ---
    let state = load_state();
    let timeout = config.idle_timeout();

    if is_idle(&state, timeout) {
        eprintln!(
            "[voice-pod] Pod idle > {} min; stopping before fresh start.",
            timeout.as_secs() / 60
        );
        let _ = ensure_stopped(client, config).await;
    }

    ensure_running(client, config).await?;

    // --- Step 1: probe Ollama health ---
    eprintln!("[voice-pod] Probing Ollama health...");
    if probe_ollama_health(&config.pod_id).await? {
        eprintln!("[voice-pod] Ollama healthy — ready.");
        return Ok(());
    }

    // --- Step 2: Ollama unhealthy — restart pod ---
    eprintln!("[voice-pod] Ollama unresponsive — restarting pod...");
    if let Err(e) = client.start_pod(&config.pod_id).await {
        eprintln!("[voice-pod] Restart request failed: {} — continuing to reset", e);
    } else if wait_for_healthy(client, &config.pod_id, RESTART_WAIT).await {
        eprintln!("[voice-pod] Ollama recovered after restart.");
        return Ok(());
    }

    // --- Step 3: restart failed — stop + start cycle (hard reset) ---
    eprintln!("[voice-pod] Restart did not recover Ollama — performing hard reset (stop + start)...");
    if let Err(e) = client.reset_pod(&config.pod_id).await {
        eprintln!("[voice-pod] Reset API call failed: {} — trying stop+start", e);
        // Fallback: manual stop then start
        let _ = client.stop_pod(&config.pod_id).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
        let _ = client.start_pod(&config.pod_id).await;
    }

    if wait_for_healthy(client, &config.pod_id, RESET_WAIT).await {
        eprintln!("[voice-pod] Ollama recovered after hard reset.");
        return Ok(());
    }

    // --- Step 4: all recovery failed ---
    bail!(
        "[voice-pod] All recovery attempts failed for pod {}. \
         Ollama on the pod is not responding after restart and hard reset. \
         Check the RunPod console for GPU/container issues.",
        config.pod_id
    );
}
