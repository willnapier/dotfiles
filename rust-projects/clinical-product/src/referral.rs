//! IMAP referral intake module.
//!
//! Watches a configured IMAP inbox for referral emails, extracts client
//! metadata via heuristic pattern matching, and proposes scaffolding a new
//! client directory. Auth via keychain (`secret-tool lookup service
//! "clinical-imap"` on Linux).
//!
//! Config lives in `~/.config/clinical-product/voice-config.toml` under the
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
/// `voice-config.toml`.
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

/// Load the `[referral]` section from `~/.config/clinical-product/voice-config.toml`.
pub fn load_referral_config() -> Result<ReferralConfig> {
    let path = config_path();
    if !path.exists() {
        bail!(
            "Config file not found: {}. Create it with a [referral] section.",
            path.display()
        );
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let value: toml::Value = toml::from_str(&content).context("Invalid TOML in config")?;
    let section = value
        .get("referral")
        .context("No [referral] section in config")?;
    let cfg: ReferralConfig = section
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
    if let Some(home) = dirs::home_dir() {
        home.join(".config")
            .join("clinical-product")
            .join("voice-config.toml")
    } else {
        PathBuf::from("voice-config.toml")
    }
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
