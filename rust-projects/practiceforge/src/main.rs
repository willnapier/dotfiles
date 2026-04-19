#![recursion_limit = "256"]

use clap::Parser;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub mod billing;
pub mod config;
mod admin_dashboard;
mod dashboard;
pub mod email;
pub mod onboard;
pub mod portal;
mod referral;
pub mod registry;
mod runpod;
pub mod search;
pub mod scheduling;
pub mod session_cookies;
pub mod sms;
mod sync;
mod sync_docs;
mod inference;
pub mod tm3_clients;
pub mod tm3_diary;
pub mod tm3_migrate;
pub mod outcomes;

#[derive(Parser)]
#[command(name = "practiceforge", about = "PracticeForge — clinical practice management")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Parser)]
enum Command {
    /// Generate a session note from an observation
    Note {
        /// The clinical observation (2-3 sentences)
        observation: String,

        /// Therapeutic modality for terminology (e.g. "ACT/CBS", "psychodynamic", "integrative CBT")
        #[arg(short, long, default_value = "ACT/CBS")]
        modality: String,

        /// Ollama API endpoint URL
        #[arg(short, long, default_value = "http://localhost:11434")]
        endpoint: String,

        /// Model name
        #[arg(long, default_value = "gemma4:26b")]
        model: String,

        /// Disable streaming (wait for full response)
        #[arg(long)]
        no_stream: bool,
    },

    /// Raw completion: reads a full prompt from stdin, streams completion to stdout.
    /// Used by `clinical note` as CLINICAL_LLM_CMD to integrate the inference model.
    Raw {
        /// Ollama API endpoint URL
        #[arg(short, long, default_value = "http://localhost:11434")]
        endpoint: String,

        /// Model name
        #[arg(long, default_value = "gemma4:26b")]
        model: String,

        /// Disable streaming (wait for full response before printing)
        #[arg(long)]
        no_stream: bool,
    },

    /// Manage the inference pod lifecycle (status/start/stop).
    ///
    /// Reads the `[pod]` section of ~/.config/practiceforge/config.toml
    /// to determine which pod to manage. If `managed = false` or pod_id is
    /// empty, all commands report the configured state without making changes.
    Inference {
        #[command(subcommand)]
        action: InferencePodAction,
    },

    /// Referral intake from IMAP email.
    ///
    /// Watches a configured inbox for referral emails, extracts client
    /// metadata, and proposes scaffolding a new client directory.
    Referral {
        #[command(subcommand)]
        action: ReferralAction,
    },

    /// Compare TM3 diary against local client directories.
    ///
    /// Scrapes today's TM3 diary, compares against ~/Clinical/clients/,
    /// and reports new clients that need scaffolding. Auto-onboards
    /// unmapped clients by default (use --dry-run to just report).
    Sync {
        /// Report unmapped clients without onboarding them
        #[arg(long)]
        dry_run: bool,
    },

    /// Auto-onboard a new TM3 client: scrape profile, scaffold, import docs.
    ///
    /// Zero-shot: scrapes the TM3 client profile for metadata (DOB, referrer,
    /// funding), derives a client ID, scaffolds the directory, populates
    /// identity.yaml, updates tm3-client-map, downloads and imports documents.
    Onboard {
        /// Client name as it appears in TM3 (e.g. "Briscoe, Elizabeth")
        name: String,

        /// TM3 numeric client ID (found via sync or diary links)
        #[arg(long)]
        tm3_id: Option<String>,
    },

    /// Poll TM3 for newly added documents across all clients and import them.
    ///
    /// For each client with a tm3_id in identity.yaml, runs `clinical import-doc`
    /// to list/download/extract any documents Olly has uploaded since the last
    /// sync. Idempotent — existing files are skipped by filename. Intended to
    /// run on a timer (hourly alongside tm3-diary-capture).
    SyncDocs {
        /// List remote docs without downloading or importing.
        #[arg(long)]
        dry_run: bool,

        /// Restrict the sync to a single client ID.
        #[arg(long)]
        client: Option<String>,

        /// Show per-client outcome, not just the summary.
        #[arg(long)]
        verbose: bool,
    },

    /// Email configuration and sending.
    ///
    /// Setup wizard for SMTP credentials, test delivery, and direct
    /// email sending with PDF attachments for clinical letters.
    Email {
        #[command(subcommand)]
        action: EmailAction,
    },

    /// Billing automation — invoice, track, and remind.
    ///
    /// Vendor-neutral, per-practitioner billing. Uses pluggable backends:
    /// Manual (file-based, no API keys) or Xero/Stripe (future).
    /// Enable via [billing] section in config.toml.
    Billing {
        #[command(subcommand)]
        action: BillingAction,
    },

    /// Start the clinical dashboard (local web UI).
    ///
    /// Serves a browser-based note-writing interface on localhost.
    Dashboard {
        /// Port to listen on
        #[arg(long, default_value = "3456")]
        port: u16,

        /// Open browser automatically
        #[arg(long)]
        open: bool,
    },

    /// PracticeForge admin dashboard — multi-practitioner practice management UI.
    ///
    /// A separate web UI for practice admin: calendar views across all
    /// practitioners, client management, search, and billing overview.
    /// Runs on a different port from the practitioner dashboard.
    AdminDashboard {
        /// Port to listen on
        #[arg(long, default_value = "3457")]
        port: u16,

        /// Open browser automatically
        #[arg(long)]
        open: bool,
    },

    /// PracticeForge scheduling — appointments, recurrence, self-booking.
    ///
    /// Infinite recurring sessions, block expiry warnings, ICS export.
    /// Enable via [scheduling] section in config.toml.
    Schedule {
        #[command(subcommand)]
        action: ScheduleAction,
    },

    /// PracticeForge central client registry.
    ///
    /// Git-backed shared client database for multi-practitioner practices.
    /// Enable via [registry] section in config.toml.
    Registry {
        #[command(subcommand)]
        action: RegistryAction,
    },

    /// SMS appointment reminders via Twilio.
    ///
    /// Send reminder texts to clients before appointments. Preview
    /// what would be sent, send for real, or check delivery status.
    /// Enable via [sms] section in config.toml.
    Sms {
        #[command(subcommand)]
        action: SmsAction,
    },

    /// Full-text search across all client files.
    ///
    /// Tantivy-powered search across notes, correspondence, identity,
    /// and diagnosis. Auto-rebuilds stale indexes (> 1 hour old).
    Search {
        /// Search query
        query: String,
        /// Restrict search to a specific client
        #[arg(long)]
        client: Option<String>,
        /// Restrict to a field (notes, correspondence, identity, diagnosis)
        #[arg(long)]
        field: Option<String>,
        /// Maximum results
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Rebuild the search index
        #[arg(long)]
        reindex: bool,
    },

    /// TM3 data export and migration to PracticeForge.
    ///
    /// Orchestrates a full data export from TM3 into PracticeForge's
    /// registry: clients, calendar, documents, and validation.
    #[command(name = "tm3-migrate")]
    Tm3Migrate {
        #[command(subcommand)]
        action: Tm3MigrateAction,
    },

    /// Outcome measures — record, view, and export standardised questionnaire scores.
    ///
    /// Supports PHQ-9, GAD-7, CORE-10, PCL-5, WEMWBS, ISI, and any custom measure.
    /// Scores are stored per-client at ~/Clinical/clients/<id>/outcomes/<measure>.yaml.
    Outcomes {
        #[command(subcommand)]
        action: OutcomesAction,
    },

    /// TM3 diary write-back — create appointments directly in TM3.
    ///
    /// Calls the TM3 ServiceStack API using stored session cookies.
    /// Requires a valid TM3 session (run tm3-spike login on Mac to refresh).
    /// IDs (practitioner_id, location_id, stock_id) are read from [tm3_diary]
    /// in config.toml; defaults match the 2026-04-19 capture.
    #[command(name = "tm3-diary")]
    Tm3Diary {
        #[command(subcommand)]
        action: Tm3DiaryAction,
    },
}

#[derive(Parser, Debug)]
enum ReferralAction {
    /// Check for new (unseen) referral emails.
    Check,
    /// List recent referrals.
    List {
        #[arg(long, default_value = "10")]
        limit: usize,
    },
    /// Process a specific referral by UID (extract, confirm, scaffold).
    Process { uid: u32 },
    /// Full client setup: scaffold → populate identity → TM3 lookup → import documents.
    Setup { uid: u32 },
    /// Interactive setup wizard for email referral monitoring.
    Init,
}

#[derive(Parser, Debug)]
enum EmailAction {
    /// Interactive setup wizard — configure SMTP server and store password.
    Init,
    /// Send a test email to yourself to verify configuration.
    Test,
    /// Send an email with optional PDF attachment.
    Send {
        /// Recipient email address
        #[arg(long)]
        to: String,
        /// Recipient name
        #[arg(long, default_value = "")]
        to_name: String,
        /// Email subject
        #[arg(long)]
        subject: String,
        /// Email body text
        #[arg(long)]
        body: String,
        /// Path to PDF attachment
        #[arg(long)]
        attachment: Option<String>,
        /// CC recipients (comma-separated)
        #[arg(long)]
        cc: Option<String>,
    },
}

#[derive(Parser, Debug)]
enum BillingAction {
    /// Show billing status (outstanding and overdue invoices).
    Status {
        /// Show only overdue invoices.
        #[arg(long)]
        overdue: bool,
    },
    /// Create an invoice for a client's uninvoiced sessions.
    Invoice {
        /// Client ID (e.g. "JB92")
        client_id: String,
        /// Specific session dates to invoice (YYYY-MM-DD, comma-separated).
        /// If omitted, invoices all uninvoiced sessions.
        #[arg(long)]
        dates: Option<String>,
        /// Email the invoice to the bill-to party after creating it.
        #[arg(long)]
        send: bool,
    },
    /// Create invoices for all clients with uninvoiced sessions.
    InvoiceBatch {
        /// Preview only — show what would be invoiced without creating.
        #[arg(long)]
        dry_run: bool,
        /// Email each invoice to the bill-to party after creating it.
        #[arg(long)]
        send: bool,
    },
    /// Mark an invoice as paid.
    Paid {
        /// Invoice reference (e.g. "INV-2026-0001")
        reference: String,
        /// Payment date (YYYY-MM-DD). Defaults to today.
        #[arg(long)]
        date: Option<String>,
    },
    /// Cancel an invoice.
    Cancel {
        /// Invoice reference
        reference: String,
        /// Reason for cancellation
        #[arg(long, default_value = "Cancelled")]
        reason: String,
    },
    /// Show reminders due for overdue invoices (dry-run by default).
    Remind {
        /// Actually send the reminder emails.
        #[arg(long)]
        send: bool,
    },
    /// Periodic maintenance: check overdue, report status.
    /// Safe to run from any scheduler (launchd, systemd, cron).
    Maintain,
    /// Interactive setup wizard — enable billing, set payment terms, currency.
    Init,
    /// View or edit billing settings.
    ///
    /// With no arguments: show current settings.
    /// With key=value: update a setting.
    Config {
        /// Setting to update (e.g. "payment_terms_days=21", "currency=GBP").
        /// Omit to show all current settings.
        #[arg(name = "KEY=VALUE")]
        setting: Option<String>,
    },
    /// Run the Xero OAuth2 PKCE authorization flow.
    ///
    /// Opens a browser URL, waits for the callback on localhost:8765,
    /// and saves the access/refresh tokens to secrets.toml.
    XeroAuth,
    /// Save Xero API credentials to secrets.toml.
    ///
    /// Obtain client_id and client_secret from the Xero Developer Portal.
    XeroSetup {
        /// Xero OAuth2 client ID
        client_id: String,
        /// Xero OAuth2 client secret
        client_secret: String,
    },
    /// Save Stripe secret key to secrets.toml.
    StripeKey {
        /// Stripe secret key (sk_live_... or sk_test_...)
        secret_key: String,
    },
}

#[derive(Parser, Debug)]
enum InferencePodAction {
    /// Show current pod status (queries RunPod API).
    Status,

    /// Start (or resume) the configured pod. Idempotent if already running.
    Start,

    /// Stop the configured pod. Idempotent if already stopped.
    Stop,

    /// Check idle timeout and stop the pod if idle. Safe to run periodically
    /// via cron/launchd/systemd as a cross-platform background sweeper.
    Maintain,

    /// List all pods on the account (for discovery / setup).
    List,

    /// List all network volumes on the account.
    Volumes,
}

#[derive(Parser, Debug)]
enum ScheduleAction {
    /// List upcoming appointments.
    List {
        /// Show appointments for a specific date (YYYY-MM-DD). Default: today.
        #[arg(long)]
        date: Option<String>,
        /// Show a full week instead of a single day.
        #[arg(long)]
        week: bool,
        /// Filter by practitioner slug.
        #[arg(long)]
        practitioner: Option<String>,
    },
    /// Create an appointment (one-off or start a recurring series).
    Create {
        /// Client ID (e.g. "EB76")
        client_id: String,
        /// Date and time (YYYY-MM-DD HH:MM)
        datetime: String,
        /// Duration in minutes
        #[arg(long, default_value = "50")]
        duration: u32,
        /// Make this a recurring series
        #[arg(long, value_parser = ["weekly", "fortnightly", "every3w", "monthly"])]
        recur: Option<String>,
        /// Number of sessions (omit for infinite)
        #[arg(long)]
        count: Option<u32>,
        /// Infinite recurrence (no end date, no count)
        #[arg(long)]
        infinite: bool,
    },
    /// Show authorisation block status and expiry warnings.
    Blocks {
        /// Client ID (omit to show all)
        client_id: Option<String>,
    },
    /// Cancel an appointment or stop a recurring series.
    Cancel {
        /// Client ID
        client_id: String,
        /// Specific date to cancel (YYYY-MM-DD) — cancels one instance
        #[arg(long)]
        date: Option<String>,
        /// Cancel the entire recurring series
        #[arg(long)]
        series: bool,
    },
    /// Reschedule an appointment to a new date/time.
    Move {
        /// Client ID
        client_id: String,
        /// Original date (YYYY-MM-DD)
        from: String,
        /// New date and time (YYYY-MM-DD HH:MM)
        to: String,
    },
    /// Update appointment status (arrived, completed, noshow, late-cancel).
    Update {
        /// Client ID
        client_id: String,
        /// Date of appointment (YYYY-MM-DD)
        date: String,
        /// New status
        #[arg(long, value_parser = ["arrived", "completed", "noshow", "late-cancel"])]
        status: String,
    },
    /// Find available reschedule slots for a cancelled/DNA session.
    ///
    /// Searches the next 48h for gaps that fit the client's session duration.
    /// Ranked by preference cascade: minimise day span, prefer Tue-Thu,
    /// pack contiguous, sooner is better.
    Reschedule {
        /// Client ID (e.g. "EB76")
        client_id: String,
        /// Date of the cancelled session (YYYY-MM-DD)
        date: String,
        /// Override session duration in minutes (default: from identity.yaml)
        #[arg(long)]
        duration: Option<u32>,
        /// Send the slot offer via email to the client
        #[arg(long)]
        send: bool,
    },
    /// Export appointments as ICS (iCalendar) format.
    Export {
        /// Filter by practitioner
        #[arg(long)]
        practitioner: Option<String>,
    },
    /// Periodic maintenance: check block expiry, send reminders.
    /// Safe to run from any scheduler (launchd, systemd, cron).
    Maintain,
    /// Generate a self-booking link for a client.
    Link {
        /// Client ID (e.g. "EB76")
        client_id: String,
    },
}

#[derive(Parser, Debug)]
enum RegistryAction {
    /// Interactive setup wizard — configure registry path and remote.
    Init,
    /// Create the registry repository (or clone from remote).
    Create {
        /// Remote git URL to clone from (omit for local-only).
        #[arg(long)]
        remote: Option<String>,
    },
    /// Sync with remote: pull, commit local changes, push.
    Sync,
    /// List all clients in the registry.
    List {
        /// Filter by status (active, discharged, all).
        #[arg(long, default_value = "active")]
        status: String,
    },
    /// Show details for a specific client.
    Get {
        /// Client ID (e.g. "EB76")
        client_id: String,
    },
    /// Import clients from ~/Clinical/clients/ into the registry.
    Import {
        /// Import a single client by ID (omit for --all).
        client_id: Option<String>,
        /// Import all clients not already in the registry.
        #[arg(long)]
        all: bool,
    },
    /// Show registry status (sync state, client count, remote info).
    Status,
    /// Push a letter PDF to the registry for a client.
    PushLetter {
        /// Client ID
        client_id: String,
        /// Path to the letter PDF
        path: String,
    },
}

#[derive(Parser, Debug)]
enum SmsAction {
    /// Preview reminders that would be sent (dry run).
    Preview {
        /// Date to preview reminders for (YYYY-MM-DD). Default: tomorrow.
        #[arg(long)]
        date: Option<String>,
    },
    /// Send reminders for a date.
    Send {
        /// Date to send reminders for (YYYY-MM-DD). Default: tomorrow.
        #[arg(long)]
        date: Option<String>,
    },
    /// Show delivery status for sent reminders.
    Status {
        /// Date to check status for (YYYY-MM-DD). Default: today.
        #[arg(long)]
        date: Option<String>,
    },
    /// Send a test SMS to verify Twilio configuration.
    Test {
        /// Phone number to send to (E.164 format, e.g. +447700900000)
        phone: String,
        /// Message text
        #[arg(long, default_value = "Test from PracticeForge")]
        message: String,
    },
}

#[derive(Parser, Debug)]
enum Tm3MigrateAction {
    /// Export all TM3 clients into the PracticeForge registry.
    ExportClients {
        #[arg(long)]
        dry_run: bool,
    },
    /// Export TM3 diary data into PracticeForge scheduling format.
    ExportCalendar {
        #[arg(long)]
        dry_run: bool,
    },
    /// Download all documents from TM3 into the registry.
    ExportDocuments {
        #[arg(long)]
        dry_run: bool,
    },
    /// Validate migration completeness — compare TM3 against registry.
    Validate,
    /// Run full migration: clients -> calendar -> documents -> validate.
    Run {
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Parser, Debug)]
enum OutcomesAction {
    /// Record a new score for a client.
    ///
    /// Example: `practiceforge outcomes record WN01 phq9 14 --notes "Baseline"`
    Record {
        /// Client ID (e.g. "WN01")
        client_id: String,
        /// Measure slug (e.g. "phq9", "gad7", "core10", "pcl5", "wemwbs", "isi")
        measure: String,
        /// Total score
        score: f64,
        /// Date of assessment (YYYY-MM-DD). Defaults to today.
        #[arg(long)]
        date: Option<String>,
        /// Individual item scores as comma-separated values (e.g. "1,2,1,2,2,1,1,2,2")
        #[arg(long)]
        items: Option<String>,
        /// Free-text notes
        #[arg(long)]
        notes: Option<String>,
    },
    /// Show outcome scores for a client.
    ///
    /// With no measure: summary table across all measures.
    /// With a measure slug: full history for that measure.
    Show {
        /// Client ID
        client_id: String,
        /// Measure slug (optional — omit for all-measures summary)
        measure: Option<String>,
    },
    /// Print the outcomes context block (markdown table) to stdout.
    ///
    /// Useful for copy-paste into clinical records or piping to other tools.
    Export {
        /// Client ID
        client_id: String,
    },
}

#[derive(Parser, Debug)]
enum Tm3DiaryAction {
    /// Book an appointment in TM3 for a client.
    ///
    /// Reads tm3_id from the client's identity.yaml, looks up the full
    /// customer record in the TM3 client cache, and creates the appointment
    /// via the captured ServiceStack API.
    ///
    /// Example: practiceforge tm3-diary book AB79 "2026-04-21 14:30" --duration 60
    Book {
        /// PracticeForge client ID (e.g. "AB79")
        client_id: String,
        /// Date and time: "YYYY-MM-DD HH:MM"
        datetime: String,
        /// Duration in minutes
        #[arg(long, default_value = "60")]
        duration: u32,
        /// Override TM3 customer ID (skips identity.yaml lookup)
        #[arg(long)]
        tm3_id: Option<u64>,
        /// Print what would be sent without calling the API
        #[arg(long)]
        dry_run: bool,
    },
    /// Mark attendance on an existing TM3 appointment.
    ///
    /// Requires the TM3 appointment ID (returned by `book` or found in TM3 URL).
    /// Status options: arrived, completed, dna (no-show), scheduled (revert).
    ///
    /// Example: practiceforge tm3-diary update 155058 --status arrived
    Update {
        /// TM3 appointment ID (numeric)
        appointment_id: u64,
        /// New attendance status
        #[arg(long, value_parser = ["arrived", "completed", "dna", "scheduled"])]
        status: String,
        /// Optional appointment note / comment
        #[arg(long)]
        comment: Option<String>,
        /// Print what would be sent without calling the API
        #[arg(long)]
        dry_run: bool,
    },

    /// Reschedule an existing appointment to a new date/time.
    ///
    /// Example: practiceforge tm3-diary reschedule 155058 "2026-04-22 14:30" --duration 60
    Reschedule {
        /// TM3 appointment ID (numeric)
        appointment_id: u64,
        /// New date and time: "YYYY-MM-DD HH:MM"
        datetime: String,
        /// Duration in minutes (to compute end time)
        #[arg(long, default_value = "60")]
        duration: u32,
        /// Print what would be sent without calling the API
        #[arg(long)]
        dry_run: bool,
    },

    /// Delete a TM3 appointment.
    ///
    /// Example: practiceforge tm3-diary delete 155058
    Delete {
        /// TM3 appointment ID (numeric)
        appointment_id: u64,
        /// Print what would be sent without calling the API
        #[arg(long)]
        dry_run: bool,
    },

    /// Fetch a TM3 appointment by ID and print its details.
    Get {
        /// TM3 appointment ID (numeric)
        appointment_id: u64,
    },

    /// Refresh the TM3 client cache (alias for convenience).
    ///
    /// Same as running 'practiceforge tm3-clients refresh'. Required before
    /// first booking and when new clients are added in TM3.
    RefreshCache,
}

#[derive(Serialize)]
struct GenerateRequest {
    model: String,
    prompt: String,
    system: String,
    stream: bool,
}

#[derive(Deserialize)]
struct StreamChunk {
    response: Option<String>,
    done: Option<bool>,
    total_duration: Option<u64>,
    eval_count: Option<u64>,
    eval_duration: Option<u64>,
}

fn build_system_prompt(modality: &str) -> String {
    format!(
        "You are a clinical psychologist's session note writer. \
         Produce a session note in the practitioner's established style. \
         Frame clinical reasoning using explicit {} process terminology — \
         name the relevant therapeutic processes where they apply to the session material. \
         Integrate these naturally into the prose rather than listing them. \
         Refer to the client by first name throughout, not 'the client' or 'Client'. \
         When describing in-session experiments or interventions, show that the client \
         was consulted and consented before proceeding — do not present them as imposed. \
         Do not combine 'collaborative' with 'agreed' — either word implies the other. \
         Frame interpretive links to developmental history or formulation tentatively \
         (e.g. 'this was explored as potentially connected to...') while anchoring \
         to the existing formulation. \
         When documenting agreed between-session tasks, include sufficient detail \
         (duration, context, what to observe) to evidence collaborative planning. \
         Every specific detail — examples, metaphors, homework tasks, contexts — must \
         come from the observation or the client file. If the source material does not \
         specify concrete examples, describe the task in general terms rather than \
         inventing plausible specifics. \
         Structure: Risk assessment, narrative body, Formulation. \
         For the risk assessment, use a brief default (e.g. 'No immediate concerns noted') \
         unless the observation specifically describes risk factors. Do NOT confabulate \
         detailed risk assessments or imply that explicit screening was conducted.",
        modality
    )
}

/// Ensure the managed pod (if any) is running before a generation request.
/// Silent no-op if pod management isn't configured.
async fn ensure_managed_pod_ready() -> anyhow::Result<()> {
    let config = inference::load_pod_config()?;
    if !config.has_pod() {
        return Ok(());
    }
    let client = runpod::Client::new()?;
    inference::prepare_for_request(&client, &config).await?;
    Ok(())
}

async fn raw_completion(
    prompt: String,
    endpoint: String,
    model: String,
    no_stream: bool,
) -> anyhow::Result<()> {
    // Pre-flight: ensure managed pod is up (no-op if unmanaged).
    ensure_managed_pod_ready().await?;

    // For raw mode, we send the entire stdin content as the prompt with
    // an empty system message — the caller (e.g. `clinical note`) has
    // already built the full context and instruction.
    let request = GenerateRequest {
        model,
        prompt,
        system: String::new(),
        stream: !no_stream,
    };

    let client = Client::new();
    let url = format!("{}/api/generate", endpoint);
    let start = std::time::Instant::now();

    if no_stream {
        let resp: serde_json::Value = client.post(&url).json(&request).send().await?.json().await?;
        let text = resp["response"].as_str().unwrap_or("");
        print!("{}", text);
        eprintln!("\n---\nGenerated in {:.1}s", start.elapsed().as_secs_f64());
    } else {
        let resp = client.post(&url).json(&request).send().await?;
        let mut stream = resp.bytes_stream();
        let stdout = io::stdout();
        let mut out = stdout.lock();
        let stderr = io::stderr();

        while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            if let Ok(parsed) = serde_json::from_slice::<StreamChunk>(&bytes) {
                if let Some(text) = &parsed.response {
                    write!(out, "{}", text)?;
                    out.flush()?;
                }
                if parsed.done == Some(true) {
                    if let (Some(ec), Some(ed)) = (parsed.eval_count, parsed.eval_duration) {
                        let tps = if ed > 0 { ec as f64 / (ed as f64 / 1e9) } else { 0.0 };
                        let mut err = stderr.lock();
                        writeln!(err)?;
                        writeln!(
                            err,
                            "---\nGenerated {} tokens in {:.1}s ({:.0} tok/s)",
                            ec,
                            start.elapsed().as_secs_f64(),
                            tps
                        )?;
                    }
                }
            }
        }
    }

    // Record activity so the idle timer knows something just happened.
    let _ = inference::record_activity();

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Raw {
            endpoint,
            model,
            no_stream,
        } => {
            // Read full prompt from stdin
            let mut prompt = String::new();
            io::stdin().read_to_string(&mut prompt)?;
            if prompt.trim().is_empty() {
                anyhow::bail!("Empty prompt on stdin");
            }
            raw_completion(prompt, endpoint, model, no_stream).await?;
        }
        Command::Note {
            observation,
            modality,
            endpoint,
            model,
            no_stream,
        } => {
            let system = build_system_prompt(&modality);
            let prompt = format!(
                "Write a session note for today's session.\n\nObservation: {}",
                observation
            );

            let request = GenerateRequest {
                model,
                prompt,
                system,
                stream: !no_stream,
            };

            let client = Client::new();
            let url = format!("{}/api/generate", endpoint);

            let start = std::time::Instant::now();

            if no_stream {
                let resp: serde_json::Value = client
                    .post(&url)
                    .json(&request)
                    .send()
                    .await?
                    .json()
                    .await?;

                let text = resp["response"].as_str().unwrap_or("");
                println!("{}", text);

                let elapsed = start.elapsed();
                eprintln!(
                    "\n---\nGenerated in {:.1}s",
                    elapsed.as_secs_f64()
                );
            } else {
                let resp = client.post(&url).json(&request).send().await?;
                let mut stream = resp.bytes_stream();
                let mut total_tokens = 0u64;

                let stderr = io::stderr();
                let stdout = io::stdout();
                let mut out = stdout.lock();

                while let Some(chunk) = stream.next().await {
                    let bytes = chunk?;
                    if let Ok(parsed) = serde_json::from_slice::<StreamChunk>(&bytes) {
                        if let Some(text) = &parsed.response {
                            write!(out, "{}", text)?;
                            out.flush()?;
                        }
                        if parsed.done == Some(true) {
                            if let (Some(ec), Some(ed)) =
                                (parsed.eval_count, parsed.eval_duration)
                            {
                                total_tokens = ec;
                                let elapsed = start.elapsed();
                                let tps = if ed > 0 {
                                    ec as f64 / (ed as f64 / 1e9)
                                } else {
                                    0.0
                                };
                                let mut err = stderr.lock();
                                writeln!(err)?;
                                writeln!(err, "---")?;
                                writeln!(
                                    err,
                                    "Generated {} tokens in {:.1}s ({:.0} tok/s)",
                                    total_tokens,
                                    elapsed.as_secs_f64(),
                                    tps
                                )?;
                            }
                        }
                    }
                }
                println!();
            }
        }
        Command::Inference { action } => {
            handle_inference(action).await?;
        }
        Command::Referral { action } => {
            if matches!(action, ReferralAction::Init) {
                referral::init_config()?;
                return Ok(());
            }
            let config = referral::load_referral_config()?;
            match action {
                ReferralAction::Check => {
                    let referrals = referral::check_referrals(&config)?;
                    if referrals.is_empty() {
                        println!("No new referral emails found.");
                    } else {
                        println!("Found {} new referral(s):\n", referrals.len());
                        for r in &referrals {
                            referral::display_referral(r);
                            println!();
                        }
                    }
                }
                ReferralAction::List { limit } => {
                    let referrals = referral::list_referrals(&config, limit)?;
                    if referrals.is_empty() {
                        println!("No referral emails found.");
                    } else {
                        println!("Recent referrals ({}):\n", referrals.len());
                        for r in &referrals {
                            referral::display_referral(r);
                            println!();
                        }
                    }
                }
                ReferralAction::Process { uid } => {
                    referral::process_referral(&config, uid)?;
                }
                ReferralAction::Setup { uid } => {
                    referral::setup_client(&config, uid)?;
                }
                ReferralAction::Init => unreachable!(),
            }
        }
        Command::Sync { dry_run } => {
            let result = sync::sync_check()?;
            sync::display_sync_result(&result);

            // Auto-onboard unmapped clients
            if !dry_run && !result.unmatched_tm3.is_empty() {
                println!("\n--- Auto-onboarding {} new client(s) ---\n", result.unmatched_tm3.len());
                for client in &result.unmatched_tm3 {
                    let tm3_id = client.tm3_id.as_deref();
                    match onboard::onboard(&client.name, tm3_id) {
                        Ok(r) if r.skipped => {
                            println!("  {} — already onboarded as {}", r.name, r.client_id);
                        }
                        Ok(r) => {
                            println!(
                                "  ✓ {} → {} ({} doc{} imported)",
                                r.name, r.client_id, r.docs_imported,
                                if r.docs_imported == 1 { "" } else { "s" }
                            );
                        }
                        Err(e) => {
                            eprintln!("  ✗ {} — onboard failed: {}", client.name, e);
                        }
                    }
                }
            }
        }
        Command::Onboard { name, tm3_id } => {
            let result = onboard::onboard(&name, tm3_id.as_deref())?;
            if result.skipped {
                println!("{} already onboarded as {}.", result.name, result.client_id);
            } else {
                println!(
                    "✓ {} onboarded as {} ({} doc{} imported).",
                    result.name,
                    result.client_id,
                    result.docs_imported,
                    if result.docs_imported == 1 { "" } else { "s" }
                );
            }
        }
        Command::SyncDocs { dry_run, client, verbose } => {
            let result = sync_docs::sync_all(dry_run, client.as_deref())?;
            sync_docs::print_report(&result, verbose);
        }
        Command::Email { action } => {
            match action {
                EmailAction::Init => {
                    email::init_config()?;
                }
                EmailAction::Test => {
                    let config = email::load_email_config()?;
                    email::send_test(&config)?;
                }
                EmailAction::Send {
                    to,
                    to_name,
                    subject,
                    body,
                    attachment,
                    cc,
                } => {
                    let config = email::load_email_config()?;
                    let cc_list: Option<Vec<String>> = cc.map(|c| {
                        c.split(',').map(|s| s.trim().to_string()).collect()
                    });
                    email::send_email(
                        &config,
                        &to,
                        &to_name,
                        &subject,
                        &body,
                        attachment.as_ref().map(|p| std::path::Path::new(p.as_str())),
                        cc_list.as_deref(),
                    )?;
                    println!("✓ Email sent to {}", to);
                }
            }
        }
        Command::Billing { action } => {
            handle_billing(action)?;
        }
        Command::Dashboard { port, open } => {
            dashboard::serve(port, open).await?;
        }
        Command::AdminDashboard { port, open } => {
            admin_dashboard::serve(port, open).await?;
        }
        Command::Schedule { action } => {
            handle_schedule(action)?;
        }
        Command::Registry { action } => {
            handle_registry(action)?;
        }
        Command::Sms { action } => {
            handle_sms(action).await?;
        }
        Command::Search {
            query,
            client,
            field,
            limit,
            reindex,
        } => {
            handle_search(query, client, field, limit, reindex)?;
        }
        Command::Tm3Migrate { action } => {
            handle_tm3_migrate(action)?;
        }
        Command::Outcomes { action } => {
            handle_outcomes(action)?;
        }
        Command::Tm3Diary { action } => {
            handle_tm3_diary(action)?;
        }
    }

    Ok(())
}

fn handle_registry(action: RegistryAction) -> anyhow::Result<()> {
    use registry::config::RegistryConfig;

    match action {
        RegistryAction::Init => {
            registry::config::init_registry()?;
        }
        RegistryAction::Create { remote } => {
            let config = RegistryConfig::load();
            let repo_path = &config.local_path;

            if repo_path.join(".git").exists() {
                println!("Registry already exists at {}", repo_path.display());
                return Ok(());
            }

            if let Some(url) = &remote {
                println!("Cloning registry from {}...", url);
                registry::repo::clone_repo(url, repo_path)?;
                println!("Cloned to {}", repo_path.display());
            } else {
                println!("Creating new registry at {}...", repo_path.display());
                registry::repo::init_repo(repo_path)?;
                println!("Registry created.");
            }

            // Add remote if configured but not yet set
            if remote.is_none() && !config.remote_url.is_empty() {
                registry::repo::add_remote(repo_path, &config.remote_url)?;
                println!("Remote added: {}", config.remote_url);
            }
        }
        RegistryAction::Sync => {
            let config = RegistryConfig::load();
            if !config.enabled {
                println!("Registry is not enabled. Run `practiceforge registry init` first.");
                return Ok(());
            }
            let summary = registry::sync::sync(&config)?;
            println!("{}", summary);
            registry::sync::mark_synced(&config)?;
        }
        RegistryAction::List { status } => {
            let config = RegistryConfig::load();
            let clients = registry::list_clients(&config)?;

            let filtered: Vec<_> = if status == "all" {
                clients.iter().collect()
            } else {
                clients.iter().filter(|c| c.status == status).collect()
            };

            if filtered.is_empty() {
                println!("No {} clients in registry.", status);
                return Ok(());
            }

            println!("{} client(s) ({}):\n", filtered.len(), status);
            for client in &filtered {
                println!("{:<6} {:<35} {:<12} {}",
                    client.client_id,
                    client.name,
                    client.funding.funding_type.as_deref().unwrap_or("-"),
                    client.status,
                );
            }
        }
        RegistryAction::Get { client_id } => {
            let config = RegistryConfig::load();
            let client = registry::get_client(&config, &client_id)?;
            println!("{}", registry::client::format_client(&client));

            let assignments = registry::client::get_assignments(&config, &client_id)?;
            if !assignments.is_empty() {
                println!("\n  Practitioners:");
                for a in &assignments {
                    println!("    {} (since {}){}",
                        a.practitioner_id,
                        a.since,
                        if a.primary { " [primary]" } else { "" },
                    );
                }
            }
        }
        RegistryAction::Import { client_id, all } => {
            let config = RegistryConfig::load();
            let clinical_root = crate::config::clinical_root();

            if !config.local_path.join(".git").exists() {
                println!("Registry not initialised. Run `practiceforge registry create` first.");
                return Ok(());
            }

            if let Some(id) = client_id {
                println!("Importing {}...", id);
                registry::import::import_client(&config, &id, &clinical_root)?;
                registry::repo::add_and_commit(
                    &config.local_path,
                    &[&format!("clients/{}/", id)],
                    &format!("Import client {}", id),
                )?;
                println!("Imported {}", id);
            } else if all {
                println!("Importing all clients from {}...", clinical_root.display());
                let (imported, skipped, errors) =
                    registry::import::import_all(&config, &clinical_root)?;
                println!(
                    "\nDone: {} imported, {} skipped (already exist), {} errors",
                    imported, skipped, errors
                );
            } else {
                println!("Specify a client ID or use --all");
            }
        }
        RegistryAction::Status => {
            let config = RegistryConfig::load();
            registry::sync::show_status(&config)?;
        }
        RegistryAction::PushLetter { client_id, path } => {
            let config = RegistryConfig::load();
            let src = std::path::PathBuf::from(&path);
            if !src.exists() {
                anyhow::bail!("File not found: {}", path);
            }

            let filename = src.file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "letter.pdf".to_string());

            let dst_dir = config.client_dir(&client_id).join("letters");
            std::fs::create_dir_all(&dst_dir)?;
            let dst = dst_dir.join(&filename);
            std::fs::copy(&src, &dst)?;

            let relative = format!("clients/{}/letters/{}", client_id, filename);
            registry::sync::commit_file(
                &config,
                &relative,
                &format!("Add letter {} for {}", filename, client_id),
            )?;

            println!("Letter pushed to registry: {}", relative);
        }
    }

    Ok(())
}

async fn handle_sms(action: SmsAction) -> anyhow::Result<()> {
    let config = sms::SmsConfig::load();

    match action {
        SmsAction::Preview { date } => {
            let previews = sms::remind::preview_reminders(&config, date.as_deref())?;
            if previews.is_empty() {
                println!("No reminders to send.");
                return Ok(());
            }

            println!(
                "{} reminder(s) for {}:\n",
                previews.len(),
                previews[0].appointment_date
            );
            for p in &previews {
                println!(
                    "  {} ({}) -> {} at {}",
                    p.client_name,
                    p.client_id,
                    p.phone,
                    p.appointment_time.format("%H:%M"),
                );
                println!("    \"{}\"", p.message_text);
                println!();
            }
        }

        SmsAction::Send { date } => {
            let results = sms::remind::send_reminders(&config, date.as_deref()).await?;
            let sent = results.iter().filter(|r| r.error_message.is_none()).count();
            let failed = results.iter().filter(|r| r.error_message.is_some()).count();
            println!(
                "\nDone: {} sent, {} failed.",
                sent, failed
            );
        }

        SmsAction::Status { date } => {
            sms::remind::show_status(&config, date.as_deref())?;
        }

        SmsAction::Test { phone, message } => {
            if !config.enabled {
                println!("Warning: SMS is not enabled in config, but sending test anyway.");
            }

            if config.twilio_account_sid.is_empty() || config.resolve_auth_token().is_empty() {
                anyhow::bail!(
                    "Twilio credentials not configured. Set twilio_account_sid and twilio_auth_token in [sms] section of {}",
                    crate::config::config_file_path().display()
                );
            }

            println!("Sending test SMS to {}...", phone);
            let result = sms::twilio::send_sms(&config, &phone, &message).await?;

            if let Some(err) = &result.error_message {
                println!("Failed: {}", err);
            } else {
                println!("Sent. SID: {} Status: {}", result.message_sid, result.status);
            }
        }
    }

    Ok(())
}

fn handle_search(
    query: String,
    client: Option<String>,
    field: Option<String>,
    limit: usize,
    reindex: bool,
) -> anyhow::Result<()> {
    use search::config::SearchConfig;
    use search::index;
    use search::query as sq;

    let config = SearchConfig::load();
    let clinical_root = index::resolve_clinical_root();

    if reindex {
        eprintln!("Rebuilding search index...");
        index::build_index(&config, &clinical_root)?;
        if query.is_empty() {
            return Ok(());
        }
    } else {
        // Auto-rebuild if stale (> 1 hour)
        let max_age = std::time::Duration::from_secs(3600);
        if index::is_index_stale(&config, max_age) {
            eprintln!("Search index is stale — rebuilding...");
            index::build_index(&config, &clinical_root)?;
        }
    }

    let results = if let Some(ref client_id) = client {
        sq::search_within_client(&config, client_id, &query)?
    } else if let Some(ref field_name) = field {
        sq::search_field(&config, &query, field_name, limit)?
    } else {
        sq::search(&config, &query, limit)?
    };

    if results.is_empty() {
        println!("No results for '{}'.", query);
        return Ok(());
    }

    println!("{} result(s) for '{}':\n", results.len(), query);
    for (i, result) in results.iter().enumerate() {
        println!(
            "  {}. {} ({}) — score {:.2}",
            i + 1,
            result.name,
            result.client_id,
            result.score,
        );
        if !result.snippet.is_empty() {
            // Strip HTML tags from snippet for terminal display
            let plain = result
                .snippet
                .replace("<b>", "\x1b[1m")
                .replace("</b>", "\x1b[0m");
            println!("     {}", plain);
        }
        println!();
    }

    Ok(())
}

async fn handle_inference(action: InferencePodAction) -> anyhow::Result<()> {
    use runpod::Client as RunPodClient;

    match action {
        InferencePodAction::List => {
            let client = RunPodClient::new()?;
            let pods = client.list_pods().await?;
            if pods.is_empty() {
                println!("No pods on this account.");
                return Ok(());
            }
            println!("{:<25} {:<20} {:<12} {:>8}  {}", "ID", "NAME", "STATUS", "$/hr", "GPU");
            println!("{}", "-".repeat(80));
            for pod in &pods {
                println!(
                    "{:<25} {:<20} {:<12} {:>8.4}  {}",
                    pod.id,
                    trunc(&pod.name, 20),
                    pod.desired_status,
                    pod.cost_per_hr,
                    pod.gpu_count,
                );
            }
        }
        InferencePodAction::Volumes => {
            let client = RunPodClient::new()?;
            let vols = client.list_network_volumes().await?;
            if vols.is_empty() {
                println!("No network volumes on this account.");
                return Ok(());
            }
            println!("{:<30} {:<25} {:>6} GB  {}", "ID", "NAME", "SIZE", "DC");
            println!("{}", "-".repeat(80));
            for vol in &vols {
                println!(
                    "{:<30} {:<25} {:>6}     {}",
                    vol.id, trunc(&vol.name, 25), vol.size, vol.data_center_id
                );
            }
        }
        InferencePodAction::Status => {
            let config = inference::load_pod_config()?;
            let state = inference::load_state();

            println!("Inference pod configuration:");
            println!("  Managed by The Product: {}", config.managed);
            println!(
                "  Pod ID:                 {}",
                if config.pod_id.is_empty() {
                    "(not set)".to_string()
                } else {
                    config.pod_id.clone()
                }
            );
            println!(
                "  Network volume:         {}",
                if config.network_volume_id.is_empty() {
                    "(not set)".to_string()
                } else {
                    config.network_volume_id.clone()
                }
            );
            println!(
                "  Idle timeout:           {} min",
                config.idle_timeout_minutes.unwrap_or(15)
            );
            println!();

            if !config.has_pod() {
                println!("No managed pod configured — nothing to query.");
                println!("See {} to configure.", inference::config_path().display());
                return Ok(());
            }

            let client = RunPodClient::new()?;
            match client.get_pod(&config.pod_id).await {
                Ok(pod) => {
                    println!("Live pod state:");
                    println!("  Name:          {}", pod.name);
                    println!("  Status:        {}", pod.desired_status);
                    println!("  Cost/hour:     ${:.4}", pod.cost_per_hr);
                    println!("  GPUs:          {}", pod.gpu_count);
                    println!("  Image:         {}", pod.image_name);
                    if let Some(ip) = &pod.public_ip {
                        println!("  Public IP:     {}", ip);
                    }
                    if !pod.ports.is_empty() {
                        println!("  Ports:         {}", pod.ports.join(", "));
                    }
                }
                Err(e) => {
                    println!("Error fetching pod state: {}", e);
                }
            }

            println!();
            println!("Local state:");
            println!(
                "  Last activity: {}",
                state.last_activity.as_deref().unwrap_or("(none)")
            );
        }
        InferencePodAction::Start => {
            let config = inference::load_pod_config()?;
            if !config.has_pod() {
                anyhow::bail!(
                    "No managed pod configured. Set [pod] managed=true and pod_id in {}",
                    inference::config_path().display()
                );
            }
            let client = RunPodClient::new()?;
            let started = inference::ensure_running(&client, &config).await?;
            if started {
                println!("Pod started.");
            } else {
                println!("Pod was already running.");
            }
        }
        InferencePodAction::Stop => {
            let config = inference::load_pod_config()?;
            if !config.has_pod() {
                anyhow::bail!(
                    "No managed pod configured. Set [pod] managed=true and pod_id in {}",
                    inference::config_path().display()
                );
            }
            let client = RunPodClient::new()?;
            let stopped = inference::ensure_stopped(&client, &config).await?;
            if stopped {
                println!("Pod stopped.");
            } else {
                println!("Pod was already stopped.");
            }
        }
        InferencePodAction::Maintain => {
            // Idle-timeout sweeper: check if pod is running AND idle-for-long-enough.
            // If so, stop it. Intended to be called periodically (cron, launchd, etc.)
            // from cross-platform schedulers the user configures themselves.
            let config = inference::load_pod_config()?;
            if !config.has_pod() {
                println!("No managed pod — nothing to maintain.");
                return Ok(());
            }
            let state = inference::load_state();
            let timeout = config.idle_timeout();
            if !inference::is_idle(&state, timeout) {
                println!(
                    "Pod not idle (last activity within {} min). No action.",
                    timeout.as_secs() / 60
                );
                return Ok(());
            }
            let client = RunPodClient::new()?;
            let pod = client.get_pod(&config.pod_id).await?;
            if !pod.is_running() {
                println!("Pod already stopped. No action.");
                return Ok(());
            }
            println!(
                "Pod has been idle > {} min. Stopping...",
                timeout.as_secs() / 60
            );
            client.stop_pod(&config.pod_id).await?;
            println!("Pod stopped.");
        }
    }

    Ok(())
}

fn handle_billing(action: BillingAction) -> anyhow::Result<()> {
    use billing::{
        config::BillingConfig,
        invoice::{self, build_invoice},
        manual::ManualProvider,
        remind,
        sessions::{
            billable_sessions_for_client, default_schedules_dir, default_session_dir,
            uninvoiced_billable, BillReason, BillableSession,
        },
        status,
        traits::{AccountingProvider, InvoiceFilter},
    };

    // Init, Config, and credential-setup commands work without billing being enabled
    match &action {
        BillingAction::Init => {
            return billing::config::init_billing();
        }
        BillingAction::Config { setting } => {
            return if let Some(s) = setting {
                billing::config::update_config(s)
            } else {
                billing::config::show_config()
            };
        }
        BillingAction::XeroSetup { client_id, client_secret } => {
            let mut secrets = billing::BillingSecrets::load()?;
            secrets.xero.client_id = Some(client_id.clone());
            secrets.xero.client_secret = Some(client_secret.clone());
            secrets.save()?;
            println!("✓ Xero credentials saved. Run 'billing xero-auth' to authorize.");
            return Ok(());
        }
        BillingAction::XeroAuth => {
            let secrets = billing::BillingSecrets::load()?;
            let client_id = secrets
                .xero
                .client_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("Run 'billing xero-setup <client_id> <client_secret>' first"))?
                .to_string();
            let (auth_url, _state, verifier) =
                billing::XeroProvider::auth_url(&client_id)?;
            println!("Open this URL in your browser to authorize Xero:");
            println!("\n  {}\n", auth_url);
            let code = billing::xero::run_oauth_callback_server(8765)?;
            let client_secret = secrets
                .xero
                .client_secret
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("client_secret not set"))?
                .to_string();
            let tenant_name =
                billing::XeroProvider::auth_complete(&code, &verifier, &client_id, &client_secret)?;
            println!("✓ Authorized. Connected to Xero tenant: {}", tenant_name);
            return Ok(());
        }
        BillingAction::StripeKey { secret_key } => {
            let mut secrets = billing::BillingSecrets::load()?;
            secrets.stripe.secret_key = Some(secret_key.clone());
            secrets.save()?;
            println!("✓ Stripe secret key saved.");
            return Ok(());
        }
        _ => {}
    }

    let config = BillingConfig::load()?;

    if !config.enabled {
        match action {
            BillingAction::Status { .. } => {
                println!("Billing is not enabled.");
                println!("Run 'practiceforge billing init' to set up, or add [billing] enabled = true to config.toml.");
                println!(
                    "Config file: {}",
                    crate::config::config_file_path().display()
                );
                return Ok(());
            }
            _ => {
                anyhow::bail!(
                    "Billing is not enabled. Run 'practiceforge billing init' or add [billing] enabled = true to {}",
                    crate::config::config_file_path().display()
                );
            }
        }
    }

    // Select accounting provider based on config
    let provider: Box<dyn billing::traits::AccountingProvider> = match config.provider.as_str() {
        "xero" => Box::new(billing::XeroProvider::new()?),
        _ => Box::new(ManualProvider::new(&config)?),
    };

    // Select payment provider based on config
    let payment: Box<dyn billing::traits::PaymentProvider> = match config.payment_provider.as_str() {
        "stripe" => Box::new(billing::StripeProvider::new()?),
        _ => Box::new(ManualProvider::new(&config)?),
    };

    match action {
        BillingAction::Status { overdue } => {
            status::show_status(provider.as_ref(), overdue)?;
        }

        BillingAction::Invoice { client_id, dates, send } => {
            let clients_dir = crate::config::clients_dir();
            let client_dir = clients_dir.join(&client_id);

            if !client_dir.exists() {
                anyhow::bail!("Client directory not found: {}", client_dir.display());
            }

            let identity_path = client_dir.join("identity.yaml");
            if !identity_path.exists() {
                anyhow::bail!(
                    "No identity.yaml for client {}",
                    client_id
                );
            }

            let uninvoiced: Vec<BillableSession> = if let Some(dates_str) = dates {
                dates_str
                    .split(',')
                    .map(|s| BillableSession {
                        date: s.trim().to_string(),
                        reason: BillReason::Attended,
                    })
                    .collect()
            } else {
                let all_sessions = billable_sessions_for_client(
                    &client_id,
                    &default_session_dir(),
                    Some(&default_schedules_dir()),
                )?;
                let already_invoiced = provider.invoiced_dates_for_client(&client_id)?;
                let sessions = uninvoiced_billable(&all_sessions, &already_invoiced);
                if sessions.is_empty() {
                    println!("No uninvoiced sessions for {}.", client_id);
                    return Ok(());
                }
                sessions
            };

            let reference = provider.next_invoice_number()?;
            let inv = build_invoice(
                reference,
                &client_id,
                &identity_path,
                &uninvoiced,
                config.payment_terms_days,
                &config.currency,
            )?;

            println!(
                "Creating invoice {} for {} ({} session{}, {} {:.2})...",
                inv.reference,
                inv.client_name,
                inv.line_items.len(),
                if inv.line_items.len() == 1 { "" } else { "s" },
                inv.currency,
                inv.total()
            );

            // Generate payment link for self-pay clients
            let payment_link = if matches!(inv.bill_to, billing::BillTo::Client { .. }) {
                payment.create_payment_link(&inv).unwrap_or(None)
            } else {
                None
            };
            let mut inv = inv;
            inv.payment_link = payment_link;

            let result = provider.create_invoice(&inv)?;
            println!("✓ Invoice {} created.", result.reference);
            if let Some(path) = &result.file_path {
                println!("  File: {}", path);
            }
            if let Some(link) = &inv.payment_link {
                println!("  Payment link: {}", link);
            }

            if send {
                let prac = billing::practitioner::PractitionerConfig::load();
                let email_cfg = crate::email::load_email_config()?;
                billing::invoice_render::send_invoice(&inv, &prac, &email_cfg)?;
                println!("✓ Invoice emailed to {}.", inv.bill_to.email().unwrap_or(""));
            }
        }

        BillingAction::InvoiceBatch { dry_run, send } => {
            let clients_dir = crate::config::clients_dir();
            if !clients_dir.exists() {
                anyhow::bail!("Clients directory not found: {}", clients_dir.display());
            }

            let mut total_created = 0u32;
            let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(&clients_dir)?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .collect();
            entries.sort_by_key(|e| e.file_name());

            for entry in entries {
                let client_id = entry.file_name().to_string_lossy().to_string();
                let client_dir = entry.path();
                let identity_path = client_dir.join("identity.yaml");

                if !identity_path.exists() {
                    continue;
                }

                // Check if client has a rate configured
                let id_content = std::fs::read_to_string(&identity_path).unwrap_or_default();
                let identity: serde_yaml::Value =
                    match serde_yaml::from_str(&id_content) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                let has_rate = identity
                    .get("funding")
                    .and_then(|f| f.get("rate"))
                    .and_then(|r| invoice::parse_rate(r))
                    .map(|r| r > 0.0)
                    .unwrap_or(false);

                if !has_rate {
                    continue;
                }

                let all_sessions = match billable_sessions_for_client(
                    &client_id,
                    &default_session_dir(),
                    Some(&default_schedules_dir()),
                ) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let already_invoiced =
                    provider.invoiced_dates_for_client(&client_id).unwrap_or_default();
                let uninvoiced = uninvoiced_billable(&all_sessions, &already_invoiced);

                if uninvoiced.is_empty() {
                    continue;
                }

                if dry_run {
                    let name = identity
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&client_id);
                    println!(
                        "  {} ({}) — {} uninvoiced session{}",
                        client_id,
                        name,
                        uninvoiced.len(),
                        if uninvoiced.len() == 1 { "" } else { "s" }
                    );
                } else {
                    let reference = provider.next_invoice_number()?;
                    match build_invoice(
                        reference,
                        &client_id,
                        &identity_path,
                        &uninvoiced,
                        config.payment_terms_days,
                        &config.currency,
                    ) {
                        Ok(inv) => {
                            let result = provider.create_invoice(&inv)?;
                            println!(
                                "  ✓ {} — {} ({} session{}, {} {:.2})",
                                result.reference,
                                inv.client_name,
                                inv.line_items.len(),
                                if inv.line_items.len() == 1 { "" } else { "s" },
                                inv.currency,
                                inv.total()
                            );
                            total_created += 1;
                            if send {
                                let prac = billing::practitioner::PractitionerConfig::load();
                                let email_cfg = crate::email::load_email_config()?;
                                match billing::invoice_render::send_invoice(&inv, &prac, &email_cfg) {
                                    Ok(()) => println!("    ✓ Emailed to {}.", inv.bill_to.email().unwrap_or("")),
                                    Err(e) => eprintln!("    ✗ Email failed: {}", e),
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("  ✗ {} — {}", client_id, e);
                        }
                    }
                }
            }

            if dry_run {
                println!("\nDry run — no invoices created. Remove --dry-run to create.");
            } else {
                println!("\n{} invoice(s) created.", total_created);
            }
        }

        BillingAction::Paid { reference, date } => {
            let date = date.unwrap_or_else(|| {
                chrono::Local::now().format("%Y-%m-%d").to_string()
            });
            provider.mark_paid(&reference, &date, None)?;
            println!("✓ {} marked as paid ({})", reference, date);
        }

        BillingAction::Cancel { reference, reason } => {
            provider.cancel_invoice(&reference, &reason)?;
            println!("✓ {} cancelled: {}", reference, reason);
        }

        BillingAction::Remind { send } => {
            let overdue = provider.list_invoices(InvoiceFilter {
                overdue_only: true,
                ..Default::default()
            })?;

            if overdue.is_empty() {
                println!("No overdue invoices — no reminders needed.");
                return Ok(());
            }

            let due = remind::due_reminders(&config, &overdue);

            if due.is_empty() {
                println!(
                    "{} overdue invoice(s), but all reminders already sent.",
                    overdue.len()
                );
                return Ok(());
            }

            // Load email config (needed for practitioner name and sending)
            let email_config = crate::email::load_email_config();
            let practitioner = email_config
                .as_ref()
                .map(|c| c.from_name.clone())
                .unwrap_or_else(|_| "The Practitioner".to_string());

            let mut sent_count = 0u32;
            let mut failed_count = 0u32;

            for (inv, tone) in &due {
                let is_insurer = !inv.bill_to_name.is_empty()
                    && inv.bill_to_name != inv.client_name;

                let reminder = if is_insurer {
                    remind::render_insurer_reminder(inv, &practitioner)
                } else {
                    remind::render_client_reminder(inv, tone, &practitioner, "")
                };

                if send {
                    let to_email = match &reminder.to_email {
                        Some(email) => email.clone(),
                        None => {
                            eprintln!(
                                "  ✗ {} — no email address for {}. Skipping.",
                                inv.reference, reminder.to_name
                            );
                            failed_count += 1;
                            continue;
                        }
                    };

                    let email_cfg = match &email_config {
                        Ok(cfg) => cfg,
                        Err(_) => {
                            eprintln!(
                                "  ✗ Email not configured. Run: practiceforge email init"
                            );
                            return Ok(());
                        }
                    };

                    match crate::email::send_email(
                        email_cfg,
                        &to_email,
                        &reminder.to_name,
                        &reminder.subject,
                        &reminder.body,
                        None,
                        None,
                    ) {
                        Ok(()) => {
                            // Log the sent reminder (always local, regardless of provider)
                            let now = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
                            let log_entry = crate::billing::ReminderLogEntry {
                                reference: inv.reference.clone(),
                                sent_at: now,
                                tone: tone.clone(),
                                to_email: to_email.clone(),
                                to_name: reminder.to_name.clone(),
                            };
                            if let Ok(manual) = ManualProvider::new(&config) {
                                if let Err(e) = manual.log_reminder(log_entry) {
                                    eprintln!("  ⚠ Sent but failed to log: {}", e);
                                }
                            }

                            println!(
                                "  ✓ {} → {} <{}> [{}]",
                                inv.reference, reminder.to_name, to_email, tone
                            );
                            sent_count += 1;
                        }
                        Err(e) => {
                            eprintln!(
                                "  ✗ {} → {} — send failed: {}",
                                inv.reference, reminder.to_name, e
                            );
                            failed_count += 1;
                        }
                    }
                } else {
                    // Dry-run: preview the reminder
                    let email_display = reminder
                        .to_email
                        .as_deref()
                        .unwrap_or("(no email)");
                    println!("--- {} ({}) ---", inv.reference, tone);
                    println!("To: {} <{}>", reminder.to_name, email_display);
                    println!("Subject: {}", reminder.subject);
                    println!();
                    println!("{}", reminder.body);
                    println!();
                }
            }

            if send {
                println!(
                    "\n{} sent, {} failed.",
                    sent_count, failed_count
                );
            } else {
                println!(
                    "{} reminder(s) ready. Use --send to deliver via email.",
                    due.len()
                );
            }
        }

        BillingAction::Maintain => {
            let summary = status::compact_summary(provider.as_ref())?;
            println!("Billing: {}", summary);

            let overdue = provider.list_invoices(InvoiceFilter {
                overdue_only: true,
                ..Default::default()
            })?;

            if !overdue.is_empty() {
                let due = remind::due_reminders(&config, &overdue);
                if !due.is_empty() {
                    println!(
                        "  {} reminder(s) pending. Run: practiceforge billing remind",
                        due.len()
                    );
                }

                // Flag seriously overdue (>28 days) for DayPage alert
                let serious: Vec<_> = overdue
                    .iter()
                    .filter(|i| i.days_overdue > 28)
                    .collect();

                if !serious.is_empty() {
                    let alert = serious
                        .iter()
                        .map(|i| {
                            format!(
                                "{} ({}, {}d overdue)",
                                i.reference, i.client_name, i.days_overdue
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("; ");
                    eprintln!(
                        "  ⚠ Seriously overdue: {}",
                        alert
                    );
                }
            }
        }

        // Already handled above, before the enabled check
        BillingAction::Init
        | BillingAction::Config { .. }
        | BillingAction::XeroSetup { .. }
        | BillingAction::XeroAuth
        | BillingAction::StripeKey { .. } => unreachable!(),
    }

    Ok(())
}

fn handle_tm3_migrate(action: Tm3MigrateAction) -> anyhow::Result<()> {
    use registry::config::RegistryConfig;

    let registry_config = RegistryConfig::load();
    let clinical_root = crate::config::clinical_root();

    match action {
        Tm3MigrateAction::ExportClients { dry_run } => {
            println!("=== TM3 Migration: Export Clients ===\n");
            let report = tm3_migrate::clients::export_clients(
                &registry_config,
                &clinical_root,
                dry_run,
            )?;
            println!("\n{}", report);
            if !report.warnings.is_empty() {
                println!("\nWarnings:");
                for w in &report.warnings {
                    println!("  {}", w);
                }
            }
            if !report.errors.is_empty() {
                println!("\nErrors:");
                for e in &report.errors {
                    println!("  {}", e);
                }
            }
        }
        Tm3MigrateAction::ExportCalendar { dry_run } => {
            println!("=== TM3 Migration: Export Calendar ===\n");
            let schedules_dir = shellexpand::tilde("~/Clinical/schedules").to_string();
            let report = tm3_migrate::calendar::export_calendar(
                std::path::Path::new(&schedules_dir),
                dry_run,
            )?;
            println!("\n{}", report);
        }
        Tm3MigrateAction::ExportDocuments { dry_run } => {
            println!("=== TM3 Migration: Export Documents ===\n");
            let report = tm3_migrate::documents::export_documents(
                &registry_config,
                dry_run,
            )?;
            println!("\n{}", report);
            if !report.failed.is_empty() {
                println!("\nFailures:");
                for f in &report.failed {
                    println!("  {}", f);
                }
            }
        }
        Tm3MigrateAction::Validate => {
            println!("=== TM3 Migration: Validate ===\n");
            let report = tm3_migrate::validate::validate(
                &registry_config,
                &clinical_root,
            )?;
            println!("{}", report);

            if !report.missing.is_empty() {
                println!("\nMissing from registry (in TM3 but not imported):");
                for m in &report.missing {
                    println!("  TM3 #{}: {}", m.tm3_id, m.name);
                }
            }
            if !report.extra.is_empty() {
                println!("\nExtra in registry (not in TM3 cache):");
                for e in &report.extra {
                    println!("  {}", e);
                }
            }
            if !report.mismatches.is_empty() {
                println!("\nField mismatches:");
                for m in &report.mismatches {
                    println!("  {}", m);
                }
            }
            if !report.missing_documents.is_empty() {
                println!("\nClients missing documents:");
                for d in &report.missing_documents {
                    println!("  {}", d);
                }
            }
        }
        Tm3MigrateAction::Run { dry_run } => {
            println!("=== TM3 Full Migration ===\n");

            if dry_run {
                println!("[DRY RUN — no changes will be made]\n");
            }

            // Step 1: Export clients
            println!("--- Step 1/4: Export Clients ---\n");
            let client_report = tm3_migrate::clients::export_clients(
                &registry_config,
                &clinical_root,
                dry_run,
            )?;
            println!("{}\n", client_report);

            // Step 2: Export calendar
            println!("--- Step 2/4: Export Calendar ---\n");
            let schedules_dir = shellexpand::tilde("~/Clinical/schedules").to_string();
            let calendar_report = tm3_migrate::calendar::export_calendar(
                std::path::Path::new(&schedules_dir),
                dry_run,
            )?;
            println!("{}\n", calendar_report);

            // Step 3: Export documents
            println!("--- Step 3/4: Export Documents ---\n");
            let doc_report = tm3_migrate::documents::export_documents(
                &registry_config,
                dry_run,
            )?;
            println!("{}\n", doc_report);

            // Step 4: Validate
            println!("--- Step 4/4: Validate ---\n");
            let validation_report = tm3_migrate::validate::validate(
                &registry_config,
                &clinical_root,
            )?;
            println!("{}\n", validation_report);

            // Summary
            println!("=== Migration Summary ===");
            println!("  {}", client_report);
            println!("  {}", calendar_report);
            println!("  {}", doc_report);
            println!(
                "  Validation: {} missing, {} extra, {} mismatches",
                validation_report.missing.len(),
                validation_report.extra.len(),
                validation_report.mismatches.len(),
            );
        }
    }

    Ok(())
}

fn trunc(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n.saturating_sub(1)])
    }
}

/// Look up a client's name. Tries registry first, then identity.yaml, then falls
/// back to using the client_id itself.
fn lookup_client_name(client_id: &str) -> String {
    // Try registry
    let reg_config = registry::config::RegistryConfig::load();
    if let Ok(client) = registry::get_client(&reg_config, client_id) {
        return client.name;
    }

    // Fall back to ~/Clinical/clients/{id}/identity.yaml
    let identity_path = crate::config::clients_dir().join(client_id).join("identity.yaml");
    if identity_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&identity_path) {
            if let Ok(val) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
                if let Some(name) = val.get("name").and_then(|v| v.as_str()) {
                    return name.to_string();
                }
            }
        }
    }

    // Last resort
    client_id.to_string()
}

fn handle_schedule(action: ScheduleAction) -> anyhow::Result<()> {
    use chrono::{Datelike, Local, NaiveDate};
    use scheduling::{
        ics, models::*, recurrence,
    };

    // Resolve schedules directory from config (expand ~)
    let config = scheduling::config::SchedulingConfig::default();
    let schedules_dir = shellexpand::tilde(&config.schedules_dir).to_string();
    let practitioner = &config.default_practitioner;

    match action {
        ScheduleAction::List { date, week, practitioner: prac_filter } => {
            let prac = prac_filter.as_deref().unwrap_or(practitioner);
            let base_date = match date {
                Some(ref d) => NaiveDate::parse_from_str(d, "%Y-%m-%d")
                    .map_err(|e| anyhow::anyhow!("Invalid date '{}': {}", d, e))?,
                None => Local::now().date_naive(),
            };

            let (from, to) = if week {
                let weekday = base_date.weekday().num_days_from_monday();
                let monday = base_date - chrono::Duration::days(weekday as i64);
                let friday = monday + chrono::Duration::days(4);
                (monday, friday)
            } else {
                (base_date, base_date)
            };

            // Load series for this practitioner
            let series_dir = std::path::PathBuf::from(&schedules_dir)
                .join(prac)
                .join("series");
            let series_list = ics::load_series_dir(&series_dir)?;

            // Load holidays
            let holidays_path = std::path::PathBuf::from(&schedules_dir)
                .join(prac)
                .join("holidays.yaml");
            let holidays = if holidays_path.exists() {
                let yaml = std::fs::read_to_string(&holidays_path)?;
                ics::load_holidays(&yaml)?
            } else {
                vec![]
            };

            // Load one-off appointments
            let appts_dir = std::path::PathBuf::from(&schedules_dir)
                .join(prac)
                .join("appointments");
            let one_offs = ics::load_appointments_dir(&appts_dir)?;

            if series_list.is_empty() && one_offs.is_empty() {
                println!("No appointments found for '{}'.", prac);
                println!("  Series dir: {}", series_dir.display());
                return Ok(());
            }

            // Materialise recurring series into date entries
            // Each entry: (date, start_time, end_time, client_name, client_id, rate_tag, description)
            let mut all_entries: Vec<(NaiveDate, chrono::NaiveTime, chrono::NaiveTime, String, String, Option<String>, String)> = Vec::new();

            for s in &series_list {
                if s.status != SeriesStatus::Active {
                    continue;
                }
                let dates = recurrence::materialise(s, from, to, &holidays)?;
                for d in dates {
                    all_entries.push((
                        d,
                        s.start_time,
                        s.end_time,
                        s.client_name.clone(),
                        s.client_id.clone(),
                        s.rate_tag.clone(),
                        format!("{}", s.recurrence.freq),
                    ));
                }
            }

            // Add one-off appointments within the date range
            for appt in &one_offs {
                if appt.date >= from && appt.date <= to && appt.status != AppointmentStatus::Cancelled {
                    all_entries.push((
                        appt.date,
                        appt.start_time,
                        appt.end_time,
                        appt.client_name.clone(),
                        appt.client_id.clone(),
                        appt.rate_tag.clone(),
                        format!("one-off [{}]", appt.status),
                    ));
                }
            }

            all_entries.sort_by_key(|(d, st, _, _, _, _, _)| (*d, *st));

            if all_entries.is_empty() {
                println!("No appointments for {} ({}--{}).", prac, from, to);
            } else {
                let mut current_date = None;
                for (date, start_time, end_time, client_name, client_id, rate_tag, desc) in &all_entries {
                    if current_date != Some(*date) {
                        println!("\n{}  {}", date, date.format("%A"));
                        current_date = Some(*date);
                    }
                    let tag = rate_tag.as_deref().unwrap_or("");
                    let tag_str = if tag.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", tag)
                    };
                    println!(
                        "  {} -- {}  {} ({}){} -- {}",
                        start_time.format("%H:%M"),
                        end_time.format("%H:%M"),
                        client_name,
                        client_id,
                        tag_str,
                        desc,
                    );
                }
            }
        }

        ScheduleAction::Create {
            client_id,
            datetime,
            duration,
            recur,
            count,
            infinite,
        } => {
            // Parse datetime
            let dt = chrono::NaiveDateTime::parse_from_str(&datetime, "%Y-%m-%d %H:%M")
                .map_err(|e| anyhow::anyhow!("Invalid datetime '{}': {}", datetime, e))?;

            let start_time = dt.time();
            let end_time = start_time + chrono::Duration::minutes(duration as i64);
            let date = dt.date();

            let (freq, interval) = match recur.as_deref() {
                Some("weekly") => (Some(Frequency::Weekly), 1),
                Some("fortnightly") => (Some(Frequency::Weekly), 2),
                Some("every3w") => (Some(Frequency::Weekly), 3),
                Some("monthly") => (Some(Frequency::Monthly), 1),
                _ => (None, 1),
            };

            let client_name = lookup_client_name(&client_id);

            if let Some(freq) = freq {
                // Create a recurring series
                let series_count = if infinite { None } else { count };
                let series = RecurringSeries {
                    id: uuid::Uuid::new_v4(),
                    practitioner: practitioner.to_string(),
                    client_id: client_id.clone(),
                    client_name: client_name.clone(),
                    start_time,
                    end_time,
                    location: config.location.clone(),
                    modality: None,
                    rate_tag: None,
                    recurrence: RecurrenceRule {
                        freq: freq.clone(),
                        interval,
                        by_day: None,
                        dtstart: date,
                        until: None,
                        count: series_count,
                    },
                    exdates: vec![],
                    status: SeriesStatus::Active,
                    created_at: chrono::Utc::now().to_rfc3339(),
                    notes: None,
                };

                // Save to series directory
                let series_dir = std::path::PathBuf::from(&schedules_dir)
                    .join(practitioner)
                    .join("series");
                std::fs::create_dir_all(&series_dir)?;
                let path = series_dir.join(format!("{}.yaml", series.id));
                let yaml = serde_yaml::to_string(&series)?;
                std::fs::write(&path, &yaml)?;

                let recur_desc = if infinite || (count.is_none() && !infinite) {
                    format!("every {} {} (infinite)", interval,
                        if interval == 1 { format!("{}", freq) } else { "weeks".to_string() })
                } else {
                    format!("every {} {} ({} sessions)", interval,
                        if interval == 1 { format!("{}", freq) } else { "weeks".to_string() },
                        series_count.unwrap())
                };

                println!("Created recurring series: {} ({}) {} at {} -- {}",
                    client_name, client_id, date.format("%A"), start_time.format("%H:%M"), recur_desc);
                println!("  Series ID: {}", series.id);
                println!("  Saved to: {}", path.display());
            } else {
                // Create a one-off appointment
                let appt = Appointment {
                    id: uuid::Uuid::new_v4(),
                    series_id: None,
                    practitioner: practitioner.to_string(),
                    client_id: client_id.clone(),
                    client_name: client_name.clone(),
                    date,
                    start_time,
                    end_time,
                    status: AppointmentStatus::Confirmed,
                    source: AppointmentSource::Practitioner,
                    modality: None,
                    rate_tag: None,
                    location: config.location.clone(),
                    reschedule_for: None,
                    sms_confirmation: None,
                    notes: None,
                    created_at: chrono::Utc::now().to_rfc3339(),
                };

                let appts_dir = std::path::PathBuf::from(&schedules_dir)
                    .join(practitioner)
                    .join("appointments");
                std::fs::create_dir_all(&appts_dir)?;
                let path = appts_dir.join(format!("{}.yaml", appt.id));
                let yaml = serde_yaml::to_string(&appt)?;
                std::fs::write(&path, &yaml)?;

                println!("Created one-off appointment: {} ({}) on {} at {}",
                    client_name, client_id, date.format("%Y-%m-%d %A"), start_time.format("%H:%M"));
                println!("  Appointment ID: {}", appt.id);
                println!("  Saved to: {}", path.display());
            }
        }

        ScheduleAction::Blocks { client_id } => {
            // For now, scan blocks.yaml files under ~/Clinical/clients/
            let clinical_root = shellexpand::tilde("~/Clinical").to_string();
            let clients_dir = std::path::PathBuf::from(&clinical_root).join("clients");

            if !clients_dir.exists() {
                println!("No clients directory found at {}", clients_dir.display());
                return Ok(());
            }

            let mut found_any = false;
            for entry in std::fs::read_dir(&clients_dir)? {
                let entry = entry?;
                let blocks_path = entry.path().join("blocks.yaml");
                if !blocks_path.exists() {
                    continue;
                }

                let id = entry.file_name().to_string_lossy().to_string();
                if let Some(ref filter) = client_id {
                    if &id != filter {
                        continue;
                    }
                }

                let yaml = std::fs::read_to_string(&blocks_path)?;
                let blocks: Vec<AuthorisationBlock> = serde_yaml::from_str(&yaml)?;

                for block in &blocks {
                    found_any = true;
                    let remaining = block.remaining();
                    let warning = recurrence::check_block_expiry(block, config.blocks.warning_threshold);

                    let status_marker = if let Some(ref w) = warning {
                        if w.remaining == 0 { "⚠ EXHAUSTED" } else { "⚠ EXPIRING" }
                    } else {
                        "✓"
                    };

                    println!(
                        "  {} {} — {} {}/{} sessions ({} remaining) {}",
                        status_marker,
                        block.client_id,
                        block.insurer,
                        block.used_sessions,
                        block.authorised_sessions,
                        remaining,
                        block.status,
                    );

                    if let Some(w) = warning {
                        println!("    → {}", w.message);
                    }
                }
            }

            if !found_any {
                if let Some(ref id) = client_id {
                    println!("No authorisation blocks found for {}.", id);
                } else {
                    println!("No authorisation blocks found.");
                }
            }
        }

        ScheduleAction::Cancel { client_id, date, series } => {
            let series_dir = std::path::PathBuf::from(&schedules_dir)
                .join(practitioner)
                .join("series");
            let appts_dir = std::path::PathBuf::from(&schedules_dir)
                .join(practitioner)
                .join("appointments");

            if series {
                // Cancel the entire recurring series
                let all_series = ics::load_series_dir(&series_dir)?;
                let matching: Vec<_> = all_series.iter()
                    .filter(|s| s.client_id == client_id && s.status == SeriesStatus::Active)
                    .collect();

                if matching.is_empty() {
                    anyhow::bail!("No active recurring series found for client {}", client_id);
                }

                for s in &matching {
                    let mut updated = (*s).clone();
                    updated.status = SeriesStatus::Ended;
                    let path = series_dir.join(format!("{}.yaml", s.id));
                    let yaml = serde_yaml::to_string(&updated)?;
                    std::fs::write(&path, &yaml)?;
                    println!("Ended recurring series {} for {} ({})", s.id, s.client_name, s.client_id);
                }
            } else if let Some(date_str) = date {
                let cancel_date = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
                    .map_err(|e| anyhow::anyhow!("Invalid date '{}': {}", date_str, e))?;

                // First check one-off appointments for this client on this date
                let one_offs = ics::load_appointments_dir(&appts_dir)?;
                let matching_appt: Vec<_> = one_offs.iter()
                    .filter(|a| a.client_id == client_id && a.date == cancel_date && a.status != AppointmentStatus::Cancelled)
                    .collect();

                if !matching_appt.is_empty() {
                    for appt in &matching_appt {
                        let mut updated = (*appt).clone();
                        updated.status = AppointmentStatus::Cancelled;
                        let path = appts_dir.join(format!("{}.yaml", appt.id));
                        let yaml = serde_yaml::to_string(&updated)?;
                        std::fs::write(&path, &yaml)?;
                        println!("Cancelled one-off appointment {} on {} for {} ({})",
                            appt.id, cancel_date, appt.client_name, appt.client_id);
                    }
                } else {
                    // Add EXDATE to the matching recurring series
                    let all_series = ics::load_series_dir(&series_dir)?;
                    let matching: Vec<_> = all_series.iter()
                        .filter(|s| s.client_id == client_id && s.status == SeriesStatus::Active)
                        .collect();

                    if matching.is_empty() {
                        anyhow::bail!("No active series or one-off appointment found for {} on {}", client_id, cancel_date);
                    }

                    for s in &matching {
                        let mut updated = (*s).clone();
                        if !updated.exdates.contains(&cancel_date) {
                            updated.exdates.push(cancel_date);
                            updated.exdates.sort();
                        }
                        let path = series_dir.join(format!("{}.yaml", s.id));
                        let yaml = serde_yaml::to_string(&updated)?;
                        std::fs::write(&path, &yaml)?;
                        println!("Added EXDATE {} to series {} for {} ({})",
                            cancel_date, s.id, s.client_name, s.client_id);
                    }
                }
            } else {
                anyhow::bail!("Specify --date YYYY-MM-DD to cancel a single instance, or --series to end the recurring series");
            }
        }

        ScheduleAction::Move { client_id, from, to } => {
            let from_date = NaiveDate::parse_from_str(&from, "%Y-%m-%d")
                .map_err(|e| anyhow::anyhow!("Invalid from date '{}': {}", from, e))?;
            let to_dt = chrono::NaiveDateTime::parse_from_str(&to, "%Y-%m-%d %H:%M")
                .map_err(|e| anyhow::anyhow!("Invalid to datetime '{}': {}", to, e))?;

            let series_dir = std::path::PathBuf::from(&schedules_dir)
                .join(practitioner)
                .join("series");
            let appts_dir = std::path::PathBuf::from(&schedules_dir)
                .join(practitioner)
                .join("appointments");

            // Check if moving a one-off appointment
            let one_offs = ics::load_appointments_dir(&appts_dir)?;
            let matching_appt: Vec<_> = one_offs.iter()
                .filter(|a| a.client_id == client_id && a.date == from_date && a.status != AppointmentStatus::Cancelled)
                .collect();

            if !matching_appt.is_empty() {
                // Move the one-off: cancel the old, create a new one
                for appt in &matching_appt {
                    let mut cancelled = (*appt).clone();
                    cancelled.status = AppointmentStatus::Cancelled;
                    let old_path = appts_dir.join(format!("{}.yaml", appt.id));
                    let yaml = serde_yaml::to_string(&cancelled)?;
                    std::fs::write(&old_path, &yaml)?;
                }
            } else {
                // Add EXDATE to the recurring series for the from date
                let all_series = ics::load_series_dir(&series_dir)?;
                let matching: Vec<_> = all_series.iter()
                    .filter(|s| s.client_id == client_id && s.status == SeriesStatus::Active)
                    .collect();

                if matching.is_empty() {
                    anyhow::bail!("No active series or one-off appointment found for {} on {}", client_id, from_date);
                }

                for s in &matching {
                    let mut updated = (*s).clone();
                    if !updated.exdates.contains(&from_date) {
                        updated.exdates.push(from_date);
                        updated.exdates.sort();
                    }
                    let path = series_dir.join(format!("{}.yaml", s.id));
                    let yaml = serde_yaml::to_string(&updated)?;
                    std::fs::write(&path, &yaml)?;
                }
            }

            // Determine duration from the original appointment or series
            let duration_mins = {
                if let Some(appt) = matching_appt.first() {
                    let d = appt.end_time.signed_duration_since(appt.start_time);
                    d.num_minutes() as u32
                } else {
                    let all_series = ics::load_series_dir(&series_dir)?;
                    all_series.iter()
                        .find(|s| s.client_id == client_id)
                        .map(|s| {
                            let d = s.end_time.signed_duration_since(s.start_time);
                            d.num_minutes() as u32
                        })
                        .unwrap_or(config.availability.slot_duration_minutes)
                }
            };

            let client_name = lookup_client_name(&client_id);
            let new_start = to_dt.time();
            let new_end = new_start + chrono::Duration::minutes(duration_mins as i64);

            // Create the new one-off at the target datetime
            let new_appt = Appointment {
                id: uuid::Uuid::new_v4(),
                series_id: None,
                practitioner: practitioner.to_string(),
                client_id: client_id.clone(),
                client_name: client_name.clone(),
                date: to_dt.date(),
                start_time: new_start,
                end_time: new_end,
                status: AppointmentStatus::Confirmed,
                source: AppointmentSource::Practitioner,
                modality: None,
                rate_tag: None,
                location: config.location.clone(),
                reschedule_for: None,
                sms_confirmation: None,
                notes: Some(format!("Moved from {}", from_date)),
                created_at: chrono::Utc::now().to_rfc3339(),
            };

            std::fs::create_dir_all(&appts_dir)?;
            let new_path = appts_dir.join(format!("{}.yaml", new_appt.id));
            let yaml = serde_yaml::to_string(&new_appt)?;
            std::fs::write(&new_path, &yaml)?;

            println!("Moved {} ({}) from {} to {} at {}",
                client_name, client_id, from_date, to_dt.date(), new_start.format("%H:%M"));
            println!("  New appointment ID: {}", new_appt.id);
        }

        ScheduleAction::Update { client_id, date, status } => {
            let target_date = NaiveDate::parse_from_str(&date, "%Y-%m-%d")
                .map_err(|e| anyhow::anyhow!("Invalid date '{}': {}", date, e))?;

            let new_status = match status.as_str() {
                "arrived" => AppointmentStatus::Arrived,
                "completed" => AppointmentStatus::Completed,
                "noshow" => AppointmentStatus::NoShow,
                "late-cancel" => AppointmentStatus::LateCancellation,
                other => anyhow::bail!("Unknown status '{}'. Use: arrived, completed, noshow, late-cancel", other),
            };

            let appts_dir = std::path::PathBuf::from(&schedules_dir)
                .join(practitioner)
                .join("appointments");

            // First check if there is an existing one-off appointment
            let one_offs = ics::load_appointments_dir(&appts_dir)?;
            let existing: Vec<_> = one_offs.iter()
                .filter(|a| a.client_id == client_id && a.date == target_date)
                .collect();

            if !existing.is_empty() {
                // Update existing one-off appointment
                for appt in &existing {
                    let mut updated = (*appt).clone();
                    updated.status = new_status.clone();
                    let path = appts_dir.join(format!("{}.yaml", appt.id));
                    let yaml = serde_yaml::to_string(&updated)?;
                    std::fs::write(&path, &yaml)?;
                    println!("Updated {} ({}) on {} to status: {}",
                        appt.client_name, appt.client_id, target_date, new_status);
                }
            } else {
                // Materialise from recurring series and create a one-off with the new status
                let series_dir = std::path::PathBuf::from(&schedules_dir)
                    .join(practitioner)
                    .join("series");
                let all_series = ics::load_series_dir(&series_dir)?;

                let holidays_path = std::path::PathBuf::from(&schedules_dir)
                    .join(practitioner)
                    .join("holidays.yaml");
                let holidays = if holidays_path.exists() {
                    let yaml = std::fs::read_to_string(&holidays_path)?;
                    ics::load_holidays(&yaml)?
                } else {
                    vec![]
                };

                let mut found = false;
                for s in &all_series {
                    if s.client_id != client_id || s.status != SeriesStatus::Active {
                        continue;
                    }
                    let dates = recurrence::materialise(s, target_date, target_date, &holidays)?;
                    if dates.contains(&target_date) {
                        // Create a one-off appointment to record this status
                        let appt = Appointment {
                            id: uuid::Uuid::new_v4(),
                            series_id: Some(s.id),
                            practitioner: practitioner.to_string(),
                            client_id: client_id.clone(),
                            client_name: s.client_name.clone(),
                            date: target_date,
                            start_time: s.start_time,
                            end_time: s.end_time,
                            status: new_status.clone(),
                            source: AppointmentSource::Practitioner,
                            modality: None,
                            rate_tag: s.rate_tag.clone(),
                            location: s.location.clone(),
                            reschedule_for: None,
                            sms_confirmation: None,
                            notes: None,
                            created_at: chrono::Utc::now().to_rfc3339(),
                        };

                        std::fs::create_dir_all(&appts_dir)?;
                        let path = appts_dir.join(format!("{}.yaml", appt.id));
                        let yaml = serde_yaml::to_string(&appt)?;
                        std::fs::write(&path, &yaml)?;
                        println!("Updated {} ({}) on {} to status: {}",
                            s.client_name, client_id, target_date, new_status);
                        println!("  Appointment ID: {}", appt.id);
                        found = true;
                        break;
                    }
                }

                if !found {
                    anyhow::bail!("No appointment found for {} on {}", client_id, target_date);
                }
            }
        }

        ScheduleAction::Reschedule {
            client_id,
            date,
            duration,
            send,
        } => {
            use scheduling::availability;

            let prac_dir = std::path::PathBuf::from(&schedules_dir).join(practitioner);
            let avail = availability::load_availability(&prac_dir)?;

            let series_dir = prac_dir.join("series");
            let appts_dir = prac_dir.join("appointments");
            let series_list = ics::load_series_dir(&series_dir)?;
            let one_offs = ics::load_appointments_dir(&appts_dir)?;

            // Load holidays
            let holidays_path = prac_dir.join("holidays.yaml");
            let holidays = if holidays_path.exists() {
                let yaml = std::fs::read_to_string(&holidays_path)?;
                ics::load_holidays(&yaml)?
            } else {
                vec![]
            };

            // Parse the cancelled date and build the search window start
            let cancelled_date = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d")
                .map_err(|e| anyhow::anyhow!("Invalid date '{}': {}", date, e))?;

            // Find the original appointment to get its start time
            let original_time = one_offs
                .iter()
                .find(|a| a.client_id == client_id && a.date == cancelled_date)
                .map(|a| a.start_time)
                .or_else(|| {
                    // Check recurring series
                    series_list
                        .iter()
                        .find(|s| s.client_id == client_id)
                        .map(|s| s.start_time)
                })
                .unwrap_or(chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap());

            let from = cancelled_date.and_time(original_time);

            // Determine session duration
            let session_dur = if let Some(d) = duration {
                d
            } else {
                // Read from identity.yaml
                let clinical_root = shellexpand::tilde("~/Clinical").to_string();
                let id_path = std::path::PathBuf::from(&clinical_root)
                    .join("clients")
                    .join(&client_id)
                    .join("identity.yaml");
                if id_path.exists() {
                    let content = std::fs::read_to_string(&id_path)?;
                    let identity: serde_yaml::Value = serde_yaml::from_str(&content)?;
                    identity
                        .get("funding")
                        .and_then(|f| f.get("session_duration"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(45) as u32
                } else {
                    45
                }
            };

            println!(
                "Finding reschedule slots for {} (cancelled {}), {} min session...\n",
                client_id, date, session_dur
            );

            let mut slots = availability::find_reschedule_slots(
                &avail,
                &series_list,
                &one_offs,
                from,
                session_dur,
                &holidays,
            );

            // If no slots for full duration and it's a 90-min client, try 45
            if slots.is_empty() && session_dur > 45 {
                println!(
                    "No {}-minute slots available. Checking for 45-minute alternatives...\n",
                    session_dur
                );
                slots = availability::find_reschedule_slots(
                    &avail,
                    &series_list,
                    &one_offs,
                    from,
                    45,
                    &holidays,
                );
            }

            if slots.is_empty() {
                println!("No available slots within the {}h reschedule window.", avail.reschedule.window_hours);
                return Ok(());
            }

            // Display top slots (max 8)
            let display_count = slots.len().min(8);
            println!("Top {} slot(s):\n", display_count);
            for (i, slot) in slots.iter().take(display_count).enumerate() {
                let dur = (slot.end_time - slot.start_time).num_minutes();
                println!(
                    "  {}. {} {} {}-{} ({} min, {})",
                    i + 1,
                    slot.day_name,
                    slot.date.format("%Y-%m-%d"),
                    slot.start_time.format("%H:%M"),
                    slot.end_time.format("%H:%M"),
                    dur,
                    slot.modality,
                );
            }

            if send {
                // Load client email from identity.yaml
                let clinical_root = shellexpand::tilde("~/Clinical").to_string();
                let id_path = std::path::PathBuf::from(&clinical_root)
                    .join("clients")
                    .join(&client_id)
                    .join("identity.yaml");

                let client_email = if id_path.exists() {
                    let content = std::fs::read_to_string(&id_path)?;
                    let identity: serde_yaml::Value = serde_yaml::from_str(&content)?;
                    identity
                        .get("email")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                };

                let client_name = if id_path.exists() {
                    let content = std::fs::read_to_string(&id_path)?;
                    let identity: serde_yaml::Value = serde_yaml::from_str(&content)?;
                    identity
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&client_id)
                        .to_string()
                } else {
                    client_id.clone()
                };

                if let Some(email) = client_email {
                    let email_config = crate::email::load_email_config()?;

                    // Build slot list for the email body
                    let slot_list: String = slots
                        .iter()
                        .take(display_count)
                        .enumerate()
                        .map(|(i, s)| {
                            let dur = (s.end_time - s.start_time).num_minutes();
                            format!(
                                "  {}. {} {} at {} ({} min, {})",
                                i + 1,
                                s.day_name,
                                s.date.format("%d %B"),
                                s.start_time.format("%H:%M"),
                                dur,
                                s.modality,
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    let body = format!(
                        "Dear {},\n\n\
                         Following the cancellation of your session on {}, \
                         I have the following times available for a replacement session:\n\n\
                         {}\n\n\
                         Please let me know which, if any, of these times works for you.\n\n\
                         Kind regards,\n{}",
                        client_name.split_whitespace().next().unwrap_or(&client_name),
                        cancelled_date.format("%A %d %B"),
                        slot_list,
                        email_config.from_name,
                    );

                    crate::email::send_email(
                        &email_config,
                        &email,
                        &client_name,
                        &format!("Rescheduling your session — {}", cancelled_date.format("%d %B")),
                        &body,
                        None,
                        None,
                    )?;

                    println!("\n✓ Offer sent to {} <{}>", client_name, email);
                } else {
                    eprintln!("\n✗ No email address for {}. Add 'email:' to identity.yaml.", client_id);
                }
            } else {
                println!("\nUse --send to email these options to the client.");
            }
        }

        ScheduleAction::Export { practitioner: prac_filter } => {
            let prac = prac_filter.as_deref().unwrap_or(practitioner);
            let series_dir = std::path::PathBuf::from(&schedules_dir)
                .join(prac)
                .join("series");
            let series_list = ics::load_series_dir(&series_dir)?;

            let appts_dir = std::path::PathBuf::from(&schedules_dir)
                .join(prac)
                .join("appointments");
            let one_offs = ics::load_appointments_dir(&appts_dir)?;

            let cal_name = format!("PracticeForge -- {}", prac);
            let ics_output = ics::full_calendar_to_ics(&series_list, &one_offs, &cal_name);
            println!("{}", ics_output);
        }

        ScheduleAction::Maintain => {
            println!("Schedule maintenance:");

            // Check all blocks for expiry
            let clinical_root = shellexpand::tilde("~/Clinical").to_string();
            let clients_dir = std::path::PathBuf::from(&clinical_root).join("clients");

            if clients_dir.exists() {
                let mut warnings = Vec::new();
                for entry in std::fs::read_dir(&clients_dir)? {
                    let entry = entry?;
                    let blocks_path = entry.path().join("blocks.yaml");
                    if !blocks_path.exists() {
                        continue;
                    }
                    let yaml = std::fs::read_to_string(&blocks_path)?;
                    let blocks: Vec<AuthorisationBlock> = serde_yaml::from_str(&yaml)?;
                    for block in &blocks {
                        if let Some(w) = recurrence::check_block_expiry(block, config.blocks.warning_threshold) {
                            warnings.push(w);
                        }
                    }
                }

                if warnings.is_empty() {
                    println!("  ✓ No block expiry warnings.");
                } else {
                    println!("  ⚠ {} block expiry warning(s):", warnings.len());
                    for w in &warnings {
                        println!("    → {}", w.message);
                    }
                }
            }

            // TODO: SMS reminders (Phase 4)
            println!("  ✓ SMS reminders: not yet configured.");
        }

        ScheduleAction::Link { client_id } => {
            let token = portal::create_booking_link(&client_id, practitioner);
            let base_url = config.portal_base_url();
            println!("Booking link for {}:", client_id);
            println!("  {}/book/{}", base_url, token);
            println!();
            println!("Note: link is valid for this server session only.");
            println!("The client opens this URL, verifies via SMS OTP, and picks a slot.");
        }

    }

    Ok(())
}

fn handle_outcomes(action: OutcomesAction) -> anyhow::Result<()> {
    use outcomes::{
        all_measures_for_client, display_name, outcomes_context_block, severity_label,
        validate_score, OutcomeEntry, OutcomeRecord,
    };

    match action {
        // ----------------------------------------------------------------
        // record
        // ----------------------------------------------------------------
        OutcomesAction::Record {
            client_id,
            measure,
            score,
            date,
            items,
            notes,
        } => {
            // Validate score for known measures (warn but don't fail).
            if let Some(warning) = validate_score(&measure, score) {
                eprintln!("{}", warning);
            }

            let date_str = date.unwrap_or_else(|| {
                chrono::Local::now().format("%Y-%m-%d").to_string()
            });

            let items_parsed: Option<Vec<f64>> = items.map(|s| {
                s.split(',')
                    .map(|v| v.trim().parse::<f64>().unwrap_or(0.0))
                    .collect()
            });

            let entry = OutcomeEntry {
                date: date_str.clone(),
                score,
                items: items_parsed,
                notes,
            };

            let mut record = OutcomeRecord::load(&client_id, &measure)?;
            record.add_entry(entry);
            record.save()?;

            println!(
                "Recorded {} score {:.1} for {} on {}.",
                display_name(&measure),
                score,
                client_id,
                date_str,
            );
        }

        // ----------------------------------------------------------------
        // show
        // ----------------------------------------------------------------
        OutcomesAction::Show { client_id, measure } => {
            match measure {
                // --- Full history for one measure ---
                Some(slug) => {
                    let record = OutcomeRecord::load(&client_id, &slug)?;
                    if record.entries.is_empty() {
                        println!("No {} scores recorded for {}.", display_name(&slug), client_id);
                        return Ok(());
                    }

                    println!("{} — {} ({})", display_name(&slug), client_id, slug);
                    println!();
                    println!("{:<12} {:>8} {:<14} {}", "Date", "Score", "Severity", "Notes");
                    println!("{}", "-".repeat(60));
                    for entry in &record.entries {
                        let sev = severity_label(&slug, entry.score);
                        let notes = entry.notes.as_deref().unwrap_or("");
                        println!(
                            "{:<12} {:>8} {:<14} {}",
                            entry.date,
                            format!("{:.1}", entry.score),
                            sev,
                            notes,
                        );
                    }

                    if let Some(trend) = record.trend() {
                        let direction = if trend < 0.0 { "improving" } else if trend > 0.0 { "worsening" } else { "stable" };
                        println!();
                        println!("Trend (last 2): {:.1} ({})", trend, direction);
                    }
                }

                // --- Summary table across all measures ---
                None => {
                    let records = all_measures_for_client(&client_id)?;
                    let records: Vec<&OutcomeRecord> =
                        records.iter().filter(|r| !r.entries.is_empty()).collect();

                    if records.is_empty() {
                        println!("No outcome scores recorded for {}.", client_id);
                        return Ok(());
                    }

                    println!("Outcomes for {}", client_id);
                    println!();
                    println!(
                        "{:<10} {:>8} {:<12} {:<14} {}",
                        "Measure", "Latest", "Date", "Severity", "Trend"
                    );
                    println!("{}", "-".repeat(65));
                    for rec in &records {
                        let latest = rec.latest().unwrap();
                        let sev = severity_label(&rec.measure, latest.score);
                        let trend_str = match rec.trend() {
                            None => "–".to_string(),
                            Some(d) => {
                                if d == 0.0 {
                                    "→ 0".to_string()
                                } else if d < 0.0 {
                                    format!("↑ {:.1} improving", d.abs())
                                } else {
                                    format!("↓ {:.1} worsening", d.abs())
                                }
                            }
                        };
                        println!(
                            "{:<10} {:>8} {:<12} {:<14} {}",
                            display_name(&rec.measure),
                            format!("{:.1}", latest.score),
                            latest.date,
                            sev,
                            trend_str,
                        );
                    }
                }
            }
        }

        // ----------------------------------------------------------------
        // export
        // ----------------------------------------------------------------
        OutcomesAction::Export { client_id } => {
            let block = outcomes_context_block(&client_id);
            if block.is_empty() {
                eprintln!("No outcome measures recorded for {}.", client_id);
            } else {
                print!("{}", block);
            }
        }
    }

    Ok(())
}

fn handle_tm3_diary(action: Tm3DiaryAction) -> anyhow::Result<()> {
    match action {
        Tm3DiaryAction::Book {
            client_id,
            datetime,
            duration,
            tm3_id,
            dry_run,
        } => {
            // Resolve TM3 customer ID from identity.yaml unless overridden
            let customer_id = if let Some(id) = tm3_id {
                id
            } else {
                tm3_diary::read_tm3_id(&client_id).ok_or_else(|| {
                    anyhow::anyhow!(
                        "No tm3_id in identity.yaml for client '{}'.\n\
                         Set tm3_id: <number> in ~/Clinical/clients/{}/identity.yaml\n\
                         or pass --tm3-id <id>.",
                        client_id, client_id
                    )
                })?
            };

            let (start_dt, end_dt) = tm3_diary::parse_datetime(&datetime, duration)?;
            let cfg = tm3_diary::Tm3DiaryConfig::load();

            eprintln!(
                "[tm3-diary] {} → TM3 customer {} | {} – {} ({} min)",
                client_id, customer_id, start_dt, end_dt, duration
            );
            eprintln!(
                "[tm3-diary] practitionerId={} locationId={} stockId={} serviceTypeId={}",
                cfg.practitioner_id, cfg.location_id, cfg.stock_id, cfg.service_type_id
            );

            if dry_run {
                println!("[dry-run] Would book {} for {} on {} ({} min).", client_id, customer_id, start_dt, duration);
                return Ok(());
            }

            let req = tm3_diary::BookingRequest {
                tm3_customer_id: customer_id,
                start_dt,
                end_dt,
                duration_mins: duration,
            };

            // Use std::thread::spawn to escape the tokio runtime context
            let result = std::thread::spawn(move || {
                let client = tm3_diary::Tm3DiaryClient::new()?;
                client.book(&req, &cfg)
            })
            .join()
            .map_err(|e| anyhow::anyhow!("Thread panic: {:?}", e))??;

            match result.appointment_id {
                Some(id) => println!("✓ Booked. TM3 appointment ID: {}", id),
                None => println!("✓ Booked. (TM3 did not return an appointment ID)"),
            }
        }

        Tm3DiaryAction::Update {
            appointment_id,
            status,
            comment,
            dry_run,
        } => {
            let new_status = tm3_diary::AttendanceStatus::from_str(&status)
                .ok_or_else(|| anyhow::anyhow!("Unknown status '{}' — use: arrived, completed, dna, scheduled", status))?;

            eprintln!("[tm3-diary] Marking appointment {} as {:?}", appointment_id, status);

            if dry_run {
                println!("[dry-run] Would set appointment {} status to {} ({})", appointment_id, status, new_status.as_u8());
                return Ok(());
            }

            let comment_owned = comment.clone();
            let result = std::thread::spawn(move || {
                let client = tm3_diary::Tm3DiaryClient::new()?;
                client.update_status(appointment_id, new_status, comment_owned.as_deref())
            })
            .join()
            .map_err(|e| anyhow::anyhow!("Thread panic: {:?}", e))??;

            let returned_id = result.get("id").and_then(|v| v.as_u64());
            println!("✓ Appointment {} marked as {}. (TM3 id={:?})", appointment_id, status, returned_id);
        }

        Tm3DiaryAction::Reschedule {
            appointment_id,
            datetime,
            duration,
            dry_run,
        } => {
            let (new_start, new_end) = tm3_diary::parse_datetime(&datetime, duration)?;

            eprintln!("[tm3-diary] Rescheduling appointment {} to {} – {}", appointment_id, new_start, new_end);

            if dry_run {
                println!("[dry-run] Would reschedule appointment {} to {} ({} min)", appointment_id, new_start, duration);
                return Ok(());
            }

            let result = std::thread::spawn(move || {
                let client = tm3_diary::Tm3DiaryClient::new()?;
                client.reschedule(appointment_id, &new_start, &new_end)
            })
            .join()
            .map_err(|e| anyhow::anyhow!("Thread panic: {:?}", e))??;

            let returned_id = result.get("id").and_then(|v| v.as_u64());
            println!("✓ Appointment {} rescheduled to {}. (TM3 id={:?})", appointment_id, datetime, returned_id);
        }

        Tm3DiaryAction::Delete {
            appointment_id,
            dry_run,
        } => {
            if dry_run {
                println!("[dry-run] Would delete appointment {}", appointment_id);
                return Ok(());
            }

            std::thread::spawn(move || {
                let client = tm3_diary::Tm3DiaryClient::new()?;
                client.delete_appointment(appointment_id)
            })
            .join()
            .map_err(|e| anyhow::anyhow!("Thread panic: {:?}", e))??;

            println!("✓ Appointment {} deleted.", appointment_id);
        }

        Tm3DiaryAction::Get { appointment_id } => {
            let appt = std::thread::spawn(move || {
                let client = tm3_diary::Tm3DiaryClient::new()?;
                client.get_appointment(appointment_id)
            })
            .join()
            .map_err(|e| anyhow::anyhow!("Thread panic: {:?}", e))??;

            println!("TM3 appointment {}", appt.id);
            println!("  Time:          {} – {}", appt.start_date_time, appt.end_date_time);
            println!("  Status:        {} ({})", appt.status, match appt.status {
                0 => "Scheduled", 1 => "Arrived", 2 => "Completed", 3 => "DNA", _ => "Unknown"
            });
            println!("  Customer:      id={}", appt.customer_id);
            println!("  Name:          {}", appt.name.as_deref().unwrap_or("(none)"));
            if let Some(ref c) = appt.comment {
                if !c.is_empty() {
                    println!("  Comment:       {}", c);
                }
            }
        }

        Tm3DiaryAction::RefreshCache => {
            let count = tm3_clients::refresh_cache()?;
            println!("✓ TM3 client cache refreshed ({} clients).", count);
        }
    }

    Ok(())
}
