use std::fs;
use std::path::PathBuf;

use anyhow::{bail, ensure, Context, Result};

use crate::config::Config;

fn bequest_dir() -> PathBuf {
    dirs::home_dir()
        .expect("could not find home directory")
        .join(".bequest")
}

/// Send disclosure notification to all trustees.
pub fn run(dry_run: bool) -> Result<()> {
    let config = Config::load()?;

    ensure!(
        !config.trustees.is_empty(),
        "no trustees configured"
    );

    let enrolment = config
        .enrolment
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no enrolment recorded — run `bequest enrol` first"))?;

    let from = config.settings.from_email.as_deref().unwrap_or_default();
    if from.is_empty() {
        bail!("from_email not set in config");
    }

    let trustee_list: String = config
        .trustees
        .iter()
        .map(|t| format!("  - {} ({})", t.name, t.email))
        .collect::<Vec<_>>()
        .join("\n");

    for trustee in &config.trustees {
        let body = format!(
            r#"Dear {},

This is an automated message from the Bequest digital estate system.

The dead man's switch has been triggered — no activity has been detected
for the configured threshold period.

You were enrolled as a trustee on {}. You should have a bundle containing:
  - Your Shamir share (share.txt)
  - A reconstruction page (reconstruction.html)
  - The encrypted vault (vault.age)

To access the vault, you need {} of {} trustees to combine their shares.

The other trustees are:
{}

Please contact them to coordinate reconstruction. Open reconstruction.html
in any browser, paste the required number of shares, and follow the
instructions in your bundle.

This message was sent automatically.
"#,
            trustee.name,
            enrolment.enrolled_at,
            enrolment.threshold,
            enrolment.shares,
            trustee_list,
        );

        if dry_run {
            println!("=== To: {} <{}> ===", trustee.name, trustee.email);
            println!("{}", body);
            println!();
            continue;
        }

        match crate::send::send_mail(
            from,
            &trustee.email,
            "Bequest — Disclosure Activated",
            &body,
            &[],
        ) {
            Ok(()) => eprintln!("Disclosure sent to {} <{}>", trustee.name, trustee.email),
            Err(e) => eprintln!(
                "FAILED sending to {} <{}>: {e}",
                trustee.name, trustee.email
            ),
        }
    }

    // Log the disclosure event
    if !dry_run {
        let log_path = bequest_dir().join("disclosure.log");
        let now = humantime::format_rfc3339_seconds(std::time::SystemTime::now()).to_string();
        let entry = format!("{} — disclosure sent to {} trustees\n", now, config.trustees.len());
        let mut log = fs::read_to_string(&log_path).unwrap_or_default();
        log.push_str(&entry);
        fs::write(&log_path, &log).context("writing disclosure log")?;
        eprintln!("Disclosure logged to {}", log_path.display());
    }

    Ok(())
}

/// Send warning email to William during grace period.
pub fn warn(days_elapsed: u64, grace_remaining: u64) -> Result<()> {
    let config = Config::load()?;

    if config.settings.warning_emails.is_empty() {
        eprintln!("No warning_emails configured — skipping warning notification.");
        return Ok(());
    }

    let from = config.settings.from_email.as_deref().unwrap_or_default();
    if from.is_empty() {
        eprintln!("No from_email configured — cannot send warning.");
        return Ok(());
    }

    let body = format!(
        r#"Bequest dead man's switch WARNING

No activity detected for {} days. Grace period: {} days remaining.

If you are reading this, record a heartbeat:
  bequest heartbeat ping

If no activity is detected within {} days, the disclosure
will be triggered automatically.
"#,
        days_elapsed, grace_remaining, grace_remaining,
    );

    for email in &config.settings.warning_emails {
        match crate::send::send_mail(
            from,
            email,
            "Bequest — Dead Man's Switch Warning",
            &body,
            &[],
        ) {
            Ok(()) => eprintln!("Warning sent to {email}"),
            Err(e) => eprintln!("WARNING: failed to send to {email}: {e}"),
        }
    }

    Ok(())
}
