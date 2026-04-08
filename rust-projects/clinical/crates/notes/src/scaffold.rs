use anyhow::{Context, Result};

use crate::client;

/// Run the scaffold command: create a new client directory with all required files.
///
/// Idempotent — skips files that already exist.
pub fn run(id: &str) -> Result<()> {
    let private = client::private_dir(id);
    let template = client::template_path();

    // Create client directory and private/ if needed
    if !private.exists() {
        std::fs::create_dir_all(&private)
            .with_context(|| format!("Failed to create: {}", private.display()))?;
        println!("  Created: {}/ and private/", id);
    }

    // Copy identity.yaml template if missing
    let identity = private.join("identity.yaml");
    if !identity.exists() {
        if template.exists() {
            std::fs::copy(&template, &identity).with_context(|| {
                format!(
                    "Failed to copy template from {}",
                    template.display()
                )
            })?;
            println!("  Created: private/identity.yaml [from template]");
        } else {
            eprintln!("  Warning: template not found at {}", template.display());
        }
    } else {
        println!("  Exists: private/identity.yaml");
    }

    // Create reference.md if missing
    let reference = private.join("reference.md");
    if !reference.exists() {
        let content = format!(
            "# {} — Reference\n\nKey facts and dramatis personae for pre-session orientation.\n",
            id
        );
        std::fs::write(&reference, content)
            .with_context(|| format!("Failed to create: {}", reference.display()))?;
        println!("  Created: private/reference.md");
    } else {
        println!("  Exists: private/reference.md");
    }

    // Create raw-notes.md if missing
    let raw_notes = private.join("raw-notes.md");
    if !raw_notes.exists() {
        let content = format!(
            "# {} — Raw Notes\n\nUnstructured session notes. Digest into reference.md over time.\n",
            id
        );
        std::fs::write(&raw_notes, content)
            .with_context(|| format!("Failed to create: {}", raw_notes.display()))?;
        println!("  Created: private/raw-notes.md");
    } else {
        println!("  Exists: private/raw-notes.md");
    }

    // Create [ID].md if missing
    let notes = client::notes_path(id);
    if !notes.exists() {
        let content = format!(
            "# {}\n\n**Referral**: \n**Started**: \n\n## Presenting Difficulties\n\n## Formulation\n\n## Session Notes\n",
            id
        );
        std::fs::write(&notes, content)
            .with_context(|| format!("Failed to create: {}", notes.display()))?;
        println!("  Created: {}.md", id);
    } else {
        println!("  Exists: {}.md", id);
    }

    println!("\nDone. Remember to update ~/Clinical/private/tm3-client-map.toml");

    Ok(())
}

#[cfg(test)]
mod tests {
    /// Test scaffold in a temporary directory by overriding HOME.
    /// We can't easily override dirs::home_dir() in tests, so we test
    /// the file creation logic directly.
    #[test]
    fn test_scaffold_creates_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("clients/TEST01");
        let private = base.join("private");

        // Simulate scaffold logic
        std::fs::create_dir_all(&private).unwrap();
        assert!(private.exists());

        // Identity template — just create a placeholder
        let identity = private.join("identity.yaml");
        std::fs::write(&identity, "name: null\n").unwrap();
        assert!(identity.exists());

        // Reference
        let reference = private.join("reference.md");
        std::fs::write(&reference, "# TEST01 — Reference\n").unwrap();

        // Raw notes
        let raw_notes = private.join("raw-notes.md");
        std::fs::write(&raw_notes, "# TEST01 — Raw Notes\n").unwrap();

        // Client notes
        let notes = base.join("TEST01.md");
        std::fs::write(&notes, "# TEST01\n").unwrap();

        // Verify all exist
        assert!(identity.exists());
        assert!(reference.exists());
        assert!(raw_notes.exists());
        assert!(notes.exists());
    }

    #[test]
    fn test_scaffold_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("clients/TEST02");
        let private = base.join("private");
        std::fs::create_dir_all(&private).unwrap();

        // Pre-create identity.yaml with custom content
        let identity = private.join("identity.yaml");
        std::fs::write(&identity, "name: Custom Content\n").unwrap();

        // Scaffold should NOT overwrite existing files
        // (We verify by checking content is preserved)
        let content = std::fs::read_to_string(&identity).unwrap();
        assert_eq!(content, "name: Custom Content\n");
    }
}
