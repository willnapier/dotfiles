use serde::{Deserialize, Serialize};

/// Central client record — the shareable subset of identity.yaml.
/// Stored in the registry repo at `clients/<ID>/identity.yaml`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RegistryClient {
    /// Derived from directory name, not stored in YAML
    #[serde(skip)]
    pub client_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dob: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tm3_id: Option<u64>,
    #[serde(default = "default_active")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discharge_date: Option<String>,
    #[serde(default)]
    pub funding: RegistryFunding,
    #[serde(default)]
    pub referrer: RegistryReferrer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnosis: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_code: Option<String>,
}

fn default_active() -> String {
    "active".to_string()
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct RegistryFunding {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub funding_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_duration: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct RegistryReferrer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub practice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gmc: Option<String>,
}

/// Practitioner assignment for a client — who sees them and since when.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PractitionerAssignment {
    pub practitioner_id: String,
    pub since: String,
    #[serde(default)]
    pub primary: bool,
}

/// A practitioner registered in the practice.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PractitionerInfo {
    pub id: String,
    pub name: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tailscale_ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// Practice-wide configuration.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PracticeConfig {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(default)]
    pub session_notes_mirror: bool,
}
