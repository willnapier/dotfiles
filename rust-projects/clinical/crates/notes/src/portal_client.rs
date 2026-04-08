//! Portal client subcommands: `share`, `status`, `revoke`, `changes`.
//!
//! These were originally in the `clinical-portal` binary alongside the HTTP
//! server. Pulling them into the `clinical` notes binary unifies the laptop
//! workflow under a single entry point. The portal binary now only contains
//! `serve` (the Fly.io HTTP server).
//!
//! All HTTP calls use `reqwest::blocking` so that the notes binary stays
//! synchronous and does not need a tokio runtime.

use anyhow::{anyhow, bail, Result};
use clinical_core::{client as core_client, identity as core_identity};
use std::path::PathBuf;

const DEFAULT_PORTAL_URL: &str = "https://clinical-portal.fly.dev";
const STATE_FILE: &str = "/tmp/clinical-last-build.json";

/// Upload a PDF and email a secure link to the recipient.
///
/// Resolves the PDF and client ID from the last `clinical-letter-build`
/// state file when not given explicitly. Recipient email/name come from
/// the client's identity.yaml unless overridden.
#[allow(clippy::too_many_arguments)]
pub fn share(
    client_id: Option<String>,
    pdf: Option<String>,
    to: Option<String>,
    name: Option<String>,
    expiry_days: u32,
    portal_url: Option<String>,
    dry_run: bool,
) -> Result<()> {
    let portal_url = portal_url.unwrap_or_else(|| DEFAULT_PORTAL_URL.to_string());

    let pdf_path = if let Some(p) = pdf {
        PathBuf::from(p)
    } else {
        let state = load_state_file()?;
        let p = state["pdf"]
            .as_str()
            .ok_or_else(|| anyhow!("Invalid state file: no pdf field"))?;
        PathBuf::from(p)
    };

    if !pdf_path.exists() {
        bail!("PDF not found: {}", pdf_path.display());
    }

    let resolved_client_id = if let Some(id) = client_id {
        id
    } else if let Ok(state) = load_state_file() {
        state["client_id"]
            .as_str()
            .unwrap_or("unknown")
            .to_string()
    } else {
        "unknown".to_string()
    };

    let (resolved_to, resolved_name) =
        resolve_recipient(&resolved_client_id, to.as_deref(), name.as_deref())?;

    println!("Client:    {resolved_client_id}");
    println!("PDF:       {}", pdf_path.display());
    println!("To:        {resolved_to}");
    println!("Name:      {resolved_name}");
    println!("Expiry:    {expiry_days} days");
    println!("Portal:    {portal_url}");
    println!();

    if dry_run {
        println!("Dry run — no upload or email sent.");
        return Ok(());
    }

    let client = reqwest::blocking::Client::new();
    let pdf_bytes = std::fs::read(&pdf_path)?;
    let filename = pdf_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let form = reqwest::blocking::multipart::Form::new()
        .text("recipient_email", resolved_to.clone())
        .text("recipient_name", resolved_name.clone())
        .text("client_id", resolved_client_id.clone())
        .text("expiry_days", expiry_days.to_string())
        .part(
            "file",
            reqwest::blocking::multipart::Part::bytes(pdf_bytes).file_name(filename),
        );

    let resp = client
        .post(format!("{portal_url}/api/upload"))
        .multipart(form)
        .send()?;

    if !resp.status().is_success() {
        let body = resp.text()?;
        bail!("Upload failed: {body}");
    }

    let result: serde_json::Value = resp.json()?;
    let link = result["link"].as_str().unwrap_or("unknown");

    println!("Secure link sent to {resolved_to}");
    println!("Link: {link}");
    println!("Expires in {expiry_days} days");

    Ok(())
}

/// List all shared documents and their status.
pub fn status(portal_url: Option<String>) -> Result<()> {
    let portal_url = portal_url.unwrap_or_else(|| DEFAULT_PORTAL_URL.to_string());
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(format!("{portal_url}/api/status"))
        .send()?;

    if !resp.status().is_success() {
        let body = resp.text()?;
        bail!("Status failed: {body}");
    }

    let docs: Vec<serde_json::Value> = resp.json()?;

    if docs.is_empty() {
        println!("No documents shared yet.");
        return Ok(());
    }

    println!(
        "{:<40} {:<25} {:<10} {:<8} {}",
        "TOKEN", "RECIPIENT", "STATUS", "VIEWS", "EXPIRES"
    );
    println!("{}", "-".repeat(95));
    for doc in &docs {
        let token = doc["token"].as_str().unwrap_or("");
        let recipient = doc["recipient_email"].as_str().unwrap_or("");
        let revoked = doc["revoked"].as_bool().unwrap_or(false);
        let access_count = doc["access_count"].as_i64().unwrap_or(0);
        let expires = doc["expires_at"].as_str().unwrap_or("");
        let expires_short = &expires[..10.min(expires.len())];

        let status = if revoked {
            "REVOKED"
        } else if expires < chrono::Utc::now().to_rfc3339().as_str() {
            "EXPIRED"
        } else {
            "ACTIVE"
        };

        println!(
            "{:<40} {:<25} {:<10} {:<8} {}",
            &token[..8.min(token.len())],
            &recipient[..25.min(recipient.len())],
            status,
            access_count,
            expires_short
        );
    }
    println!("\n{} document(s)", docs.len());

    Ok(())
}

/// Revoke a previously shared document by token.
pub fn revoke(token: String, portal_url: Option<String>) -> Result<()> {
    let portal_url = portal_url.unwrap_or_else(|| DEFAULT_PORTAL_URL.to_string());
    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(format!("{portal_url}/api/revoke/{token}"))
        .send()?;

    if !resp.status().is_success() {
        let body = resp.text()?;
        bail!("Revoke failed: {body}");
    }

    let result: serde_json::Value = resp.json()?;
    let status = result["status"].as_str().unwrap_or("unknown");
    println!("Status: {status}");
    if status == "revoked" {
        println!(
            "Access to this document has been revoked. The recipient can no longer view it."
        );
    }

    Ok(())
}

/// List files changed in `~/Clinical` within the last N days, via `fd`.
pub fn changes(days: u32) -> Result<()> {
    let clinical_dir = core_client::clinical_root().join("clients");
    let clinical_dir_str = clinical_dir.display().to_string();

    let output = std::process::Command::new("fd")
        .args([
            ".",
            &clinical_dir_str,
            "--type",
            "f",
            "--changed-within",
            &format!("{}d", days),
            "--exclude",
            ".DS_Store",
        ])
        .output()?;

    let files = String::from_utf8_lossy(&output.stdout);
    if files.trim().is_empty() {
        println!("No changes in the last {} days.", days);
        return Ok(());
    }

    println!("Files changed in last {} days:\n", days);
    let prefix = format!("{}/", clinical_dir_str);
    for line in files.lines() {
        let rel = line.strip_prefix(&prefix).unwrap_or(line);
        println!("  {}", rel);
    }

    Ok(())
}

// ---------- internals ----------

fn load_state_file() -> Result<serde_json::Value> {
    let path = std::path::Path::new(STATE_FILE);
    if !path.exists() {
        bail!(
            "No --pdf specified and no recent build found.\n\
             Run clinical-letter-build first, or pass --pdf explicitly."
        );
    }
    let contents = std::fs::read_to_string(path)?;
    let state: serde_json::Value = serde_json::from_str(&contents)?;
    Ok(state)
}

/// Read recipient email and name from identity.yaml, with CLI overrides.
fn resolve_recipient(
    client_id: &str,
    to_override: Option<&str>,
    name_override: Option<&str>,
) -> Result<(String, String)> {
    if let Some(to) = to_override {
        let name = name_override.unwrap_or("Colleague").to_string();
        return Ok((to.to_string(), name));
    }

    let identity_path = core_client::identity_path(client_id);

    if !identity_path.exists() {
        bail!(
            "No --to specified and no identity.yaml found at {}\n\
             Pass --to explicitly, or ensure referrer.email is set in identity.yaml.",
            identity_path.display()
        );
    }

    let identity = core_identity::load_identity(&identity_path)?;

    let email = identity
        .referrer
        .email
        .as_deref()
        .filter(|s| !s.is_empty() && *s != "null")
        .ok_or_else(|| {
            anyhow!(
                "No referrer.email in {}\n\
                 Pass --to explicitly, or add referrer.email to identity.yaml.",
                identity_path.display()
            )
        })?
        .to_string();

    let name = name_override
        .map(|s| s.to_string())
        .or_else(|| {
            identity
                .referrer
                .name
                .as_deref()
                .filter(|s| !s.is_empty() && *s != "null")
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "Colleague".to_string());

    Ok((email, name))
}
