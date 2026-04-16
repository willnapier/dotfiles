use anyhow::{Context, Result};
use std::path::PathBuf;

use super::config::RegistryConfig;
use super::repo;
use super::types::{PractitionerAssignment, RegistryClient, RegistryFunding, RegistryReferrer};

/// Import a single client from the local ~/Clinical/clients/ directory into the registry.
/// Returns the client ID if successfully imported.
pub fn import_client(
    config: &RegistryConfig,
    client_id: &str,
    clinical_root: &PathBuf,
) -> Result<String> {
    let client_dir = clinical_root.join("clients").join(client_id);
    if !client_dir.exists() {
        anyhow::bail!("Client directory not found: {}", client_dir.display());
    }

    // Find identity.yaml (Route C: at root, Route A: in private/)
    let identity_path = if client_dir.join("identity.yaml").exists() {
        client_dir.join("identity.yaml")
    } else if client_dir.join("private").join("identity.yaml").exists() {
        client_dir.join("private").join("identity.yaml")
    } else {
        anyhow::bail!(
            "No identity.yaml found for client {} (checked root and private/)",
            client_id
        );
    };

    let content = std::fs::read_to_string(&identity_path)
        .with_context(|| format!("Failed to read {}", identity_path.display()))?;

    // Identity files may contain multiple YAML documents (--- separators).
    // Extract only the first document for parsing.
    let first_doc = if content.contains("\n---\n") {
        content.splitn(2, "\n---\n").next().unwrap_or(&content)
    } else if content.starts_with("---\n") {
        // Starts with ---, take everything up to the next ---
        let after_first = &content[4..];
        after_first.splitn(2, "\n---").next().unwrap_or(after_first)
    } else {
        &content
    };

    // Parse with serde_yaml::Value first for flexible handling
    let value: serde_yaml::Value = serde_yaml::from_str(first_doc)
        .with_context(|| format!("Failed to parse YAML for {}", client_id))?;

    let registry_client = convert_identity_to_registry(client_id, &value)?;

    // Save to registry
    super::client::save_client(config, &registry_client)?;

    // Create a default assignment for the current practitioner
    if !config.practitioner_id.is_empty() {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let assignments = vec![PractitionerAssignment {
            practitioner_id: config.practitioner_id.clone(),
            since: today,
            primary: true,
        }];
        super::client::save_assignments(config, client_id, &assignments)?;
    }

    // Copy correspondence files if they exist
    let correspondence_src = client_dir.join("correspondence");
    if correspondence_src.exists() {
        let correspondence_dst = config.client_dir(client_id).join("correspondence");
        std::fs::create_dir_all(&correspondence_dst)?;
        copy_dir_contents(&correspondence_src, &correspondence_dst)?;
    }

    Ok(client_id.to_string())
}

/// Import all clients from ~/Clinical/clients/ into the registry.
/// Returns (imported_count, skipped_count, error_count).
pub fn import_all(
    config: &RegistryConfig,
    clinical_root: &PathBuf,
) -> Result<(usize, usize, usize)> {
    let clients_dir = clinical_root.join("clients");
    if !clients_dir.exists() {
        anyhow::bail!("Clinical clients directory not found: {}", clients_dir.display());
    }

    let mut entries: Vec<String> = std::fs::read_dir(&clients_dir)?
        .filter_map(|e| {
            let e = e.ok()?;
            if e.file_type().ok()?.is_dir() {
                let name = e.file_name().to_string_lossy().to_string();
                if !name.starts_with('.') {
                    return Some(name);
                }
            }
            None
        })
        .collect();
    entries.sort();

    let mut imported = 0;
    let mut skipped = 0;
    let mut errors = 0;

    for client_id in &entries {
        // Skip if already in registry
        if config.client_dir(client_id).join("identity.yaml").exists() {
            skipped += 1;
            continue;
        }

        match import_client(config, client_id, clinical_root) {
            Ok(_) => {
                imported += 1;
                println!("  Imported: {}", client_id);
            }
            Err(e) => {
                errors += 1;
                eprintln!("  Error importing {}: {}", client_id, e);
            }
        }
    }

    // Commit all imports in one batch
    if imported > 0 {
        repo::add_and_commit(
            &config.local_path,
            &["clients/"],
            &format!("Import {} clients from local clinical directory", imported),
        )?;
    }

    Ok((imported, skipped, errors))
}

/// Convert a serde_yaml::Value (from identity.yaml) to a RegistryClient.
fn convert_identity_to_registry(
    client_id: &str,
    value: &serde_yaml::Value,
) -> Result<RegistryClient> {
    let get_str = |key: &str| -> Option<String> {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    };

    let name = get_str("name").unwrap_or_else(|| client_id.to_string());

    // Parse tm3_id flexibly (can be string or number in source)
    let tm3_id = value.get("tm3_id").and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_i64().map(|i| i as u64))
            .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
    });

    // Parse funding section
    let funding = if let Some(f) = value.get("funding") {
        RegistryFunding {
            funding_type: f
                .get("type")
                .or_else(|| f.get("funding_type"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            rate: f.get("rate").and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_i64().map(|i| i as f64))
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            }),
            session_duration: f
                .get("session_duration")
                .and_then(|v| v.as_u64().map(|u| u as u32)),
            contact: f.get("contact").and_then(|v| v.as_str()).map(|s| s.to_string()),
            policy: f.get("policy").and_then(|v| v.as_str()).map(|s| s.to_string()),
            email: f.get("email").and_then(|v| v.as_str()).map(|s| s.to_string()),
        }
    } else {
        RegistryFunding::default()
    };

    // Parse referrer section — Route C identity uses referral_via (flat string),
    // registry uses structured referrer section.
    let referrer = if let Some(r) = value.get("referrer") {
        RegistryReferrer {
            name: r.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()),
            role: r.get("role").and_then(|v| v.as_str()).map(|s| s.to_string()),
            practice: r.get("practice").and_then(|v| v.as_str()).map(|s| s.to_string()),
            email: r.get("email").and_then(|v| v.as_str()).map(|s| s.to_string()),
            credentials: r.get("credentials").and_then(|v| v.as_str()).map(|s| s.to_string()),
            gmc: r.get("gmc").and_then(|v| v.as_str()).map(|s| s.to_string()),
        }
    } else if let Some(via) = get_str("referral_via") {
        // Route C flat field → structured referrer
        RegistryReferrer {
            name: Some(via),
            ..RegistryReferrer::default()
        }
    } else {
        RegistryReferrer::default()
    };

    Ok(RegistryClient {
        client_id: client_id.to_string(),
        name,
        dob: get_str("dob"),
        address: get_str("address"),
        phone: get_str("phone"),
        email: get_str("email"),
        tm3_id,
        status: get_str("status").unwrap_or_else(|| "active".to_string()),
        discharge_date: get_str("discharge_date"),
        funding,
        referrer,
        diagnosis: get_str("diagnosis"),
        diagnostic_code: get_str("diagnostic_code"),
    })
}

/// Copy contents of one directory to another (non-recursive for simplicity).
fn copy_dir_contents(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());

        if file_type.is_file() {
            std::fs::copy(entry.path(), &dst_path)?;
        }
    }
    Ok(())
}
