//! Client data export — reads TM3 client cache and imports into the registry.

use anyhow::{Context, Result};
use std::path::PathBuf;

use crate::registry::config::RegistryConfig;
use crate::registry::types::{RegistryClient, RegistryFunding, RegistryReferrer};
use crate::tm3_clients::{self, TM3Client};

/// Result of a client migration run.
#[derive(Debug, Clone, Default)]
pub struct MigrationReport {
    pub imported: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl MigrationReport {
    pub fn total_processed(&self) -> usize {
        self.imported + self.skipped + self.errors.len()
    }
}

impl std::fmt::Display for MigrationReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Clients: {} imported, {} skipped, {} errors",
            self.imported,
            self.skipped,
            self.errors.len()
        )?;
        if !self.warnings.is_empty() {
            write!(f, ", {} warnings", self.warnings.len())?;
        }
        Ok(())
    }
}

/// Map a TM3Client record to a RegistryClient.
///
/// This is the central field-mapping function. TM3 fields map to registry
/// fields as follows:
///   - `id` -> `tm3_id`
///   - `surname` + `forename` -> `name` (formatted "Surname, Forename")
///   - `dateOfBirth` -> `dob`
///   - `email` -> `email`
///   - `number` (phone) -> `phone`
///   - `address` + `postCode` -> `address`
///   - `patientGroup` -> `funding.funding_type`
///   - `practitionerName` -> (ignored, handled via assignments)
pub fn map_tm3_to_registry(tm3: &TM3Client, client_id: &str) -> RegistryClient {
    let name = format!("{}, {}", tm3.surname, tm3.forename);

    let dob = tm3
        .date_of_birth
        .as_deref()
        .map(tm3_clients::clean_dob)
        .filter(|s| !s.is_empty());

    let address = match (&tm3.address, &tm3.post_code) {
        (Some(addr), Some(pc)) if !addr.is_empty() && !pc.is_empty() => {
            Some(format!("{}, {}", addr, pc))
        }
        (Some(addr), _) if !addr.is_empty() => Some(addr.clone()),
        _ => None,
    };

    let phone = tm3
        .number
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let email = tm3
        .email
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let funding = RegistryFunding {
        funding_type: tm3
            .patient_group
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        ..Default::default()
    };

    RegistryClient {
        client_id: client_id.to_string(),
        name,
        dob,
        address,
        phone,
        email,
        tm3_id: Some(tm3.id),
        status: "active".to_string(),
        discharge_date: None,
        funding,
        referrer: RegistryReferrer::default(),
        diagnosis: None,
        diagnostic_code: None,
    }
}

/// Derive a client ID from a TM3Client record.
///
/// Uses the pattern: first two letters of surname (uppercase) + last two digits of TM3 ID.
/// Example: "Briscoe" with TM3 ID 4392 -> "BR92".
pub fn derive_client_id(tm3: &TM3Client) -> String {
    let prefix: String = tm3
        .surname
        .chars()
        .filter(|c| c.is_alphabetic())
        .take(2)
        .collect::<String>()
        .to_uppercase();

    let suffix = format!("{:02}", tm3.id % 100);

    format!("{}{}", prefix, suffix)
}

/// Export all TM3 clients into the PracticeForge registry.
///
/// Reads the TM3 client cache, maps each to a RegistryClient, and imports
/// into the registry. Skips clients that already exist (by tm3_id match).
pub fn export_clients(
    registry_config: &RegistryConfig,
    _clinical_root: &PathBuf,
    dry_run: bool,
) -> Result<MigrationReport> {
    let mut report = MigrationReport::default();

    let tm3_clients = tm3_clients::load_cache()
        .context("Failed to load TM3 client cache. Run diary capture first to populate it.")?;

    eprintln!(
        "[tm3-migrate] Loaded {} clients from TM3 cache",
        tm3_clients.len()
    );

    // Build a set of TM3 IDs already in the registry to avoid duplicates
    let existing_clients = crate::registry::list_clients(registry_config).unwrap_or_default();
    let existing_tm3_ids: std::collections::HashSet<u64> = existing_clients
        .iter()
        .filter_map(|c| c.tm3_id)
        .collect();

    for tm3 in &tm3_clients {
        // Skip if already imported (matched by TM3 ID)
        if existing_tm3_ids.contains(&tm3.id) {
            report.skipped += 1;
            continue;
        }

        let client_id = derive_client_id(tm3);

        // Check if the client_id already exists (collision)
        if registry_config.client_dir(&client_id).exists() {
            report.warnings.push(format!(
                "Client ID {} already exists (TM3 #{} {} {}). Skipped.",
                client_id, tm3.id, tm3.surname, tm3.forename
            ));
            report.skipped += 1;
            continue;
        }

        if dry_run {
            println!(
                "  Would import: {} ({}, {}) as {}",
                tm3.id, tm3.surname, tm3.forename, client_id
            );
            report.imported += 1;
            continue;
        }

        let registry_client = map_tm3_to_registry(tm3, &client_id);

        match crate::registry::client::save_client(registry_config, &registry_client) {
            Ok(()) => {
                report.imported += 1;
            }
            Err(e) => {
                report.errors.push(format!(
                    "Failed to import TM3 #{} ({} {}): {}",
                    tm3.id, tm3.surname, tm3.forename, e
                ));
            }
        }
    }

    Ok(report)
}
