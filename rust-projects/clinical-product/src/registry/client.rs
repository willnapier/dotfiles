use anyhow::{bail, Context, Result};

use super::config::RegistryConfig;
use super::types::{PractitionerAssignment, RegistryClient};

/// List all client IDs in the registry.
pub fn list_client_ids(config: &RegistryConfig) -> Result<Vec<String>> {
    let clients_dir = config.clients_dir();
    if !clients_dir.exists() {
        return Ok(Vec::new());
    }

    let mut ids: Vec<String> = std::fs::read_dir(&clients_dir)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if entry.file_type().ok()?.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name != ".gitkeep" && !name.starts_with('.') {
                    return Some(name);
                }
            }
            None
        })
        .collect();

    ids.sort();
    Ok(ids)
}

/// List all clients with their identity data.
pub fn list_clients(config: &RegistryConfig) -> Result<Vec<RegistryClient>> {
    let ids = list_client_ids(config)?;
    let mut clients = Vec::with_capacity(ids.len());

    for id in &ids {
        match get_client(config, id) {
            Ok(client) => clients.push(client),
            Err(e) => eprintln!("Warning: failed to load client {}: {}", id, e),
        }
    }

    Ok(clients)
}

/// Load a single client's registry record.
pub fn get_client(config: &RegistryConfig, client_id: &str) -> Result<RegistryClient> {
    let identity_path = config.client_dir(client_id).join("identity.yaml");
    if !identity_path.exists() {
        bail!("Client {} not found in registry", client_id);
    }

    let content = std::fs::read_to_string(&identity_path)
        .with_context(|| format!("Failed to read {}", identity_path.display()))?;

    let mut client: RegistryClient = serde_yaml::from_str(&content)
        .with_context(|| format!("Failed to parse identity.yaml for {}", client_id))?;

    client.client_id = client_id.to_string();
    Ok(client)
}

/// Save a client record to the registry (creates directory if needed).
pub fn save_client(config: &RegistryConfig, client: &RegistryClient) -> Result<()> {
    let client_dir = config.client_dir(&client.client_id);
    std::fs::create_dir_all(&client_dir)?;
    std::fs::create_dir_all(client_dir.join("letters"))?;
    std::fs::create_dir_all(client_dir.join("correspondence"))?;

    let identity_path = client_dir.join("identity.yaml");
    let yaml = serde_yaml::to_string(client)
        .context("Failed to serialize client identity")?;

    // Prepend YAML document separator
    let content = format!("---\n{}", yaml);
    std::fs::write(&identity_path, content)?;

    Ok(())
}

/// Load practitioner assignments for a client.
pub fn get_assignments(
    config: &RegistryConfig,
    client_id: &str,
) -> Result<Vec<PractitionerAssignment>> {
    let path = config.client_dir(client_id).join("assignments.yaml");
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(&path)?;
    let assignments: Vec<PractitionerAssignment> = serde_yaml::from_str(&content)?;
    Ok(assignments)
}

/// Save practitioner assignments for a client.
pub fn save_assignments(
    config: &RegistryConfig,
    client_id: &str,
    assignments: &[PractitionerAssignment],
) -> Result<()> {
    let client_dir = config.client_dir(client_id);
    std::fs::create_dir_all(&client_dir)?;

    let path = client_dir.join("assignments.yaml");
    let yaml = serde_yaml::to_string(assignments)?;
    std::fs::write(&path, format!("---\n{}", yaml))?;

    Ok(())
}

/// Delete a client from the registry.
pub fn delete_client(config: &RegistryConfig, client_id: &str) -> Result<()> {
    let client_dir = config.client_dir(client_id);
    if !client_dir.exists() {
        bail!("Client {} not found in registry", client_id);
    }
    std::fs::remove_dir_all(&client_dir)?;
    Ok(())
}

/// Count clients by status.
pub fn count_by_status(config: &RegistryConfig) -> Result<(usize, usize)> {
    let clients = list_clients(config)?;
    let active = clients.iter().filter(|c| c.status == "active").count();
    let discharged = clients.iter().filter(|c| c.status == "discharged").count();
    Ok((active, discharged))
}

/// Format a client record for display.
pub fn format_client(client: &RegistryClient) -> String {
    let mut lines = Vec::new();
    lines.push(format!("  ID:     {}", client.client_id));
    lines.push(format!("  Name:   {}", client.name));
    if let Some(dob) = &client.dob {
        lines.push(format!("  DOB:    {}", dob));
    }
    if let Some(phone) = &client.phone {
        if !phone.is_empty() {
            lines.push(format!("  Phone:  {}", phone));
        }
    }
    if let Some(email) = &client.email {
        if !email.is_empty() {
            lines.push(format!("  Email:  {}", email));
        }
    }
    if let Some(tm3_id) = client.tm3_id {
        lines.push(format!("  TM3:    {}", tm3_id));
    }
    lines.push(format!("  Status: {}", client.status));
    if let Some(ft) = &client.funding.funding_type {
        lines.push(format!("  Funding: {}", ft));
    }
    if let Some(ref_name) = &client.referrer.name {
        lines.push(format!("  Referrer: {}", ref_name));
    }
    lines.join("\n")
}
