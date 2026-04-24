use std::fs;
use std::path::PathBuf;

use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};

use crate::config::{Config, Enrolment};
use crate::page;
use crate::shamir;

fn bequest_dir() -> PathBuf {
    dirs::home_dir()
        .expect("could not find home directory")
        .join(".bequest")
}

/// Run the enrollment workflow: split vault key, create per-trustee bundles.
pub fn run(k: u8, n: u8) -> Result<()> {
    let mut config = Config::load()?;

    ensure!(
        !config.trustees.is_empty(),
        "no trustees configured — add trustees to ~/.bequest/config.toml first"
    );
    ensure!(
        config.trustees.len() == n as usize,
        "config has {} trustees but you specified -n {}. These must match.",
        config.trustees.len(),
        n
    );
    ensure!(k >= 2, "threshold must be at least 2 for meaningful sharing");
    ensure!(n >= k, "shares must be >= threshold");

    // Check vault identity key exists
    let identity_path = bequest_dir().join("identity.key");
    ensure!(
        identity_path.exists(),
        "no identity key — run `bequest vault init` first"
    );

    // Check vault.age exists (vault must be sealed)
    let vault_archive = bequest_dir().join("vault.age");
    ensure!(
        vault_archive.exists(),
        "vault.age not found — run `bequest vault seal` first"
    );

    // Read identity key and split
    let identity = fs::read_to_string(&identity_path).context("reading identity key")?;
    let shares = shamir::split(identity.as_bytes(), k, n)?;

    // Generate reconstruction page
    let html = page::generate_reconstruction_html();

    // Create bundles directory
    let bundles_dir = bequest_dir().join("bundles");
    if bundles_dir.exists() {
        fs::remove_dir_all(&bundles_dir).context("removing old bundles")?;
    }

    for (i, trustee) in config.trustees.iter().enumerate() {
        let slug = trustee
            .name
            .to_lowercase()
            .replace(|c: char| !c.is_alphanumeric(), "-");
        let bundle_dir = bundles_dir.join(&slug);
        fs::create_dir_all(&bundle_dir)
            .with_context(|| format!("creating bundle dir for {}", trustee.name))?;

        // Write share
        let encoded = STANDARD.encode(&shares[i]);
        let share_content = format!(
            "Share {} of {} (threshold: {}): {}",
            i + 1,
            n,
            k,
            encoded
        );
        fs::write(bundle_dir.join("share.txt"), &share_content)
            .context("writing share")?;

        // Write reconstruction page
        fs::write(bundle_dir.join("reconstruction.html"), &html)
            .context("writing reconstruction.html")?;

        // Copy vault.age
        fs::copy(&vault_archive, bundle_dir.join("vault.age"))
            .context("copying vault.age")?;

        // Write instructions
        let instructions = format!(
            r#"# Bequest — Instructions for {}

You are receiving this because William has named you as a trustee
for his digital estate.

## What you have

- **share.txt** — your portion of the encryption key (1 of {})
- **reconstruction.html** — a tool to combine shares (works offline)
- **vault.age** — the encrypted vault containing estate information

## What to do when the time comes

1. Contact the other trustees. You need at least {} shares to proceed.
2. Open **reconstruction.html** in any web browser (works offline).
3. Each trustee pastes their share into a separate box.
4. Click **Reconstruct Secret** — this produces the decryption key.
5. Save the decryption key to a file called `key.txt`.
6. Decrypt the vault:
   - Install `age` if needed: https://github.com/FiloSottile/age
   - Run: `age -d -i key.txt vault.age > vault.tar`
   - Run: `tar xf vault.tar`
7. Read the vault contents — they contain account details and instructions.

## Important

- Keep share.txt safe. Do not share it until needed.
- The reconstruction page works entirely offline — no data leaves your browser.
- You do NOT need to do anything with these files unless the disclosure
  notification is triggered.

## Other trustees

{}
"#,
            trustee.name,
            n,
            k,
            config
                .trustees
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, t)| format!("- {} ({})", t.name, t.email))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        fs::write(bundle_dir.join("instructions.md"), &instructions)
            .context("writing instructions")?;

        eprintln!(
            "Bundle created: {}/",
            bundle_dir.strip_prefix(dirs::home_dir().unwrap()).unwrap_or(&bundle_dir).display()
        );
    }

    // Record enrolment in config
    let now = humantime::format_rfc3339_seconds(std::time::SystemTime::now()).to_string();
    config.enrolment = Some(Enrolment {
        threshold: k,
        shares: n,
        enrolled_at: now,
    });
    config.save()?;

    eprintln!();
    eprintln!("Enrolment complete. {} bundles created in {}/", n, bundles_dir.display());
    eprintln!();
    eprintln!("Next: send each trustee their bundle (email, USB, etc.).");
    if config.trustees.iter().any(|t| !t.email.is_empty()) {
        eprintln!("      Or run `bequest enrol --send` to email bundles via msmtp (pizauth XOAUTH2).");
    }

    Ok(())
}

/// Send pre-created bundles to trustees via msmtp (pizauth XOAUTH2).
pub fn send_bundles() -> Result<()> {
    let config = Config::load()?;
    let bundles_dir = bequest_dir().join("bundles");

    ensure!(
        bundles_dir.exists(),
        "no bundles found — run `bequest enrol` first"
    );

    let from = config.settings.from_email.as_deref().unwrap_or_default();
    if from.is_empty() {
        bail!("from_email not set in config — add it to ~/.bequest/config.toml [settings]");
    }

    for trustee in &config.trustees {
        let slug = trustee
            .name
            .to_lowercase()
            .replace(|c: char| !c.is_alphanumeric(), "-");
        let bundle_dir = bundles_dir.join(&slug);

        if !bundle_dir.exists() {
            eprintln!("WARNING: no bundle for {} — skipping", trustee.name);
            continue;
        }

        let share = fs::read_to_string(bundle_dir.join("share.txt"))
            .context("reading share")?;
        let instructions = fs::read_to_string(bundle_dir.join("instructions.md"))
            .context("reading instructions")?;

        // Compose email. Plain-text body with share inline; two files
        // (reconstruction.html + vault.age) become MIME attachments.
        let body = format!(
            "{}\n\n---\n\nYour share (keep this safe):\n{}\n\n---\n\n\
             The reconstruction page and vault archive are attached.\n\
             You do not need to do anything with these files unless you\n\
             receive a disclosure notification.\n",
            instructions, share
        );

        let html_path = bundle_dir.join("reconstruction.html");
        let vault_path = bundle_dir.join("vault.age");
        let mut attachments = Vec::new();
        if html_path.exists() {
            attachments.push(html_path.as_path());
        }
        if vault_path.exists() {
            attachments.push(vault_path.as_path());
        }

        let subject = format!("Bequest — Trustee Bundle for {}", trustee.name);
        match crate::send::send_mail(from, &trustee.email, &subject, &body, &attachments) {
            Ok(()) => eprintln!("Sent bundle to {} <{}>", trustee.name, trustee.email),
            Err(e) => eprintln!(
                "FAILED sending to {} <{}>: {e}",
                trustee.name, trustee.email
            ),
        }
    }

    Ok(())
}
