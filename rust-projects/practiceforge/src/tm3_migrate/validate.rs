//! Migration validation — compare TM3 client cache against the registry
//! to verify completeness and detect mismatches.

use anyhow::Result;
use std::path::PathBuf;

use crate::registry::config::RegistryConfig;
use crate::registry::types::RegistryClient;
use crate::tm3_clients::{self, TM3Client};

/// A field-level mismatch between TM3 and registry data.
#[derive(Debug, Clone)]
pub struct FieldMismatch {
    pub client_id: String,
    pub tm3_id: u64,
    pub field: String,
    pub tm3_value: String,
    pub registry_value: String,
}

impl std::fmt::Display for FieldMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (TM3 #{}): {} differs — TM3='{}' vs registry='{}'",
            self.client_id, self.tm3_id, self.field, self.tm3_value, self.registry_value
        )
    }
}

/// Result of a migration validation.
#[derive(Debug, Clone, Default)]
pub struct ValidationReport {
    pub total_tm3: usize,
    pub total_registry: usize,
    /// Clients in TM3 but not in the registry.
    pub missing: Vec<MissingClient>,
    /// Clients in registry but not in TM3 (registry-only).
    pub extra: Vec<String>,
    /// Field-level mismatches between TM3 and registry.
    pub mismatches: Vec<FieldMismatch>,
    /// Clients missing documents (have TM3 ID but empty correspondence/).
    pub missing_documents: Vec<String>,
}

/// A client present in TM3 but not found in the registry.
#[derive(Debug, Clone)]
pub struct MissingClient {
    pub tm3_id: u64,
    pub name: String,
}

impl std::fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "Validation: {} TM3 clients, {} registry clients",
            self.total_tm3, self.total_registry
        )?;
        writeln!(
            f,
            "  Missing from registry: {}",
            self.missing.len()
        )?;
        writeln!(
            f,
            "  Extra in registry (not in TM3): {}",
            self.extra.len()
        )?;
        writeln!(
            f,
            "  Field mismatches: {}",
            self.mismatches.len()
        )?;
        write!(
            f,
            "  Missing documents: {}",
            self.missing_documents.len()
        )
    }
}

/// Compare a TM3Client against a RegistryClient and return field mismatches.
pub fn compare_fields(tm3: &TM3Client, registry: &RegistryClient) -> Vec<FieldMismatch> {
    let mut mismatches = Vec::new();
    let tm3_id = tm3.id;
    let client_id = registry.client_id.clone();

    // Compare name
    let tm3_name = format!("{}, {}", tm3.surname, tm3.forename);
    if !names_match(&tm3_name, &registry.name) {
        mismatches.push(FieldMismatch {
            client_id: client_id.clone(),
            tm3_id,
            field: "name".to_string(),
            tm3_value: tm3_name,
            registry_value: registry.name.clone(),
        });
    }

    // Compare email
    let tm3_email = tm3.email.as_deref().unwrap_or("").to_lowercase();
    let reg_email = registry.email.as_deref().unwrap_or("").to_lowercase();
    if !tm3_email.is_empty() && !reg_email.is_empty() && tm3_email != reg_email {
        mismatches.push(FieldMismatch {
            client_id: client_id.clone(),
            tm3_id,
            field: "email".to_string(),
            tm3_value: tm3_email,
            registry_value: reg_email,
        });
    }

    // Compare phone
    let tm3_phone = normalise_phone(tm3.number.as_deref().unwrap_or(""));
    let reg_phone = normalise_phone(registry.phone.as_deref().unwrap_or(""));
    if !tm3_phone.is_empty() && !reg_phone.is_empty() && tm3_phone != reg_phone {
        mismatches.push(FieldMismatch {
            client_id: client_id.clone(),
            tm3_id,
            field: "phone".to_string(),
            tm3_value: tm3.number.as_deref().unwrap_or("").to_string(),
            registry_value: registry.phone.as_deref().unwrap_or("").to_string(),
        });
    }

    mismatches
}

/// Check if two name strings refer to the same person.
/// Handles "Surname, Forename" format with case-insensitive comparison.
fn names_match(a: &str, b: &str) -> bool {
    let normalise = |s: &str| -> String {
        s.to_lowercase()
            .replace("  ", " ")
            .trim()
            .to_string()
    };
    normalise(a) == normalise(b)
}

/// Strip non-digit characters from a phone number for comparison.
fn normalise_phone(phone: &str) -> String {
    phone.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// Run the full validation: compare TM3 cache against registry.
pub fn validate(
    registry_config: &RegistryConfig,
    _clinical_root: &PathBuf,
) -> Result<ValidationReport> {
    let mut report = ValidationReport::default();

    // Load TM3 cache
    let tm3_clients = tm3_clients::load_cache().unwrap_or_default();
    report.total_tm3 = tm3_clients.len();

    // Load registry clients
    let registry_clients = crate::registry::list_clients(registry_config)?;
    report.total_registry = registry_clients.len();

    // Build lookup maps
    let registry_by_tm3_id: std::collections::HashMap<u64, &RegistryClient> = registry_clients
        .iter()
        .filter_map(|c| c.tm3_id.map(|id| (id, c)))
        .collect();

    let tm3_by_id: std::collections::HashMap<u64, &TM3Client> =
        tm3_clients.iter().map(|c| (c.id, c)).collect();

    // Find missing clients (in TM3 but not registry)
    for tm3 in &tm3_clients {
        if !registry_by_tm3_id.contains_key(&tm3.id) {
            report.missing.push(MissingClient {
                tm3_id: tm3.id,
                name: format!("{}, {}", tm3.surname, tm3.forename),
            });
        }
    }

    // Find extra clients (in registry with TM3 ID but not in TM3 cache)
    for reg in &registry_clients {
        if let Some(tm3_id) = reg.tm3_id {
            if !tm3_by_id.contains_key(&tm3_id) {
                report.extra.push(format!(
                    "{} (TM3 #{})",
                    reg.client_id, tm3_id
                ));
            }
        }
    }

    // Compare fields for matched clients
    for tm3 in &tm3_clients {
        if let Some(reg) = registry_by_tm3_id.get(&tm3.id) {
            let field_mismatches = compare_fields(tm3, reg);
            report.mismatches.extend(field_mismatches);
        }
    }

    // Check for missing documents
    for reg in &registry_clients {
        if reg.tm3_id.is_some() {
            let correspondence_dir = registry_config
                .client_dir(&reg.client_id)
                .join("correspondence");
            let has_docs = correspondence_dir.exists()
                && std::fs::read_dir(&correspondence_dir)
                    .map(|entries| entries.count() > 0)
                    .unwrap_or(false);

            if !has_docs {
                report
                    .missing_documents
                    .push(reg.client_id.clone());
            }
        }
    }

    Ok(report)
}
