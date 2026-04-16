//! Email sending via direct SMTP (lettre).
//!
//! Reads [email] config from config.toml. Password from OS keychain.
//! No himalaya or Resend dependency — sends directly via the
//! practitioner's own SMTP server.

use anyhow::{bail, Context, Result};
use lettre::message::{header::ContentType, Attachment, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct EmailConfig {
    pub smtp_server: String,
    pub smtp_port: u16,
    pub username: String,
    pub from_name: String,
    pub from_email: String,
    pub signature: String,
}

/// Load email config from ~/.config/practiceforge/config.toml [email] section.
pub fn load_email_config() -> Result<EmailConfig> {
    let config_path = dirs::config_dir()
        .map(|d| d.join("practiceforge/config.toml"))
        .unwrap_or_default();

    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Could not read {}", config_path.display()))?;

    let table: toml::Table = content
        .parse()
        .context("Failed to parse config.toml")?;

    let email = table
        .get("email")
        .and_then(|v| v.as_table())
        .ok_or_else(|| anyhow::anyhow!(
            "No [email] section in config.toml.\nRun: practiceforge email init"
        ))?;

    Ok(EmailConfig {
        smtp_server: email
            .get("smtp_server")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("email.smtp_server not set"))?
            .to_string(),
        smtp_port: email
            .get("smtp_port")
            .and_then(|v| v.as_integer())
            .unwrap_or(465) as u16,
        username: email
            .get("username")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("email.username not set"))?
            .to_string(),
        from_name: email
            .get("from_name")
            .and_then(|v| v.as_str())
            .unwrap_or("Practitioner")
            .to_string(),
        from_email: email
            .get("from_email")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("email.from_email not set"))?
            .to_string(),
        signature: email
            .get("signature")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

/// Load email password from OS keychain.
fn load_password(username: &str) -> Result<String> {
    let output = if cfg!(target_os = "macos") {
        Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                "clinical-email",
                "-a",
                username,
                "-w",
            ])
            .output()
            .context("Failed to read macOS keychain")?
    } else {
        Command::new("secret-tool")
            .args([
                "lookup",
                "service",
                "clinical-email",
                "account",
                username,
            ])
            .output()
            .context("Failed to read secret-service")?
    };

    if !output.status.success() {
        bail!(
            "No password found in keychain for '{}'.\n\
             Store it with:\n  \
             security add-generic-password -s clinical-email -a '{}' -w '<password>'",
            username,
            username
        );
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Store email password in OS keychain.
fn store_password(username: &str, password: &str) -> Result<()> {
    if cfg!(target_os = "macos") {
        // Delete existing entry (ignore error if not found)
        Command::new("security")
            .args([
                "delete-generic-password",
                "-s",
                "clinical-email",
                "-a",
                username,
            ])
            .output()
            .ok();

        let status = Command::new("security")
            .args([
                "add-generic-password",
                "-s",
                "clinical-email",
                "-a",
                username,
                "-w",
                password,
                "-U",
            ])
            .status()
            .context("Failed to store in macOS keychain")?;

        if !status.success() {
            bail!("Failed to store password in keychain");
        }
    } else {
        let mut child = Command::new("secret-tool")
            .args([
                "store",
                "--label",
                "Clinical email",
                "service",
                "clinical-email",
                "account",
                username,
            ])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .context("Failed to run secret-tool")?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(password.as_bytes())?;
        }
        child.wait()?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Send
// ---------------------------------------------------------------------------

/// Send an email with optional PDF attachment.
pub fn send_email(
    config: &EmailConfig,
    to_email: &str,
    to_name: &str,
    subject: &str,
    body: &str,
    attachment_path: Option<&Path>,
    cc: Option<&[String]>,
) -> Result<()> {
    let password = load_password(&config.username)?;

    // Build the message
    let from = format!("{} <{}>", config.from_name, config.from_email)
        .parse()
        .context("Invalid from address")?;

    let to = if to_name.is_empty() {
        to_email
            .parse()
            .context("Invalid recipient address")?
    } else {
        format!("{} <{}>", to_name, to_email)
            .parse()
            .context("Invalid recipient address")?
    };

    let mut builder = Message::builder()
        .from(from)
        .to(to)
        .subject(subject);

    if let Some(cc_addrs) = cc {
        for addr in cc_addrs {
            if let Ok(parsed) = addr.trim().parse() {
                builder = builder.cc(parsed);
            }
        }
    }

    let message = if let Some(pdf_path) = attachment_path {
        // Multipart: text body + PDF attachment
        let pdf_bytes = std::fs::read(pdf_path)
            .with_context(|| format!("Could not read {}", pdf_path.display()))?;

        let filename = pdf_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let attachment = Attachment::new(filename)
            .body(pdf_bytes, ContentType::parse("application/pdf").unwrap());

        let text_body = SinglePart::builder()
            .header(lettre::message::header::ContentType::TEXT_PLAIN)
            .body(body.to_string());

        builder
            .multipart(MultiPart::mixed().singlepart(text_body).singlepart(attachment))
            .context("Failed to build email message")?
    } else {
        builder
            .body(body.to_string())
            .context("Failed to build email message")?
    };

    // Connect and send
    let creds = Credentials::new(config.username.clone(), password);

    let transport = if config.smtp_port == 465 {
        // Implicit TLS (port 465)
        SmtpTransport::relay(&config.smtp_server)
            .context("Failed to create SMTP transport")?
            .port(config.smtp_port)
            .credentials(creds)
            .build()
    } else {
        // STARTTLS (port 587)
        SmtpTransport::starttls_relay(&config.smtp_server)
            .context("Failed to create SMTP transport")?
            .port(config.smtp_port)
            .credentials(creds)
            .build()
    };

    transport
        .send(&message)
        .context("Failed to send email")?;

    Ok(())
}

/// Generate a random 6-digit verification code.
fn generate_otp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    format!("{:06}", seed % 1_000_000)
}

/// Send a verification code to the given email address.
/// Returns the code that was sent.
pub fn send_verification_code(config: &EmailConfig) -> Result<String> {
    let code = generate_otp();

    send_email(
        config,
        &config.from_email,
        &config.from_name,
        "Clinical Product — Email Verification",
        &format!(
            "Your verification code is: {}\n\n\
             Enter this code in the setup wizard to confirm your email configuration.\n\n\
             This code was sent from {} via {}:{}.\n\
             If you didn't request this, you can ignore it.",
            code, config.from_email, config.smtp_server, config.smtp_port
        ),
        None,
        None,
    )?;

    Ok(code)
}

/// Send a test email to the configured from address (non-interactive).
pub fn send_test(config: &EmailConfig) -> Result<()> {
    eprintln!("Sending test email to {}...", config.from_email);

    send_email(
        config,
        &config.from_email,
        &config.from_name,
        "Clinical Product — Email Test",
        &format!(
            "This is a test email from practiceforge.\n\n\
             If you received this, your email configuration is working correctly.\n\n\
             Server: {}:{}\n\
             From: {} <{}>\n",
            config.smtp_server, config.smtp_port, config.from_name, config.from_email
        ),
        None,
        None,
    )?;

    println!("✓ Test email sent to {}", config.from_email);
    Ok(())
}

// ---------------------------------------------------------------------------
// Setup wizard
// ---------------------------------------------------------------------------

fn prompt(label: &str, default: Option<&str>) -> Result<String> {
    if let Some(d) = default {
        eprint!("{} [{}]: ", label, d);
    } else {
        eprint!("{}: ", label);
    }
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();

    if input.is_empty() {
        match default {
            Some(d) => Ok(d.to_string()),
            None => bail!("Required field"),
        }
    } else {
        Ok(input.to_string())
    }
}

fn prompt_password(label: &str) -> Result<String> {
    eprint!("{}: ", label);
    io::stderr().flush()?;

    // Read password (no echo would be ideal but stdin works for now)
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

/// Auto-detect SMTP server from email domain.
fn detect_smtp(email: &str) -> Option<(String, u16)> {
    let domain = email.split('@').nth(1)?;
    let domain_lower = domain.to_lowercase();

    // Well-known providers
    if domain_lower.contains("gmail") || domain_lower.contains("google") {
        return Some(("smtp.gmail.com".to_string(), 465));
    }
    if domain_lower.contains("outlook") || domain_lower.contains("hotmail") || domain_lower.contains("office365") {
        return Some(("smtp.office365.com".to_string(), 587));
    }

    // cPanel convention: mail.domain.com
    Some((format!("mail.{}", domain), 465))
}

/// Interactive setup wizard for email configuration.
pub fn init_config() -> Result<()> {
    println!("=== Email Setup ===\n");

    let email = prompt("Practice email address", None)?;

    let (detected_server, detected_port) = detect_smtp(&email)
        .unwrap_or(("mail.example.com".to_string(), 465));

    let smtp_server = prompt("SMTP server", Some(&detected_server))?;
    let smtp_port: u16 = prompt("SMTP port", Some(&detected_port.to_string()))?
        .parse()
        .context("Invalid port number")?;

    let password = prompt_password("Email password")?;
    if password.is_empty() {
        bail!("Password is required");
    }

    // Derive display name from email prefix
    let default_name = email
        .split('@')
        .next()
        .unwrap_or("")
        .replace('.', " ")
        .split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().to_string() + c.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    let from_name = prompt("Display name", Some(&default_name))?;

    let signature = prompt(
        "Signature (single line, or press Enter for default)",
        Some(&format!(
            "{}\n\nChange of Harley Street\n37 Gloucester Place, London W1U 8JA",
            from_name
        )),
    )?;

    // Store password temporarily in keychain (needed for verification send)
    store_password(&email, &password)?;

    // Build a temporary config for verification
    let temp_config = EmailConfig {
        smtp_server: smtp_server.clone(),
        smtp_port,
        username: email.clone(),
        from_name: from_name.clone(),
        from_email: email.clone(),
        signature: signature.clone(),
    };

    // --- OTP Verification ---
    eprintln!("\nSending verification code to {}...", email);
    let code = match send_verification_code(&temp_config) {
        Ok(c) => {
            eprintln!("✓ Code sent. Check your inbox.");
            c
        }
        Err(e) => {
            eprintln!("\n✗ Failed to send verification code: {}", e);
            eprintln!("Check your SMTP server, port, and password, then try again.");
            bail!("Email verification failed — config not saved.");
        }
    };

    // Prompt for code (up to 3 attempts)
    let mut verified = false;
    for attempt in 1..=3 {
        let entered = prompt(
            &format!("Enter the 6-digit code (attempt {}/3)", attempt),
            None,
        )?;
        if entered.trim() == code {
            verified = true;
            break;
        }
        eprintln!("Incorrect code.");
    }

    if !verified {
        bail!("Verification failed after 3 attempts — config not saved.");
    }

    eprintln!("\n✓ Email verified.");

    // --- Save config (only after verification) ---
    let config_path = dirs::config_dir()
        .map(|d| d.join("practiceforge/config.toml"))
        .unwrap_or_default();

    let existing = std::fs::read_to_string(&config_path).unwrap_or_default();

    // Remove existing [email] section if present
    if existing.contains("[email]") {
        let mut new_content = String::new();
        let mut in_email_section = false;
        for line in existing.lines() {
            if line.trim() == "[email]" {
                in_email_section = true;
                continue;
            }
            if in_email_section && line.starts_with('[') && line.contains(']') {
                in_email_section = false;
            }
            if !in_email_section {
                new_content.push_str(line);
                new_content.push('\n');
            }
        }
        std::fs::write(&config_path, &new_content)?;
    }

    // Append [email] section
    let email_section = format!(
        "\n[email]\nsmtp_server = \"{}\"\nsmtp_port = {}\nusername = \"{}\"\nfrom_name = \"{}\"\nfrom_email = \"{}\"\nsignature = \"\"\"{}\"\"\"\n",
        smtp_server, smtp_port, email, from_name, email,
        signature.replace('\n', "\n")
    );

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&config_path)?;
    file.write_all(email_section.as_bytes())?;

    println!("\n✓ Email setup complete.");
    println!("  From: {} <{}>", from_name, email);
    println!("  Server: {}:{}", smtp_server, smtp_port);
    println!("  Config: {}", config_path.display());
    println!("  Password: stored in keychain (service: clinical-email)");

    Ok(())
}
