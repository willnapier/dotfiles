//! RunPod REST API client.
//!
//! Minimal client for pod lifecycle management (list/get/create/start/stop/delete)
//! and network volume operations. Auth via API key stored in OS keychain
//! (service `runpod-api-key`).
//!
//! RunPod REST docs: https://rest.runpod.io/v1/openapi.json

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::time::Duration;

const BASE_URL: &str = "https://rest.runpod.io/v1";

/// Get the RunPod API key from the OS keychain.
///
/// On macOS, reads from the `runpod-api-key` generic password entry.
/// On Linux, reads from libsecret under the same service name.
/// Cross-platform: macOS Keychain (`security`) or Linux secret-service (`secret-tool`).
pub fn load_api_key() -> Result<String> {
    let output = if cfg!(target_os = "macos") {
        Command::new("security")
            .args(["find-generic-password", "-s", "runpod-api-key", "-w"])
            .output()
            .context("Failed to run `security`")?
    } else {
        Command::new("secret-tool")
            .args(["lookup", "service", "runpod-api-key"])
            .output()
            .context("Failed to run `secret-tool` — is libsecret installed?")?
    };

    if !output.status.success() {
        if cfg!(target_os = "macos") {
            bail!("RunPod API key not found. Run: security add-generic-password -s runpod-api-key -a practiceforge -w <key>");
        } else {
            bail!("RunPod API key not found. Run: echo -n '<key>' | secret-tool store --label 'RunPod API Key' service runpod-api-key account practiceforge");
        }
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Pod status as returned by RunPod.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Pod {
    pub id: String,
    pub name: String,
    #[serde(rename = "desiredStatus")]
    pub desired_status: String,
    #[serde(rename = "costPerHr", default)]
    pub cost_per_hr: f64,
    #[serde(rename = "gpuCount", default)]
    pub gpu_count: u32,
    #[serde(rename = "imageName", default)]
    pub image_name: String,
    #[serde(rename = "templateId", default)]
    pub template_id: Option<String>,
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(rename = "portMappings", default)]
    pub port_mappings: serde_json::Value,
    #[serde(rename = "publicIp", default)]
    pub public_ip: Option<String>,
    #[serde(rename = "containerDiskInGb", default)]
    pub container_disk_gb: u32,
    #[serde(rename = "volumeInGb", default)]
    pub volume_gb: u32,
    #[serde(rename = "networkVolumeId", default)]
    pub network_volume_id: Option<String>,
    #[serde(rename = "lastStartedAt", default)]
    pub last_started_at: Option<String>,
    #[serde(rename = "createdAt", default)]
    pub created_at: Option<String>,
}

impl Pod {
    pub fn is_running(&self) -> bool {
        self.desired_status == "RUNNING"
    }

    pub fn is_stopped(&self) -> bool {
        self.desired_status == "EXITED" || self.desired_status == "STOPPED"
    }
}

/// Network volume info.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NetworkVolume {
    pub id: String,
    pub name: String,
    pub size: u32, // GB
    #[serde(rename = "dataCenterId", default)]
    pub data_center_id: String,
}

/// Input for creating a new pod.
#[derive(Debug, Clone, Serialize)]
pub struct PodCreateInput {
    pub name: String,
    #[serde(rename = "gpuTypeIds")]
    pub gpu_type_ids: Vec<String>,
    #[serde(rename = "imageName")]
    pub image_name: String,
    #[serde(rename = "containerDiskInGb")]
    pub container_disk_in_gb: u32,
    #[serde(rename = "networkVolumeId", skip_serializing_if = "Option::is_none")]
    pub network_volume_id: Option<String>,
    #[serde(rename = "volumeMountPath", skip_serializing_if = "Option::is_none")]
    pub volume_mount_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(rename = "cloudType", skip_serializing_if = "Option::is_none")]
    pub cloud_type: Option<String>,
    #[serde(rename = "dataCenterIds", skip_serializing_if = "Option::is_none")]
    pub data_center_ids: Option<Vec<String>>,
    #[serde(rename = "gpuCount")]
    pub gpu_count: u32,
}

/// Input for creating a network volume.
#[derive(Debug, Clone, Serialize)]
pub struct NetworkVolumeCreateInput {
    pub name: String,
    pub size: u32,
    #[serde(rename = "dataCenterId")]
    pub data_center_id: String,
}

/// RunPod API client (async — uses reqwest's async client to coexist with tokio).
pub struct Client {
    api_key: String,
    http: reqwest::Client,
}

impl Client {
    pub fn new() -> Result<Self> {
        let api_key = load_api_key()?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { api_key, http })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{}", BASE_URL, path))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
    }

    /// List all pods.
    pub async fn list_pods(&self) -> Result<Vec<Pod>> {
        let resp = self
            .request(reqwest::Method::GET, "/pods")
            .send()
            .await
            .context("Failed to call /pods")?;
        if !resp.status().is_success() {
            bail!("list_pods: HTTP {}", resp.status());
        }
        let pods: Vec<Pod> = resp.json().await.context("Failed to parse pod list")?;
        Ok(pods)
    }

    /// Get a single pod by ID.
    pub async fn get_pod(&self, pod_id: &str) -> Result<Pod> {
        let resp = self
            .request(reqwest::Method::GET, &format!("/pods/{}", pod_id))
            .send()
            .await
            .context("Failed to call get_pod")?;
        if !resp.status().is_success() {
            bail!("get_pod: HTTP {} for pod {}", resp.status(), pod_id);
        }
        let pod: Pod = resp.json().await.context("Failed to parse pod")?;
        Ok(pod)
    }

    /// Create a new pod.
    pub async fn create_pod(&self, input: &PodCreateInput) -> Result<Pod> {
        let resp = self
            .request(reqwest::Method::POST, "/pods")
            .json(input)
            .send()
            .await
            .context("Failed to POST /pods")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("create_pod: HTTP {}: {}", status, body);
        }
        let pod: Pod = resp.json().await.context("Failed to parse created pod")?;
        Ok(pod)
    }

    /// Start (resume) a stopped pod.
    pub async fn start_pod(&self, pod_id: &str) -> Result<()> {
        let resp = self
            .request(reqwest::Method::POST, &format!("/pods/{}/start", pod_id))
            .send()
            .await
            .context("Failed to POST /pods/{id}/start")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("start_pod: HTTP {}: {}", status, body);
        }
        Ok(())
    }

    /// Stop a running pod. Container disk is wiped; network volume persists.
    pub async fn stop_pod(&self, pod_id: &str) -> Result<()> {
        let resp = self
            .request(reqwest::Method::POST, &format!("/pods/{}/stop", pod_id))
            .send()
            .await
            .context("Failed to POST /pods/{id}/stop")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("stop_pod: HTTP {}: {}", status, body);
        }
        Ok(())
    }

    /// Hard-reset a pod (stronger than restart — tears down the container and
    /// re-provisions). Calls `POST /pods/{podId}/reset`.
    pub async fn reset_pod(&self, pod_id: &str) -> Result<()> {
        let resp = self
            .request(reqwest::Method::POST, &format!("/pods/{}/reset", pod_id))
            .send()
            .await
            .context("Failed to POST /pods/{id}/reset")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("reset_pod: HTTP {}: {}", status, body);
        }
        Ok(())
    }

    /// Permanently delete a pod.
    pub async fn delete_pod(&self, pod_id: &str) -> Result<()> {
        let resp = self
            .request(reqwest::Method::DELETE, &format!("/pods/{}", pod_id))
            .send()
            .await
            .context("Failed to DELETE /pods/{id}")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("delete_pod: HTTP {}: {}", status, body);
        }
        Ok(())
    }

    /// List network volumes.
    pub async fn list_network_volumes(&self) -> Result<Vec<NetworkVolume>> {
        let resp = self
            .request(reqwest::Method::GET, "/networkvolumes")
            .send()
            .await
            .context("Failed to call /networkvolumes")?;
        if !resp.status().is_success() {
            bail!("list_network_volumes: HTTP {}", resp.status());
        }
        let volumes: Vec<NetworkVolume> =
            resp.json().await.context("Failed to parse volumes")?;
        Ok(volumes)
    }

    /// Create a new network volume.
    pub async fn create_network_volume(
        &self,
        input: &NetworkVolumeCreateInput,
    ) -> Result<NetworkVolume> {
        let resp = self
            .request(reqwest::Method::POST, "/networkvolumes")
            .json(input)
            .send()
            .await
            .context("Failed to POST /networkvolumes")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("create_network_volume: HTTP {}: {}", status, body);
        }
        let vol: NetworkVolume = resp.json().await.context("Failed to parse volume")?;
        Ok(vol)
    }

    /// Delete a network volume.
    pub async fn delete_network_volume(&self, volume_id: &str) -> Result<()> {
        let resp = self
            .request(
                reqwest::Method::DELETE,
                &format!("/networkvolumes/{}", volume_id),
            )
            .send()
            .await
            .context("Failed to DELETE /networkvolumes/{id}")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("delete_network_volume: HTTP {}: {}", status, body);
        }
        Ok(())
    }
}
