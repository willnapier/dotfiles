use anyhow::{bail, Context, Result};
use std::path::PathBuf;

/// Root of the clinical directory tree.
///
/// Checks `CLINICAL_ROOT` env var first, then falls back to `~/Clinical`.
/// This allows Leigh (Windows/Dropbox) to point at her Dropbox path.
pub fn clinical_root() -> PathBuf {
    if let Ok(root) = std::env::var("CLINICAL_ROOT") {
        PathBuf::from(root)
    } else {
        dirs::home_dir()
            .expect("Could not find home directory")
            .join("Clinical")
    }
}

/// Directory containing all client folders: ~/Clinical/clients/
pub fn clients_dir() -> PathBuf {
    clinical_root().join("clients")
}

/// Directory for a specific client: ~/Clinical/clients/<id>/
pub fn client_dir(id: &str) -> PathBuf {
    clients_dir().join(id)
}

/// Private subdirectory for a client: ~/Clinical/clients/<id>/private/
pub fn private_dir(id: &str) -> PathBuf {
    client_dir(id).join("private")
}

/// Path to a client's identity.yaml: ~/Clinical/clients/<id>/private/identity.yaml
pub fn identity_path(id: &str) -> PathBuf {
    private_dir(id).join("identity.yaml")
}

/// Path to a client's main notes file: ~/Clinical/clients/<id>/<id>.md
pub fn notes_path(id: &str) -> PathBuf {
    client_dir(id).join(format!("{}.md", id))
}

/// Path to the identity template: ~/Clinical/PRIVATE-FILE-TEMPLATE.yaml
pub fn template_path() -> PathBuf {
    clinical_root().join("PRIVATE-FILE-TEMPLATE.yaml")
}

/// Path to the drafts directory for a client: ~/Clinical/clients/<id>/drafts/
pub fn drafts_dir(id: &str) -> PathBuf {
    client_dir(id).join("drafts")
}

/// List all client IDs (directory names under ~/Clinical/clients/).
pub fn list_client_ids() -> Result<Vec<String>> {
    let dir = clients_dir();
    if !dir.exists() {
        bail!("Clients directory not found: {}", dir.display());
    }

    let mut ids: Vec<String> = std::fs::read_dir(&dir)
        .with_context(|| format!("Failed to read: {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();

    ids.sort();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_construction() {
        let root = clinical_root();
        assert!(root.ends_with("Clinical"));

        let cdir = client_dir("EB88");
        assert!(cdir.ends_with("Clinical/clients/EB88"));

        let pdir = private_dir("EB88");
        assert!(pdir.ends_with("Clinical/clients/EB88/private"));

        let ipath = identity_path("EB88");
        assert!(ipath.ends_with("Clinical/clients/EB88/private/identity.yaml"));

        let npath = notes_path("EB88");
        assert!(npath.ends_with("Clinical/clients/EB88/EB88.md"));
    }
}
