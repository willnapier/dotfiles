use anyhow::Result;

use super::config::RegistryConfig;
use super::repo;

/// High-level sync: pull from remote, push local changes.
/// Returns a human-readable summary.
pub fn sync(config: &RegistryConfig) -> Result<String> {
    let repo_path = &config.local_path;

    if !repo_path.join(".git").exists() {
        anyhow::bail!(
            "Registry not initialised at {}. Run `clinical-product registry init` first.",
            repo_path.display()
        );
    }

    let mut messages = Vec::new();

    // Step 1: Pull from remote
    match repo::pull(repo_path)? {
        repo::PullResult::UpToDate => {
            messages.push("Remote: already up to date".to_string());
        }
        repo::PullResult::Updated { summary } => {
            messages.push(format!("Remote: pulled updates\n{}", summary));
        }
        repo::PullResult::NoRemote => {
            messages.push("Remote: none configured (local-only mode)".to_string());
        }
    }

    // Step 2: Check for uncommitted local changes and commit them
    let status = repo::status(repo_path)?;
    if !status.clean {
        repo::add_and_commit(
            repo_path,
            &["."],
            &format!(
                "Auto-sync: {} uncommitted changes",
                status.uncommitted_count
            ),
        )?;
        messages.push(format!(
            "Local: committed {} changes",
            status.uncommitted_count
        ));
    } else {
        messages.push("Local: clean".to_string());
    }

    // Step 3: Push to remote
    if status.has_remote {
        repo::push(repo_path)?;
        messages.push("Push: done".to_string());
    }

    Ok(messages.join("\n"))
}

/// Commit a specific file to the registry and push.
/// Used by other modules after writing letters, attendance, etc.
pub fn commit_file(config: &RegistryConfig, relative_path: &str, message: &str) -> Result<()> {
    let repo_path = &config.local_path;

    if !repo_path.join(".git").exists() {
        anyhow::bail!("Registry not initialised");
    }

    repo::add_and_commit(repo_path, &[relative_path], message)?;

    if repo::has_remote(repo_path)? {
        repo::push(repo_path)?;
    }

    Ok(())
}

/// Check if a sync is needed based on last sync time.
/// Returns true if more than sync_interval_minutes have passed.
pub fn sync_due(config: &RegistryConfig) -> bool {
    let marker = config.local_path.join(".last-sync");
    if !marker.exists() {
        return true;
    }

    let Ok(metadata) = std::fs::metadata(&marker) else {
        return true;
    };

    let Ok(modified) = metadata.modified() else {
        return true;
    };

    let elapsed = modified.elapsed().unwrap_or_default();
    elapsed.as_secs() > (config.sync_interval_minutes as u64 * 60)
}

/// Update the last-sync marker.
pub fn mark_synced(config: &RegistryConfig) -> Result<()> {
    let marker = config.local_path.join(".last-sync");
    std::fs::write(&marker, chrono::Local::now().to_rfc3339())?;
    Ok(())
}

/// Display a formatted status report of the registry.
pub fn show_status(config: &RegistryConfig) -> Result<()> {
    let repo_path = &config.local_path;

    if !repo_path.join(".git").exists() {
        println!("Registry: not initialised");
        println!("  Path: {}", repo_path.display());
        println!("  Run `clinical-product registry init` to create.");
        return Ok(());
    }

    let status = repo::status(repo_path)?;
    let (active, discharged) = super::client::count_by_status(config)?;

    println!("Registry: {}", if config.enabled { "enabled" } else { "disabled" });
    println!("  Path:        {}", repo_path.display());
    println!("  Branch:      {}", status.branch);
    println!("  Remote:      {}", if status.has_remote {
        &config.remote_url
    } else {
        "none (local-only)"
    });
    println!("  Clean:       {}", if status.clean { "yes" } else { "no" });
    if !status.clean {
        println!("  Uncommitted: {}", status.uncommitted_count);
    }
    if status.has_remote {
        println!("  Ahead:       {}", status.ahead);
        println!("  Behind:      {}", status.behind);
    }
    println!("  Clients:     {} active, {} discharged", active, discharged);
    println!("  Auto-sync:   {}", if config.auto_sync { "on" } else { "off" });
    println!("  Sync every:  {} min", config.sync_interval_minutes);
    println!("  Practitioner: {}", if config.practitioner_id.is_empty() {
        "(not set)"
    } else {
        &config.practitioner_id
    });

    if sync_due(config) {
        println!("  ⚠ Sync is overdue");
    }

    Ok(())
}
