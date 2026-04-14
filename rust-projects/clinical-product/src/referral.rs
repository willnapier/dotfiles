//! IMAP referral intake module.
//!
//! Watches a configured IMAP inbox for referral emails, extracts client
//! metadata via heuristic pattern matching, and proposes scaffolding a new
//! client directory. Auth via keychain (`secret-tool lookup service
//! "clinical-imap"` on Linux).
//!
//! Config lives in `~/.config/clinical-product/config.toml` under the
//! `[referral]` section.

use anyhow::{bail, Context, Result};
use imap::{ClientBuilder, Connection, Session};
use mail_parser::MessageParser;
use regex::Regex;
use serde::Deserialize;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::Command;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Subcommands exposed to Clap in main.rs.
#[derive(Debug, Clone)]
pub enum ReferralCommand {
    /// Check for new (unseen) referral emails.
    Check,
    /// List recent referrals up to `limit`.
    List { limit: usize },
    /// Process a specific referral by UID (extract, confirm, scaffold).
    Process { uid: u32 },
}

/// IMAP connection and filtering configuration from `[referral]` in
/// `config.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct ReferralConfig {
    /// IMAP server hostname (e.g. "imap.gmail.com").
    pub imap_server: String,
    /// IMAP TLS port (default 993).
    #[serde(default = "default_imap_port")]
    pub imap_port: u16,
    /// IMAP username / email address.
    pub username: String,
    /// Regex pattern matched against the sender address. Only emails whose
    /// `From` header matches this pattern are treated as referrals.
    #[serde(default = "default_sender_pattern")]
    pub sender_pattern: String,
    /// Regex pattern matched against the subject line.
    #[serde(default = "default_subject_pattern")]
    pub subject_pattern: String,
    /// Mailbox to search for referrals (default "INBOX").
    #[serde(default = "default_inbox")]
    pub inbox: String,
    /// Mailbox to move processed referrals into (default "Referrals/Processed").
    #[serde(default = "default_processed_folder")]
    pub processed_folder: String,
}

fn default_imap_port() -> u16 {
    993
}
fn default_sender_pattern() -> String {
    ".*".to_string()
}
fn default_subject_pattern() -> String {
    "(?i)referral".to_string()
}
fn default_inbox() -> String {
    "INBOX".to_string()
}
fn default_processed_folder() -> String {
    "Referrals/Processed".to_string()
}

/// A parsed referral extracted from a single email.
#[derive(Debug, Clone)]
pub struct Referral {
    /// IMAP UID of the source message.
    pub email_uid: u32,
    /// Sender address.
    pub from: String,
    /// Email subject.
    pub subject: String,
    /// Date string from the email headers.
    pub date: String,
    /// Extracted client name (heuristic).
    pub client_name: Option<String>,
    /// Extracted referrer name (heuristic).
    pub referrer: Option<String>,
    /// Extracted funding source (heuristic).
    pub funding_source: Option<String>,
    /// Extracted appointment / requested date (heuristic).
    pub appointment_date: Option<String>,
    /// Raw body text of the email.
    pub raw_body: String,
}

// ---------------------------------------------------------------------------
// Config / auth loading
// ---------------------------------------------------------------------------

/// Interactive setup wizard: ask for IMAP details, write config, store password.
pub fn init_config() -> Result<()> {
    let path = config_path();
    println!("=== Referral email setup ===\n");

    // Check if config already exists with a [referral] section
    if path.exists() {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        if content.contains("[referral]") {
            println!("Config already has a [referral] section at {}", path.display());
            print!("Overwrite? [y/N] ");
            io::stdout().flush()?;
            let mut answer = String::new();
            io::stdin().lock().read_line(&mut answer)?;
            if !answer.trim().eq_ignore_ascii_case("y") {
                println!("Aborted.");
                return Ok(());
            }
        }
    }

    // Common providers
    println!("Common IMAP servers:");
    println!("  1. Gmail        (imap.gmail.com)");
    println!("  2. Outlook/365  (outlook.office365.com)");
    println!("  3. Fastmail     (imap.fastmail.com)");
    println!("  4. Other");
    print!("\nYour provider [1-4]: ");
    io::stdout().flush()?;
    let mut choice = String::new();
    io::stdin().lock().read_line(&mut choice)?;

    let (imap_server, note) = match choice.trim() {
        "1" => ("imap.gmail.com".to_string(), "Gmail requires an App Password (not your regular password).\nGenerate one: Google Account → Security → 2-Step Verification → App Passwords."),
        "2" => ("outlook.office365.com".to_string(), "Use your Outlook/Microsoft 365 password. If MFA is enabled, you may need an App Password."),
        "3" => ("imap.fastmail.com".to_string(), "Use a Fastmail app-specific password from Settings → Privacy & Security → App Passwords."),
        _ => {
            print!("IMAP server hostname: ");
            io::stdout().flush()?;
            let mut server = String::new();
            io::stdin().lock().read_line(&mut server)?;
            (server.trim().to_string(), "")
        }
    };

    if !note.is_empty() {
        println!("\nNote: {}", note);
    }

    print!("\nYour email address: ");
    io::stdout().flush()?;
    let mut username = String::new();
    io::stdin().lock().read_line(&mut username)?;
    let username = username.trim().to_string();
    if username.is_empty() {
        bail!("Email address is required.");
    }

    print!("Referral sender pattern (e.g., 'olly' to match Olly's emails): ");
    io::stdout().flush()?;
    let mut sender_pattern = String::new();
    io::stdin().lock().read_line(&mut sender_pattern)?;
    let sender_pattern = sender_pattern.trim().to_string();

    print!("Subject pattern (e.g., 'appointment'): ");
    io::stdout().flush()?;
    let mut subject_pattern = String::new();
    io::stdin().lock().read_line(&mut subject_pattern)?;
    let subject_pattern = subject_pattern.trim().to_string();

    // Write config
    let referral_section = format!(
        "\n[referral]\nimap_server = \"{}\"\nimap_port = 993\nusername = \"{}\"\nsender_pattern = \"{}\"\nsubject_pattern = \"{}\"\ninbox = \"INBOX\"\nprocessed_folder = \"Referrals/Processed\"\n",
        imap_server, username,
        if sender_pattern.is_empty() { "referral" } else { &sender_pattern },
        if subject_pattern.is_empty() { "appointment" } else { &subject_pattern },
    );

    // Append to existing config or create new
    if path.exists() {
        let mut content = std::fs::read_to_string(&path)?;
        // Remove existing [referral] section if present
        if let Some(start) = content.find("[referral]") {
            // Find the next section or end of file
            let rest = &content[start + 10..];
            let end = rest.find("\n[").map(|i| start + 10 + i).unwrap_or(content.len());
            content = format!("{}{}", &content[..start], &content[end..]);
        }
        content.push_str(&referral_section);
        std::fs::write(&path, content)?;
    } else {
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, referral_section)?;
    }

    println!("\nConfig written to {}", path.display());

    // Store password
    println!("\nNow store your IMAP password in the OS keychain.");
    println!("(This is NOT saved to any file — it stays in the OS secret store.)\n");

    if cfg!(target_os = "macos") {
        println!("Run:");
        println!("  security add-generic-password -s clinical-imap -a \"{}\" -w <your-password>", username);
    } else {
        println!("Run:");
        println!("  \"<your-password>\" | secret-tool store --label \"Clinical IMAP\" service clinical-imap account \"{}\"", username);
    }

    println!("\nOnce stored, test with:");
    println!("  clinical-product referral check");

    Ok(())
}

/// Load the `[referral]` section from `~/.config/clinical-product/config.toml`.
pub fn load_referral_config() -> Result<ReferralConfig> {
    let path = config_path();
    if !path.exists() {
        bail!(
            "Config file not found: {path}\n\n\
             Create it with a [referral] section:\n\n\
             [referral]\n\
             imap_server = \"imap.gmail.com\"    # or your email provider\n\
             imap_port = 993\n\
             username = \"you@gmail.com\"\n\
             # password stored in OS keychain (service: clinical-imap)\n\
             sender_pattern = \"olly\"            # match referral sender\n\
             subject_pattern = \"appointment\"    # match subject line\n\n\
             Then store your IMAP password (Gmail users: generate an App Password first):\n\
             Linux:  \"<pw>\" | secret-tool store --label \"Clinical IMAP\" service clinical-imap account <email>\n\
             macOS:  security add-generic-password -s clinical-imap -a <email> -w <pw>",
            path = path.display()
        );
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let value: toml::Value = toml::from_str(&content).context("Invalid TOML in config")?;
    let section = value.get("referral");
    if section.is_none() {
        bail!(
            "No [referral] section in {path}.\n\n\
             Add:\n\n\
             [referral]\n\
             imap_server = \"imap.gmail.com\"\n\
             username = \"you@gmail.com\"\n\
             sender_pattern = \"olly\"\n\
             subject_pattern = \"appointment\"",
            path = path.display()
        );
    }
    let cfg: ReferralConfig = section
        .unwrap()
        .clone()
        .try_into()
        .context("Failed to parse [referral] section")?;
    Ok(cfg)
}

/// Retrieve the IMAP password from the OS keychain.
///
/// On Linux: `secret-tool lookup service "clinical-imap"`
/// On macOS: `security find-generic-password -s clinical-imap -w`
pub fn load_imap_password() -> Result<String> {
    let output = if cfg!(target_os = "macos") {
        Command::new("security")
            .args(["find-generic-password", "-s", "clinical-imap", "-w"])
            .output()
            .context("Failed to run `security` (macOS keychain)")?
    } else {
        Command::new("secret-tool")
            .args(["lookup", "service", "clinical-imap"])
            .output()
            .context(
                "Failed to run `secret-tool`. Install libsecret and store \
                 the password with: secret-tool store --label='Clinical IMAP' service clinical-imap",
            )?
    };

    if !output.status.success() {
        bail!(
            "IMAP password not found in keychain (service: clinical-imap). \
             Store it with:\n  secret-tool store --label='Clinical IMAP' service clinical-imap"
        );
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn config_path() -> PathBuf {
    crate::config::config_file_path()
}

// ---------------------------------------------------------------------------
// IMAP session helpers
// ---------------------------------------------------------------------------

type ImapSession = Session<Connection>;

/// Open an authenticated IMAP session over TLS.
fn connect(config: &ReferralConfig) -> Result<ImapSession> {
    let password = load_imap_password()?;
    let client = ClientBuilder::new(&config.imap_server, config.imap_port)
        .connect()
        .context("Failed to connect to IMAP server")?;
    let session = client
        .login(&config.username, &password)
        .map_err(|(e, _)| e)
        .context("IMAP login failed")?;
    Ok(session)
}

/// Move a message to the processed folder by copying + deleting.
fn mark_processed(session: &mut ImapSession, uid: u32, folder: &str) -> Result<()> {
    // Attempt to create the destination — ignore error if it already exists.
    let _ = session.create(folder);
    session
        .uid_copy(&uid.to_string(), folder)
        .with_context(|| format!("Failed to copy UID {} to {}", uid, folder))?;
    session
        .uid_store(&uid.to_string(), "+FLAGS (\\Deleted)")
        .with_context(|| format!("Failed to flag UID {} as deleted", uid))?;
    session.expunge().context("EXPUNGE failed")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Email parsing
// ---------------------------------------------------------------------------

/// Parse a raw RFC-2822 email into a `Referral`.
fn parse_email(raw: &[u8], uid: u32) -> Result<Referral> {
    let message = MessageParser::default()
        .parse(raw)
        .context("mail-parser failed to parse message")?;

    let from = message
        .from()
        .and_then(|addrs| addrs.first())
        .map(|a| {
            if let Some(name) = a.name() {
                format!("{} <{}>", name, a.address().unwrap_or_default())
            } else {
                a.address().unwrap_or_default().to_string()
            }
        })
        .unwrap_or_else(|| "(unknown sender)".to_string());

    let subject = message.subject().unwrap_or("(no subject)").to_string();

    let date = message
        .date()
        .map(|d| d.to_rfc3339())
        .unwrap_or_else(|| "(no date)".to_string());

    let body = message
        .body_text(0)
        .map(|b| b.to_string())
        .unwrap_or_default();

    let (client_name, referrer, funding_source, appointment_date) = extract_metadata(&body);

    Ok(Referral {
        email_uid: uid,
        from,
        subject,
        date,
        client_name,
        referrer,
        funding_source,
        appointment_date,
        raw_body: body,
    })
}

/// Heuristic extraction of structured metadata from a referral email body.
///
/// Looks for lines matching common patterns such as:
///   Client: Jane Doe
///   Patient Name: John Smith
///   Referrer: Dr Williams
///   Referred by: Dr Ahmed
///   Funding: AXA / Bupa / Self-funding
///   Appointment: 2026-05-01
///   Date requested: 15/06/2026
fn extract_metadata(
    body: &str,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let client_name = extract_first_match(
        body,
        &[
            r"(?i)(?:client|patient)\s*(?:name)?\s*:\s*(.+)",
            r"(?i)name\s*:\s*(.+)",
        ],
    );

    let referrer = extract_first_match(
        body,
        &[
            r"(?i)referr(?:er|ed\s+by)\s*:\s*(.+)",
            r"(?i)referring\s+(?:clinician|doctor|gp)\s*:\s*(.+)",
        ],
    );

    let funding = extract_first_match(
        body,
        &[
            r"(?i)funding\s*(?:source)?\s*:\s*(.+)",
            r"(?i)insur(?:ance|er)\s*:\s*(.+)",
            r"(?i)payment\s*:\s*(.+)",
        ],
    );

    let appt_date = extract_first_match(
        body,
        &[
            r"(?i)(?:appointment|session)\s*(?:date)?\s*:\s*(.+)",
            r"(?i)date\s*(?:requested|preferred)\s*:\s*(.+)",
            // ISO date on its own line
            r"(?m)^\s*(\d{4}-\d{2}-\d{2})\s*$",
            // UK date format dd/mm/yyyy
            r"(?m)^\s*(\d{1,2}/\d{1,2}/\d{4})\s*$",
        ],
    );

    (client_name, referrer, funding, appt_date)
}

/// Try each pattern in order; return the first capture group 1 that matches.
fn extract_first_match(text: &str, patterns: &[&str]) -> Option<String> {
    for pat in patterns {
        if let Ok(re) = Regex::new(pat) {
            if let Some(caps) = re.captures(text) {
                if let Some(m) = caps.get(1) {
                    let val = m.as_str().trim().to_string();
                    if !val.is_empty() {
                        return Some(val);
                    }
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Core public functions
// ---------------------------------------------------------------------------

/// Check for new (UNSEEN) referral emails matching the configured filters.
/// Returns parsed referrals without marking them as processed.
pub fn check_referrals(config: &ReferralConfig) -> Result<Vec<Referral>> {
    let mut session = connect(config)?;
    session
        .select(&config.inbox)
        .with_context(|| format!("Failed to select mailbox '{}'", config.inbox))?;

    let sender_re = Regex::new(&config.sender_pattern).context("Invalid sender_pattern regex")?;
    let subject_re =
        Regex::new(&config.subject_pattern).context("Invalid subject_pattern regex")?;

    // Search for unseen messages
    let uids = session
        .uid_search("UNSEEN")
        .context("IMAP UID SEARCH UNSEEN failed")?;

    if uids.is_empty() {
        session.logout().ok();
        return Ok(Vec::new());
    }

    let uid_set: String = uids
        .iter()
        .map(|u| u.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let fetched = session
        .uid_fetch(&uid_set, "(UID RFC822)")
        .context("IMAP UID FETCH failed")?;

    let mut referrals = Vec::new();
    for msg in fetched.iter() {
        let uid = msg.uid.unwrap_or(0);
        if uid == 0 {
            continue;
        }
        let Some(body) = msg.body() else { continue };
        match parse_email(body, uid) {
            Ok(r) if sender_re.is_match(&r.from) && subject_re.is_match(&r.subject) => {
                referrals.push(r);
            }
            Ok(_) => {} // didn't match filters
            Err(e) => {
                eprintln!("[referral] Failed to parse UID {}: {}", uid, e);
            }
        }
    }

    session.logout().ok();
    Ok(referrals)
}

/// List recent referrals (both seen and unseen) up to `limit`.
pub fn list_referrals(config: &ReferralConfig, limit: usize) -> Result<Vec<Referral>> {
    let mut session = connect(config)?;
    session
        .select(&config.inbox)
        .with_context(|| format!("Failed to select mailbox '{}'", config.inbox))?;

    let subject_re =
        Regex::new(&config.subject_pattern).context("Invalid subject_pattern regex")?;

    // Search ALL messages (we filter by subject pattern after fetch).
    let uids = session
        .uid_search("ALL")
        .context("IMAP UID SEARCH ALL failed")?;

    if uids.is_empty() {
        session.logout().ok();
        return Ok(Vec::new());
    }

    // Take the most recent `limit` UIDs (highest UIDs = newest).
    let mut sorted: Vec<u32> = uids.into_iter().collect();
    sorted.sort_unstable();
    sorted.reverse();
    let selected: Vec<String> = sorted
        .iter()
        .take(limit * 2) // fetch extra to account for filtering
        .map(|u| u.to_string())
        .collect();

    let uid_set = selected.join(",");
    let fetched = session
        .uid_fetch(&uid_set, "(UID RFC822)")
        .context("IMAP UID FETCH failed")?;

    let mut referrals = Vec::new();
    for msg in fetched.iter() {
        if referrals.len() >= limit {
            break;
        }
        let uid = msg.uid.unwrap_or(0);
        if uid == 0 {
            continue;
        }
        let Some(body) = msg.body() else { continue };
        match parse_email(body, uid) {
            Ok(r) if subject_re.is_match(&r.subject) => {
                referrals.push(r);
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("[referral] Failed to parse UID {}: {}", uid, e);
            }
        }
    }

    session.logout().ok();
    Ok(referrals)
}

/// Process a specific referral by UID: fetch, display, confirm, scaffold.
///
/// The propose-and-confirm flow:
/// 1. Fetch and parse the email
/// 2. Pretty-print extracted metadata
/// 3. Prompt for y/n confirmation
/// 4. On 'y': run `clinical scaffold <client_id>` and move email to processed folder
pub fn process_referral(config: &ReferralConfig, uid: u32) -> Result<()> {
    let mut session = connect(config)?;
    session
        .select(&config.inbox)
        .with_context(|| format!("Failed to select mailbox '{}'", config.inbox))?;

    let fetched = session
        .uid_fetch(&uid.to_string(), "(UID RFC822)")
        .context("IMAP UID FETCH failed")?;

    let msg = fetched
        .iter()
        .next()
        .with_context(|| format!("No message found with UID {}", uid))?;

    let body = msg.body().context("Message has no body")?;
    let referral = parse_email(body, uid)?;

    println!();
    display_referral(&referral);
    println!();

    // Build a client ID from the name (lowercase, hyphenated)
    let client_id = referral
        .client_name
        .as_deref()
        .unwrap_or("unknown-client")
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-");

    println!("Proposed client ID: {}", client_id);
    print!("Scaffold client directory? [y/N] ");
    io::stdout().flush()?;

    let mut answer = String::new();
    io::stdin()
        .lock()
        .read_line(&mut answer)
        .context("Failed to read stdin")?;

    if !answer.trim().eq_ignore_ascii_case("y") {
        println!("Aborted.");
        session.logout().ok();
        return Ok(());
    }

    // Run scaffold command
    println!("Running: clinical scaffold {}", client_id);
    let status = Command::new("clinical")
        .args(["scaffold", &client_id])
        .status()
        .context("Failed to run `clinical scaffold`")?;

    if !status.success() {
        bail!("`clinical scaffold {}` exited with {}", client_id, status);
    }

    // Move to processed folder
    mark_processed(&mut session, uid, &config.processed_folder)?;
    println!(
        "Referral UID {} moved to '{}'.",
        uid, config.processed_folder
    );

    session.logout().ok();
    Ok(())
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

/// Pretty-print a referral to stdout.
pub fn display_referral(r: &Referral) {
    println!("--- Referral (UID {}) ---", r.email_uid);
    println!("  From:       {}", r.from);
    println!("  Subject:    {}", r.subject);
    println!("  Date:       {}", r.date);
    println!(
        "  Client:     {}",
        r.client_name.as_deref().unwrap_or("(not extracted)")
    );
    println!(
        "  Referrer:   {}",
        r.referrer.as_deref().unwrap_or("(not extracted)")
    );
    println!(
        "  Funding:    {}",
        r.funding_source.as_deref().unwrap_or("(not extracted)")
    );
    println!(
        "  Appt date:  {}",
        r.appointment_date.as_deref().unwrap_or("(not extracted)")
    );
    if !r.raw_body.is_empty() {
        println!("  Body preview:");
        for line in r.raw_body.lines().take(10) {
            println!("    {}", line);
        }
        let total_lines = r.raw_body.lines().count();
        if total_lines > 10 {
            println!("    ... ({} more lines)", total_lines - 10);
        }
    }
}

// ---------------------------------------------------------------------------
// Full client setup pipeline
// ---------------------------------------------------------------------------

/// Full client setup pipeline: scaffold, populate identity, TM3 lookup, import documents.
/// Propose-and-confirm at each step.
pub fn setup_client(config: &ReferralConfig, uid: u32) -> Result<()> {
    // 1. Fetch and parse the referral email
    let mut session = connect(config)?;
    session
        .select(&config.inbox)
        .with_context(|| format!("Failed to select mailbox '{}'", config.inbox))?;

    let fetched = session
        .uid_fetch(&uid.to_string(), "(UID RFC822)")
        .context("IMAP UID FETCH failed")?;

    let msg = fetched
        .iter()
        .next()
        .with_context(|| format!("No message found with UID {}", uid))?;

    let body = msg.body().context("Message has no body")?;
    let referral = parse_email(body, uid)?;

    // 2. Display extracted metadata
    println!();
    display_referral(&referral);
    println!();

    // Build client ID from the name (lowercase, hyphenated)
    let client_id = referral
        .client_name
        .as_deref()
        .unwrap_or("unknown-client")
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-");

    println!("Proposed client ID: {}", client_id);

    // 3. Prompt: scaffold client directory
    if !prompt_yn("Scaffold client directory?")? {
        println!("Aborted.");
        session.logout().ok();
        return Ok(());
    }

    // 4. Run `clinical scaffold <client_id>`
    println!("Running: clinical scaffold {}", client_id);
    let status = Command::new("clinical")
        .args(["scaffold", &client_id])
        .status()
        .context("Failed to run `clinical scaffold`")?;

    if !status.success() {
        bail!("`clinical scaffold {}` exited with {}", client_id, status);
    }
    println!("  [OK] Client directory scaffolded.");

    // 5. Populate identity.yaml with extracted metadata
    let identity_path = identity_yaml_path(&client_id);
    if identity_path.exists() {
        println!("\nPopulating identity.yaml with extracted metadata...");
        let mut actions: Vec<String> = Vec::new();

        if let Some(ref name) = referral.client_name {
            update_identity_field(&identity_path, "name", name)?;
            actions.push(format!("  name: {}", name));
        }
        if let Some(ref referrer_name) = referral.referrer {
            update_identity_nested_field(&identity_path, "referrer", "name", referrer_name)?;
            actions.push(format!("  referrer.name: {}", referrer_name));
        }
        if let Some(ref funding) = referral.funding_source {
            update_identity_nested_field(&identity_path, "funding", "funding_type", funding)?;
            actions.push(format!("  funding.funding_type: {}", funding));
        }

        if actions.is_empty() {
            println!("  (no metadata to populate)");
        } else {
            println!("  [OK] Updated fields:");
            for a in &actions {
                println!("  {}", a);
            }
        }
    } else {
        eprintln!(
            "  [WARN] identity.yaml not found at {}; skipping populate.",
            identity_path.display()
        );
    }

    // 6. TM3 lookup — manual entry for now
    println!();
    let tm3_id = prompt_input("Enter TM3 ID (or press Enter to skip): ")?;
    let tm3_id = tm3_id.trim().to_string();

    if !tm3_id.is_empty() {
        // Write tm3_id to identity.yaml
        if identity_path.exists() {
            update_identity_field(&identity_path, "tm3_id", &tm3_id)?;
            println!("  [OK] tm3_id set to: {}", tm3_id);
        }

        // 7. Prompt: import documents from TM3
        println!();
        if prompt_yn("Import documents from TM3?")? {
            println!("Running: clinical import-doc {}", client_id);
            let import_status = Command::new("clinical")
                .args(["import-doc", &client_id])
                .status()
                .context("Failed to run `clinical import-doc`")?;

            if import_status.success() {
                println!("  [OK] Documents imported.");
            } else {
                eprintln!(
                    "  [WARN] `clinical import-doc {}` exited with {}",
                    client_id, import_status
                );
            }
        }
    } else {
        println!("  (skipping TM3 lookup)");
    }

    // 8. Move referral email to processed folder
    println!();
    mark_processed(&mut session, uid, &config.processed_folder)?;
    println!(
        "  [OK] Referral UID {} moved to '{}'.",
        uid, config.processed_folder
    );

    // 9. Summary
    println!("\n--- Setup complete ---");
    println!("  Client ID:  {}", client_id);
    println!(
        "  Directory:  ~/Clinical/clients/{}/",
        client_id
    );
    if !tm3_id.is_empty() {
        println!("  TM3 ID:     {}", tm3_id);
    }
    println!("  Email:      moved to {}", config.processed_folder);

    session.logout().ok();
    Ok(())
}

// ---------------------------------------------------------------------------
// Identity.yaml helpers
// ---------------------------------------------------------------------------

/// Return the path to a client's identity.yaml file (Route C at root, fallback to legacy private/).
fn identity_yaml_path(client_id: &str) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let root = home.join("Clinical").join("clients").join(client_id).join("identity.yaml");
    if root.exists() {
        return root;
    }
    // Legacy Route A fallback
    home.join("Clinical").join("clients").join(client_id).join("private").join("identity.yaml")
}

/// Update a top-level field in identity.yaml (e.g. `name: "..."` or `tm3_id: "..."`).
///
/// Reads the file line-by-line, replaces the first line starting with `field:`,
/// or appends the field if not found.
fn update_identity_field(path: &PathBuf, field: &str, value: &str) -> Result<()> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    let prefix = format!("{}:", field);
    let new_line = format!("{}: \"{}\"", field, value.replace('"', "\\\""));
    let mut found = false;
    let mut lines: Vec<String> = content
        .lines()
        .map(|line| {
            if !found && line.trim_start().starts_with(&prefix) {
                // Only replace if this is a top-level key (no leading whitespace)
                if line.starts_with(&prefix) || line.starts_with(&format!("{}:", field)) {
                    found = true;
                    return new_line.clone();
                }
            }
            line.to_string()
        })
        .collect();

    if !found {
        lines.push(new_line);
    }

    let output = lines.join("\n");
    // Ensure trailing newline
    let output = if output.ends_with('\n') {
        output
    } else {
        format!("{}\n", output)
    };

    std::fs::write(path, &output)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

/// Update a nested field in identity.yaml (e.g. `referrer:\n  name: "..."`).
///
/// Looks for the parent key, then searches for the child key indented below it.
/// If the child isn't found under the parent, inserts it.
fn update_identity_nested_field(
    path: &PathBuf,
    parent: &str,
    child: &str,
    value: &str,
) -> Result<()> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    let parent_prefix = format!("{}:", parent);
    let child_key = format!("{}:", child);
    let new_child_line = format!("  {}: \"{}\"", child, value.replace('"', "\\\""));

    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let mut parent_idx: Option<usize> = None;
    let mut child_idx: Option<usize> = None;

    // Find the parent key (top-level, no indentation)
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with(&parent_prefix) {
            parent_idx = Some(i);
            break;
        }
    }

    if let Some(pidx) = parent_idx {
        // Search for the child key in the indented block below the parent
        for i in (pidx + 1)..lines.len() {
            let line = &lines[i];
            // If we hit a non-indented line (next top-level key), stop searching
            if !line.is_empty() && !line.starts_with(' ') && !line.starts_with('\t') {
                break;
            }
            if line.trim_start().starts_with(&child_key) {
                child_idx = Some(i);
                break;
            }
        }

        if let Some(cidx) = child_idx {
            lines[cidx] = new_child_line;
        } else {
            // Insert the child right after the parent
            lines.insert(pidx + 1, new_child_line);
        }
    } else {
        // Parent not found — append parent and child
        lines.push(format!("{}:", parent));
        lines.push(new_child_line);
    }

    let output = lines.join("\n");
    let output = if output.ends_with('\n') {
        output
    } else {
        format!("{}\n", output)
    };

    std::fs::write(path, &output)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Interactive prompt helpers
// ---------------------------------------------------------------------------

/// Prompt the user with a yes/no question. Returns true if 'y'/'Y'.
fn prompt_yn(question: &str) -> Result<bool> {
    print!("{} [y/N] ", question);
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin()
        .lock()
        .read_line(&mut answer)
        .context("Failed to read stdin")?;
    Ok(answer.trim().eq_ignore_ascii_case("y"))
}

/// Prompt the user for free-text input. Returns the trimmed response.
fn prompt_input(prompt: &str) -> Result<String> {
    print!("{}", prompt);
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin()
        .lock()
        .read_line(&mut answer)
        .context("Failed to read stdin")?;
    Ok(answer)
}
