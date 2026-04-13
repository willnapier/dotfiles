use anyhow::{bail, Context, Result};
use std::path::PathBuf;

/// Client directory layout.
///
/// Route A: de-identified notes, `private/` directory, `<ID>.md` naming.
/// Route C: real names throughout, `admin/` for operational files, `notes.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Legacy personal system — private/ directory, de-identification.
    RouteA,
    /// Product direction — flat structure, real names, no de-identification.
    RouteC,
}

/// Detect which layout a client uses based on directory structure.
///
/// If `private/` exists → Route A. Otherwise → Route C.
pub fn detect_layout(id: &str) -> Layout {
    if client_dir(id).join("private").exists() {
        Layout::RouteA
    } else {
        Layout::RouteC
    }
}

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

/// Private subdirectory for a client (Route A only).
pub fn private_dir(id: &str) -> PathBuf {
    client_dir(id).join("private")
}

/// Admin subdirectory for a client (Route C only).
pub fn admin_dir(id: &str) -> PathBuf {
    client_dir(id).join("admin")
}

/// Path to a client's identity.yaml — auto-detects layout.
///
/// Route A: ~/Clinical/clients/<id>/private/identity.yaml
/// Route C: ~/Clinical/clients/<id>/identity.yaml
pub fn identity_path(id: &str) -> PathBuf {
    match detect_layout(id) {
        Layout::RouteA => private_dir(id).join("identity.yaml"),
        Layout::RouteC => client_dir(id).join("identity.yaml"),
    }
}

/// Path to a client's main notes file — auto-detects layout.
///
/// Route A: ~/Clinical/clients/<id>/<id>.md
/// Route C: ~/Clinical/clients/<id>/notes.md
pub fn notes_path(id: &str) -> PathBuf {
    match detect_layout(id) {
        Layout::RouteA => client_dir(id).join(format!("{}.md", id)),
        Layout::RouteC => client_dir(id).join("notes.md"),
    }
}

/// Path to the identity template: ~/Clinical/PRIVATE-FILE-TEMPLATE.yaml
pub fn template_path() -> PathBuf {
    clinical_root().join("PRIVATE-FILE-TEMPLATE.yaml")
}

/// Path to the Route C identity template: ~/Clinical/IDENTITY-TEMPLATE.yaml
pub fn identity_template_path() -> PathBuf {
    clinical_root().join("IDENTITY-TEMPLATE.yaml")
}

/// Path to the drafts directory for a client — auto-detects layout.
///
/// Route A: ~/Clinical/clients/<id>/drafts/
/// Route C: ~/Clinical/clients/<id>/admin/drafts/
pub fn drafts_dir(id: &str) -> PathBuf {
    match detect_layout(id) {
        Layout::RouteA => client_dir(id).join("drafts"),
        Layout::RouteC => admin_dir(id).join("drafts"),
    }
}

/// Correspondence directory (Route C only): ~/Clinical/clients/<id>/correspondence/
pub fn correspondence_dir(id: &str) -> PathBuf {
    client_dir(id).join("correspondence")
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
    }

    #[test]
    fn test_route_c_paths() {
        // Route C: no private/ dir, so detect_layout returns RouteC
        // (in test env, private/ won't exist for a non-existent client)
        let tmp = tempfile::TempDir::new().unwrap();
        let client = tmp.path().join("clients").join("TEST99");
        std::fs::create_dir_all(&client).unwrap();

        // With CLINICAL_ROOT override
        std::env::set_var("CLINICAL_ROOT", tmp.path());

        assert_eq!(detect_layout("TEST99"), Layout::RouteC);
        assert!(notes_path("TEST99").ends_with("TEST99/notes.md"));
        assert!(identity_path("TEST99").ends_with("TEST99/identity.yaml"));
        assert!(drafts_dir("TEST99").ends_with("TEST99/admin/drafts"));

        std::env::remove_var("CLINICAL_ROOT");
    }

    #[test]
    fn test_route_a_paths() {
        let tmp = tempfile::TempDir::new().unwrap();
        let client = tmp.path().join("clients").join("TEST88");
        let private = client.join("private");
        std::fs::create_dir_all(&private).unwrap();

        std::env::set_var("CLINICAL_ROOT", tmp.path());

        assert_eq!(detect_layout("TEST88"), Layout::RouteA);
        assert!(notes_path("TEST88").ends_with("TEST88/TEST88.md"));
        assert!(identity_path("TEST88").ends_with("TEST88/private/identity.yaml"));
        assert!(drafts_dir("TEST88").ends_with("TEST88/drafts"));

        std::env::remove_var("CLINICAL_ROOT");
    }
}
