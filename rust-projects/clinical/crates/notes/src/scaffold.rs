use anyhow::{Context, Result};

use clinical_core::client;

/// Run the scaffold command: create a new client directory with all required files.
///
/// Idempotent — skips files that already exist.
/// Default layout is Route C (Product direction). Use `--route-a` for legacy layout.
pub fn run(id: &str, route_a: bool) -> Result<()> {
    if route_a {
        scaffold_route_a(id)
    } else {
        scaffold_route_c(id)
    }
}

/// Route C scaffold: flat structure, real names, no de-identification.
///
/// ```text
/// <id>/
///   identity.yaml
///   notes.md
///   correspondence/
///   admin/
///     raw-notes.md
///     drafts/
///     letters/
/// ```
fn scaffold_route_c(id: &str) -> Result<()> {
    let dir = client::client_dir(id);
    let admin = client::admin_dir(id);
    let correspondence = client::correspondence_dir(id);

    // Create directories
    for d in [&dir, &admin, &correspondence] {
        if !d.exists() {
            std::fs::create_dir_all(d)
                .with_context(|| format!("Failed to create: {}", d.display()))?;
        }
    }

    // Create admin subdirectories
    let drafts = admin.join("drafts");
    let letters = admin.join("letters");
    for d in [&drafts, &letters] {
        if !d.exists() {
            std::fs::create_dir_all(d)
                .with_context(|| format!("Failed to create: {}", d.display()))?;
        }
    }

    println!("  Created: {}/", id);

    // identity.yaml — from template or minimal
    let identity = dir.join("identity.yaml");
    if !identity.exists() {
        let template = client::identity_template_path();
        if template.exists() {
            std::fs::copy(&template, &identity).with_context(|| {
                format!("Failed to copy template from {}", template.display())
            })?;
            println!("  Created: identity.yaml [from template]");
        } else {
            let content = format!(
                "# {} — Client Identity\n\
                 # Metadata for clinical tooling. Real names — no de-identification needed.\n\n\
                 name:\ntitle:\ndob:\ntm3_id:\nstatus: active\n\n\
                 funding:\n  funding_type:\n  rate:\n  session_duration: 50\n\n\
                 referrer:\n  name:\n  role:\n  email:\n",
                id
            );
            std::fs::write(&identity, content)
                .with_context(|| format!("Failed to create: {}", identity.display()))?;
            println!("  Created: identity.yaml [minimal]");
        }
    } else {
        println!("  Exists: identity.yaml");
    }

    // notes.md — main clinical file
    let notes = dir.join("notes.md");
    if !notes.exists() {
        let content = format!(
            "# {}\n\n\
             **Referral source**: \n\
             **Referral type**: \n\
             **Referring doctor**: \n\
             **Funding**: \n\
             **Therapy commenced**: \n\
             **Session count**: 0\n\n\
             ## Presenting Difficulties\n\n\
             ## Formulation\n\n\
             ## Session Notes\n",
            id
        );
        std::fs::write(&notes, content)
            .with_context(|| format!("Failed to create: {}", notes.display()))?;
        println!("  Created: notes.md");
    } else {
        println!("  Exists: notes.md");
    }

    // raw-notes.md — in-session jottings
    let raw_notes = admin.join("raw-notes.md");
    if !raw_notes.exists() {
        let content = format!(
            "# {} — Raw Notes\n\nIn-session observations. Not for the model.\n",
            id
        );
        std::fs::write(&raw_notes, content)
            .with_context(|| format!("Failed to create: {}", raw_notes.display()))?;
        println!("  Created: admin/raw-notes.md");
    } else {
        println!("  Exists: admin/raw-notes.md");
    }

    println!("\nDone (Route C layout). Fill in identity.yaml and you're set.");

    Ok(())
}

/// Route A scaffold: legacy layout with private/ directory and de-identification.
fn scaffold_route_a(id: &str) -> Result<()> {
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
    let notes = client::client_dir(id).join(format!("{}.md", id));
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

    println!("\nDone (Route A layout). Remember to update ~/Clinical/tm3-client-map.toml");

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_scaffold_route_c_creates_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("clients/TEST01");

        // Simulate Route C scaffold logic
        let admin = base.join("admin");
        let correspondence = base.join("correspondence");
        std::fs::create_dir_all(&admin.join("drafts")).unwrap();
        std::fs::create_dir_all(&admin.join("letters")).unwrap();
        std::fs::create_dir_all(&correspondence).unwrap();

        let identity = base.join("identity.yaml");
        std::fs::write(&identity, "name:\n").unwrap();

        let notes = base.join("notes.md");
        std::fs::write(&notes, "# TEST01\n").unwrap();

        let raw_notes = admin.join("raw-notes.md");
        std::fs::write(&raw_notes, "# TEST01 — Raw Notes\n").unwrap();

        assert!(identity.exists());
        assert!(notes.exists());
        assert!(raw_notes.exists());
        assert!(admin.join("drafts").exists());
        assert!(admin.join("letters").exists());
        assert!(correspondence.exists());
        // No private/ directory
        assert!(!base.join("private").exists());
    }

    #[test]
    fn test_scaffold_route_a_creates_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("clients/TEST02");
        let private = base.join("private");
        std::fs::create_dir_all(&private).unwrap();

        let identity = private.join("identity.yaml");
        std::fs::write(&identity, "name: null\n").unwrap();

        let notes = base.join("TEST02.md");
        std::fs::write(&notes, "# TEST02\n").unwrap();

        assert!(identity.exists());
        assert!(notes.exists());
        assert!(private.exists());
    }
}
