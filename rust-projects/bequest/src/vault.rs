use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;

use crate::shamir;

fn bequest_dir() -> PathBuf {
    dirs::home_dir()
        .expect("could not find home directory")
        .join(".bequest")
}

fn vault_dir() -> PathBuf {
    bequest_dir().join("vault")
}

fn vault_archive() -> PathBuf {
    bequest_dir().join("vault.age")
}

fn identity_file() -> PathBuf {
    bequest_dir().join("identity.key")
}

fn check_age() -> Result<()> {
    Command::new("age")
        .arg("--version")
        .output()
        .context("age is not installed — run: sudo pacman -S age")?;
    Ok(())
}

/// Extract the public key (recipient) from the identity file.
fn read_recipient() -> Result<String> {
    let path = identity_file();
    ensure!(
        path.exists(),
        "no identity file at {} — run `bequest vault init` first",
        path.display()
    );
    let content = fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    // Public key is in a comment line: # public key: age1...
    for line in content.lines() {
        if let Some(key) = line.strip_prefix("# public key: ") {
            return Ok(key.trim().to_string());
        }
    }
    bail!("could not find public key in {}", path.display());
}

/// Read the full identity file content (this is what gets Shamir-split).
fn read_identity() -> Result<String> {
    let path = identity_file();
    ensure!(
        path.exists(),
        "no identity file at {} — run `bequest vault init` first",
        path.display()
    );
    fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
}

pub fn init() -> Result<()> {
    let base = bequest_dir();
    let vault = vault_dir();
    let id_path = identity_file();

    if id_path.exists() {
        bail!(
            "vault already initialised at {} — identity key exists",
            base.display()
        );
    }

    // Create directory structure
    fs::create_dir_all(vault.join("vault-export")).context("creating vault-export/")?;
    fs::create_dir_all(vault.join("legal")).context("creating legal/")?;
    fs::create_dir_all(vault.join("personal")).context("creating personal/")?;

    // Write README for trustees
    fs::write(
        vault.join("README.md"),
        r#"# Bequest Vault

This vault contains William's digital estate information.

## Contents

- **vault-export/** — password manager exports
- **legal/** — will location, solicitor details, financial accounts
- **personal/** — letters, memorabilia guide, important photos

## How you got here

Someone reconstructed the vault key using Shamir's Secret Sharing
and decrypted this archive. If you're reading this, follow the playbook
below to handle accounts and subscriptions in an orderly way.
"#,
    )
    .context("writing README.md")?;

    // Generate age identity (key pair)
    check_age()?;
    let keygen = Command::new("age-keygen")
        .args(["-o"])
        .arg(&id_path)
        .output()
        .context("running age-keygen")?;

    ensure!(
        keygen.status.success(),
        "age-keygen failed: {}",
        String::from_utf8_lossy(&keygen.stderr)
    );

    // Restrict permissions
    fs::set_permissions(&id_path, fs::Permissions::from_mode(0o600))
        .context("setting identity key permissions")?;

    let recipient = read_recipient()?;

    eprintln!("Vault initialised at {}", base.display());
    eprintln!("Identity key: {} (mode 0600)", id_path.display());
    eprintln!("Public key:   {}", recipient);
    eprintln!();
    eprintln!("Next steps:");
    eprintln!("  1. Add files to {}", vault.display());
    eprintln!("  2. bequest vault seal     — encrypt and remove plaintext");
    eprintln!("  3. bequest vault split    — generate Shamir shares of the key");

    Ok(())
}

pub fn seal() -> Result<()> {
    check_age()?;
    let vault = vault_dir();
    let archive = vault_archive();
    let recipient = read_recipient()?;

    ensure!(
        vault.exists(),
        "vault directory not found at {} — nothing to seal (already sealed?)",
        vault.display()
    );

    // tar the vault directory
    let tar_output = Command::new("tar")
        .args(["-cf", "-", "-C"])
        .arg(bequest_dir())
        .arg("vault")
        .output()
        .context("running tar")?;

    ensure!(
        tar_output.status.success(),
        "tar failed: {}",
        String::from_utf8_lossy(&tar_output.stderr)
    );

    // Encrypt with age using recipient public key (no interactive prompt)
    if archive.exists() {
        fs::remove_file(&archive).ok();
    }

    let mut age_child = Command::new("age")
        .args(["-r", &recipient, "-o"])
        .arg(&archive)
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("starting age")?;

    {
        let stdin = age_child.stdin.as_mut().unwrap();
        stdin
            .write_all(&tar_output.stdout)
            .context("writing to age stdin")?;
    }

    let age_result = age_child.wait_with_output().context("waiting for age")?;
    ensure!(
        age_result.status.success(),
        "age encryption failed: {}",
        String::from_utf8_lossy(&age_result.stderr)
    );

    // Remove plaintext vault directory
    fs::remove_dir_all(&vault).context("removing plaintext vault directory")?;

    eprintln!("Vault sealed → {}", archive.display());
    eprintln!("Plaintext removed.");

    Ok(())
}

pub fn open() -> Result<()> {
    check_age()?;
    let vault = vault_dir();
    let archive = vault_archive();
    let id_path = identity_file();

    ensure!(
        archive.exists(),
        "no sealed vault at {} — run `bequest vault seal` first",
        archive.display()
    );
    ensure!(
        id_path.exists(),
        "no identity key at {} — cannot decrypt",
        id_path.display()
    );

    if vault.exists() {
        bail!(
            "vault directory already exists at {} — already open? Remove it or seal first.",
            vault.display()
        );
    }

    // Decrypt with age using identity file
    let age_output = Command::new("age")
        .args(["-d", "-i"])
        .arg(&id_path)
        .arg(&archive)
        .output()
        .context("running age decrypt")?;

    ensure!(
        age_output.status.success(),
        "age decryption failed: {}",
        String::from_utf8_lossy(&age_output.stderr)
    );

    // Untar into bequest directory
    let mut tar_child = Command::new("tar")
        .args(["-xf", "-", "-C"])
        .arg(bequest_dir())
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("starting tar")?;

    {
        let stdin = tar_child.stdin.as_mut().unwrap();
        stdin
            .write_all(&age_output.stdout)
            .context("writing to tar stdin")?;
    }

    let tar_result = tar_child.wait_with_output().context("waiting for tar")?;
    ensure!(
        tar_result.status.success(),
        "tar extraction failed: {}",
        String::from_utf8_lossy(&tar_result.stderr)
    );

    eprintln!("Vault opened at {}", vault.display());
    eprintln!("Edit contents, then run `bequest vault seal` when done.");

    Ok(())
}

pub fn status() -> Result<()> {
    let base = bequest_dir();
    let vault = vault_dir();
    let archive = vault_archive();
    let id = identity_file();

    if !base.exists() {
        println!("No vault found. Run `bequest vault init` to create one.");
        return Ok(());
    }

    let vault_exists = vault.exists();
    let archive_exists = archive.exists();
    let id_exists = id.exists();

    let state = match (vault_exists, archive_exists) {
        (true, false) => "OPEN (plaintext available, not yet sealed)",
        (false, true) => "SEALED (encrypted archive only)",
        (true, true) => "MIXED (both plaintext and archive exist — seal or remove one)",
        (false, false) => "EMPTY (initialised but no content)",
    };

    println!("Vault: {}", base.display());
    println!("State: {}", state);
    println!(
        "Identity key: {}",
        if id_exists { "present" } else { "MISSING" }
    );

    if id_exists {
        if let Ok(recipient) = read_recipient() {
            println!("Public key: {}", recipient);
        }
    }

    if vault_exists {
        println!();
        println!("Contents:");
        print_tree(&vault, &vault, 0)?;
    }

    if archive_exists {
        let meta = fs::metadata(&archive)?;
        println!();
        println!("Archive: {} ({} bytes)", archive.display(), meta.len());
    }

    Ok(())
}

fn print_tree(_base: &Path, dir: &Path, depth: usize) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let indent = "  ".repeat(depth + 1);
        if path.is_dir() {
            println!("{}{}/", indent, entry.file_name().to_string_lossy());
            print_tree(_base, &path, depth + 1)?;
        } else {
            let meta = fs::metadata(&path)?;
            println!(
                "{}{} ({} bytes)",
                indent,
                entry.file_name().to_string_lossy(),
                meta.len()
            );
        }
    }
    Ok(())
}

/// Pull the "Estate" folder from Vaultwarden and update the vault.
///
/// Flow: bw unlock → find Estate folder → export items → open vault → write → re-seal.
pub fn update() -> Result<()> {
    check_age()?;
    let id_path = identity_file();
    ensure!(id_path.exists(), "no identity key — run `bequest vault init` first");

    // Check bw is installed
    let bw_ver = bw_cmd()
        .arg("--version")
        .output()
        .context("bw (bitwarden-cli) not installed — run: sudo pacman -S bitwarden-cli")?;
    ensure!(bw_ver.status.success(), "bw --version failed");

    // Check login status
    let status_out = bw_cmd().args(["status"]).output()?;
    let status_str = String::from_utf8_lossy(&status_out.stdout);
    if status_str.contains("\"unauthenticated\"") {
        bail!("Not logged in. Run: NODE_TLS_REJECT_UNAUTHORIZED=0 bw login");
    }

    // Get session token — either from env or by unlocking
    let session = get_bw_session()?;

    // List folders to find "Estate"
    let folders_out = bw_cmd()
        .args(["list", "folders", "--session", &session])
        .output()
        .context("listing folders")?;
    ensure!(folders_out.status.success(), "bw list folders failed");

    let folders_json = String::from_utf8_lossy(&folders_out.stdout);
    let folders: Vec<BwFolder> =
        serde_json::from_str(&folders_json).context("parsing folders JSON")?;

    let estate_folder = folders
        .iter()
        .find(|f| f.name.eq_ignore_ascii_case("estate"))
        .ok_or_else(|| {
            let names: Vec<_> = folders.iter().map(|f| f.name.as_str()).collect();
            anyhow::anyhow!(
                "No 'Estate' folder found in Vaultwarden. Found: {:?}. \
                 Create a folder called 'Estate' in the web vault first.",
                names
            )
        })?;

    eprintln!("Found folder: {} ({})", estate_folder.name, estate_folder.id);

    // List items in the Estate folder
    let items_out = bw_cmd()
        .args([
            "list",
            "items",
            "--folderid",
            &estate_folder.id,
            "--session",
            &session,
        ])
        .output()
        .context("listing items")?;
    ensure!(items_out.status.success(), "bw list items failed");

    let items_json = String::from_utf8_lossy(&items_out.stdout);
    let items: Vec<serde_json::Value> =
        serde_json::from_str(&items_json).context("parsing items JSON")?;

    eprintln!("{} items in Estate folder", items.len());

    if items.is_empty() {
        eprintln!("Warning: Estate folder is empty. Nothing to export.");
    }

    // Determine vault state and handle accordingly
    let vault = vault_dir();
    let archive = vault_archive();
    let was_sealed = !vault.exists() && archive.exists();

    // Open vault if sealed
    if was_sealed {
        open()?;
    } else if !vault.exists() {
        bail!("No vault — run `bequest vault init` first");
    }

    // Write the export
    let export_dir = vault.join("vault-export");
    fs::create_dir_all(&export_dir).context("creating vault-export/")?;
    let export_path = export_dir.join("vaultwarden-estate.json");
    let pretty = serde_json::to_string_pretty(&items).context("formatting JSON")?;
    fs::write(&export_path, &pretty)
        .with_context(|| format!("writing {}", export_path.display()))?;

    eprintln!(
        "Wrote {} items to {}",
        items.len(),
        export_path.display()
    );

    // Re-seal if it was sealed before
    if was_sealed {
        seal()?;
    } else {
        eprintln!("Vault is open — run `bequest vault seal` when ready.");
    }

    // Lock bw session
    let _ = bw_cmd()
        .args(["lock"])
        .output();

    Ok(())
}

fn bw_cmd() -> Command {
    let mut cmd = Command::new("bw");
    cmd.env("NODE_TLS_REJECT_UNAUTHORIZED", "0");
    cmd.stderr(std::process::Stdio::piped());
    cmd
}

fn get_bw_session() -> Result<String> {
    // Check if BW_SESSION is already set
    if let Ok(s) = std::env::var("BW_SESSION") {
        if !s.is_empty() {
            return Ok(s);
        }
    }

    // Prompt for master password and unlock
    eprint!("Vaultwarden master password: ");
    std::io::stderr().flush()?;
    let mut password = String::new();
    std::io::stdin().read_line(&mut password)?;
    let password = password.trim().to_string();

    let unlock = bw_cmd()
        .args(["unlock", "--raw", &password])
        .output()
        .context("running bw unlock")?;

    ensure!(
        unlock.status.success(),
        "bw unlock failed: {}",
        String::from_utf8_lossy(&unlock.stderr)
    );

    let session = String::from_utf8_lossy(&unlock.stdout).trim().to_string();
    ensure!(!session.is_empty(), "bw unlock returned empty session");
    Ok(session)
}

#[derive(Deserialize)]
struct BwFolder {
    id: String,
    name: String,
}

pub fn split_key(k: u8, n: u8, output: Option<PathBuf>) -> Result<()> {
    let identity = read_identity()?;
    let shares = shamir::split(identity.as_bytes(), k, n)?;

    if let Some(dir) = output {
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        for (i, share) in shares.iter().enumerate() {
            let encoded = STANDARD.encode(share);
            let label = format!(
                "Share {} of {} (threshold: {}): {}\n",
                i + 1,
                n,
                k,
                encoded
            );
            let path = dir.join(format!("share-{}.txt", i + 1));
            fs::write(&path, &label)
                .with_context(|| format!("writing {}", path.display()))?;
            eprintln!("Wrote {}", path.display());
        }
    } else {
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        for (i, share) in shares.iter().enumerate() {
            let encoded = STANDARD.encode(share);
            writeln!(
                out,
                "Share {} of {} (threshold: {}): {}",
                i + 1,
                n,
                k,
                encoded
            )?;
        }
    }

    Ok(())
}
