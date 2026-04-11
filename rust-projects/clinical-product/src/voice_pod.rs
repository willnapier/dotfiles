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

/// Called before a generation request. If the pod is managed and idle beyond
/// the configured timeout, stop it first. Then ensure it's running.
pub async fn prepare_for_request(client: &Client, config: &PodConfig) -> Result<()> {
    if !config.has_pod() {
        return Ok(()); // externally managed pod — nothing to do
    }

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
    Ok(())
}
