//! Tantivy index management — build, update, and staleness checks.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tantivy::schema::*;
use tantivy::{doc, Index, IndexWriter};

use super::config::SearchConfig;

/// Build the Tantivy schema for client search.
pub fn build_schema() -> Schema {
    let mut builder = Schema::builder();

    // Stored + indexed fields
    builder.add_text_field("client_id", STRING | STORED);
    builder.add_text_field("name", TEXT | STORED);
    builder.add_text_field("funding_type", STRING | STORED);
    builder.add_text_field("status", STRING | STORED);

    // Full-text searchable fields
    builder.add_text_field("diagnosis", TEXT);
    builder.add_text_field("notes_content", TEXT);
    builder.add_text_field("correspondence_content", TEXT);

    builder.build()
}

/// Full rebuild of the search index from all client directories.
///
/// Scans `clinical_root/clients/` for client directories. If registry is
/// available it is used for metadata; otherwise identity.yaml is read
/// directly from each client directory.
pub fn build_index(config: &SearchConfig, clinical_root: &Path) -> Result<()> {
    let clients_dir = clinical_root.join("clients");
    if !clients_dir.exists() {
        anyhow::bail!(
            "Clients directory not found: {}",
            clients_dir.display()
        );
    }

    // Remove old index and recreate
    if config.index_path.exists() {
        std::fs::remove_dir_all(&config.index_path)
            .with_context(|| format!("Failed to remove old index at {}", config.index_path.display()))?;
    }
    std::fs::create_dir_all(&config.index_path)
        .with_context(|| format!("Failed to create index directory {}", config.index_path.display()))?;

    let schema = build_schema();
    let index = Index::create_in_dir(&config.index_path, schema.clone())
        .context("Failed to create Tantivy index")?;

    let mut writer: IndexWriter = index
        .writer(50_000_000) // 50 MB heap
        .context("Failed to create index writer")?;

    // Scan client directories
    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(&clients_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            !name.starts_with('.')
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    let mut indexed = 0u32;
    for entry in &entries {
        let client_id = entry.file_name().to_string_lossy().to_string();
        let client_dir = entry.path();

        if let Err(e) = index_client_dir(&schema, &mut writer, config, &client_id, &client_dir) {
            eprintln!("Warning: failed to index {}: {}", client_id, e);
            continue;
        }
        indexed += 1;
    }

    writer.commit().context("Failed to commit index")?;

    // Touch a marker file so we can check staleness
    touch_index_marker(&config.index_path)?;

    eprintln!("Indexed {} client(s).", indexed);
    Ok(())
}

/// Update the index for a single client (delete old docs, re-add).
pub fn update_client_index(
    config: &SearchConfig,
    client_id: &str,
    clinical_root: &Path,
) -> Result<()> {
    let client_dir = clinical_root.join("clients").join(client_id);
    if !client_dir.exists() {
        anyhow::bail!("Client directory not found: {}", client_dir.display());
    }

    if !config.index_path.exists() {
        // No index yet — do a full build instead
        return build_index(config, clinical_root);
    }

    let schema = build_schema();
    let index = Index::open_in_dir(&config.index_path)
        .context("Failed to open existing index")?;

    let mut writer: IndexWriter = index
        .writer(50_000_000)
        .context("Failed to create index writer")?;

    // Delete existing documents for this client
    let client_id_field = schema.get_field("client_id").unwrap();
    let term = tantivy::Term::from_field_text(client_id_field, client_id);
    writer.delete_term(term);

    // Re-index
    index_client_dir(&schema, &mut writer, config, client_id, &client_dir)?;

    writer.commit().context("Failed to commit index update")?;
    touch_index_marker(&config.index_path)?;

    Ok(())
}

/// Check whether the index is stale (older than the given threshold).
///
/// Returns `true` if the index needs a rebuild:
/// - Index does not exist
/// - Index marker file is older than `max_age`
pub fn is_index_stale(config: &SearchConfig, max_age: std::time::Duration) -> bool {
    let marker = config.index_path.join(".indexed_at");
    if !marker.exists() {
        return true;
    }

    match marker.metadata().and_then(|m| m.modified()) {
        Ok(modified) => {
            let age = std::time::SystemTime::now()
                .duration_since(modified)
                .unwrap_or(std::time::Duration::MAX);
            age > max_age
        }
        Err(_) => true,
    }
}

// --- internal helpers ---

/// Index a single client directory into the Tantivy writer.
fn index_client_dir(
    schema: &Schema,
    writer: &mut IndexWriter,
    config: &SearchConfig,
    client_id: &str,
    client_dir: &Path,
) -> Result<()> {
    let client_id_field = schema.get_field("client_id").unwrap();
    let name_field = schema.get_field("name").unwrap();
    let funding_type_field = schema.get_field("funding_type").unwrap();
    let status_field = schema.get_field("status").unwrap();
    let diagnosis_field = schema.get_field("diagnosis").unwrap();
    let notes_field = schema.get_field("notes_content").unwrap();
    let corr_field = schema.get_field("correspondence_content").unwrap();

    // Read identity.yaml for metadata
    let identity_path = client_dir.join("identity.yaml");
    let (name, funding_type, status, diagnosis) = if identity_path.exists() {
        parse_identity(&identity_path)?
    } else {
        (
            client_id.to_string(),
            String::new(),
            "active".to_string(),
            String::new(),
        )
    };

    // Read notes.md
    let notes_content = if config.include_notes {
        let notes_path = client_dir.join("notes.md");
        if notes_path.exists() {
            std::fs::read_to_string(&notes_path).unwrap_or_default()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // Read correspondence/ directory
    let corr_content = if config.include_correspondence {
        read_correspondence_dir(&client_dir.join("correspondence"))
    } else {
        String::new()
    };

    writer.add_document(doc!(
        client_id_field => client_id,
        name_field => name.as_str(),
        funding_type_field => funding_type.as_str(),
        status_field => status.as_str(),
        diagnosis_field => diagnosis.as_str(),
        notes_field => notes_content.as_str(),
        corr_field => corr_content.as_str(),
    ))?;

    Ok(())
}

/// Parse identity.yaml to extract searchable fields.
fn parse_identity(path: &Path) -> Result<(String, String, String, String)> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    let value: serde_yaml::Value = serde_yaml::from_str(&content)
        .with_context(|| format!("Failed to parse {}", path.display()))?;

    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let funding_type = value
        .get("funding")
        .and_then(|f| f.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let status = value
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("active")
        .to_string();

    let diagnosis = value
        .get("diagnosis")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok((name, funding_type, status, diagnosis))
}

/// Read all markdown files in a correspondence/ directory and concatenate them.
fn read_correspondence_dir(dir: &Path) -> String {
    if !dir.exists() {
        return String::new();
    }

    let mut content = String::new();
    let entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(entries) => entries.filter_map(|e| e.ok()).collect(),
        Err(_) => return String::new(),
    };

    for entry in entries {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "md" || ext == "txt") {
            if let Ok(text) = std::fs::read_to_string(&path) {
                content.push_str(&text);
                content.push('\n');
            }
        }
    }

    content
}

/// Touch the index marker file to record when the index was last built.
fn touch_index_marker(index_path: &Path) -> Result<()> {
    let marker = index_path.join(".indexed_at");
    std::fs::write(&marker, "")
        .with_context(|| format!("Failed to write index marker {}", marker.display()))?;
    Ok(())
}

/// Resolve the clients directory: prefer clinical_root from config.
pub fn resolve_clinical_root() -> PathBuf {
    crate::config::clinical_root()
}
