//! Cross-platform session cookie loading for browser automation tools.
//!
//! Checks three sources in order:
//! 1. Local OS keychain (macOS: security, Linux: secret-tool)
//! 2. Syncthing-shared cookie file (~//Assistants/shared/.tm3-cookies.json)
//! 3. SSH to Mac to extract from macOS keychain (requires SSH agent)
//!
//! This allows TM3 tools to work from any machine in the Tailnet.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StoredCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub expires: Option<f64>,
}

/// Load cookies for a service, trying multiple sources.
pub fn load_cookies(service: &str, account: &str) -> Result<Vec<StoredCookie>> {
    // 1. Try local keychain
    if let Ok(cookies) = load_from_local_keychain(service, account) {
        eprintln!("[cookies] Loaded from local keychain.");
        return Ok(cookies);
    }

    // 2. Try Syncthing-shared file
    let shared_path = shared_cookie_path(service);
    if shared_path.exists() {
        if let Ok(cookies) = load_from_file(&shared_path) {
            eprintln!("[cookies] Loaded from shared file: {}", shared_path.display());
            return Ok(cookies);
        }
    }

    // 3. Try SSH to Mac
    if let Ok(cookies) = load_via_ssh_mac(service, account) {
        eprintln!("[cookies] Loaded via SSH from Mac keychain.");
        // Cache locally for next time
        store_in_local_keychain(service, account, &cookies).ok();
        return Ok(cookies);
    }

    bail!(
        "No session cookies found for '{service}'.\n\
         Options:\n\
         - On Mac: log in via the browser tool, then run `tm3-cookie-sync`\n\
         - Or ensure SSH to Mac works: `ssh mac \"echo ok\"`"
    );
}

/// Store cookies in the local OS keychain.
pub fn store_in_local_keychain(
    service: &str,
    account: &str,
    cookies: &[StoredCookie],
) -> Result<()> {
    let json = serde_json::to_string(cookies)?;
    if cfg!(target_os = "macos") {
        // Delete existing, ignore error
        Command::new("security")
            .args([
                "delete-generic-password",
                "-s",
                service,
                "-a",
                account,
            ])
            .output()
            .ok();
        Command::new("security")
            .args([
                "add-generic-password",
                "-s",
                service,
                "-a",
                account,
                "-w",
                &json,
                "-U",
            ])
            .status()
            .context("Failed to store in macOS keychain")?;
    } else {
        let mut child = Command::new("secret-tool")
            .args([
                "store",
                "--label",
                &format!("{service} session"),
                "service",
                service,
                "account",
                account,
            ])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .context("Failed to run secret-tool")?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin.write_all(json.as_bytes())?;
        }
        child.wait()?;
    }
    Ok(())
}

fn load_from_local_keychain(service: &str, account: &str) -> Result<Vec<StoredCookie>> {
    let output = if cfg!(target_os = "macos") {
        Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                service,
                "-a",
                account,
                "-w",
            ])
            .output()
            .context("Failed to read macOS keychain")?
    } else {
        Command::new("secret-tool")
            .args(["lookup", "service", service, "account", account])
            .output()
            .context("Failed to read secret-service")?
    };

    if !output.status.success() {
        bail!("No entry found in local keychain");
    }

    let json = String::from_utf8(output.stdout)?.trim().to_string();
    let cookies: Vec<StoredCookie> = serde_json::from_str(&json)?;
    Ok(cookies)
}

fn load_from_file(path: &PathBuf) -> Result<Vec<StoredCookie>> {
    let json = std::fs::read_to_string(path)?;
    let cookies: Vec<StoredCookie> = serde_json::from_str(json.trim())?;
    Ok(cookies)
}

fn load_via_ssh_mac(service: &str, account: &str) -> Result<Vec<StoredCookie>> {
    let output = Command::new("ssh")
        .args([
            "-o",
            "ConnectTimeout=5",
            "mac",
            &format!(
                "security find-generic-password -s '{}' -a '{}' -w",
                service, account
            ),
        ])
        .output()
        .context("SSH to Mac failed")?;

    if !output.status.success() {
        bail!("Could not extract cookies from Mac keychain via SSH");
    }

    let json = String::from_utf8(output.stdout)?.trim().to_string();
    let cookies: Vec<StoredCookie> = serde_json::from_str(&json)?;
    Ok(cookies)
}

fn shared_cookie_path(service: &str) -> PathBuf {
    dirs::home_dir()
        .expect("cannot resolve home directory")
        .join("Assistants")
        .join("shared")
        .join(format!(".{}-cookies.json", service))
}
