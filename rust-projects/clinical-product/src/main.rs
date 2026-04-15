use clap::Parser;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub mod billing;
pub mod config;
mod dashboard;
pub mod email;
pub mod onboard;
mod referral;
mod runpod;
pub mod session_cookies;
mod sync;
mod inference;
pub mod tm3_clients;

#[derive(Parser)]
#[command(name = "clinical-product", about = "Clinical session note generator")]
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
    /// Reads the `[pod]` section of ~/.config/clinical-product/config.toml
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
    },
    /// Create invoices for all clients with uninvoiced sessions.
    InvoiceBatch {
        /// Preview only — show what would be invoiced without creating.
        #[arg(long)]
        dry_run: bool,
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
        invoice::{self, build_invoice, extract_session_dates, uninvoiced_sessions},
        manual::ManualProvider,
        remind,
        status,
        traits::{AccountingProvider, InvoiceFilter},
    };

    let config = BillingConfig::load()?;

    if !config.enabled {
        match action {
            BillingAction::Status { .. } => {
                println!("Billing is not enabled.");
                println!("Add [billing] enabled = true to config.toml to activate.");
                println!(
                    "Config file: {}",
                    crate::config::config_file_path().display()
                );
                return Ok(());
            }
            _ => {
                anyhow::bail!(
                    "Billing is not enabled. Add [billing] enabled = true to {}",
                    crate::config::config_file_path().display()
                );
            }
        }
    }

    // For now, only the Manual provider is implemented.
    // Future: match on config.provider to select Xero, etc.
    let provider = ManualProvider::new(&config)?;

    match action {
        BillingAction::Status { overdue } => {
            status::show_status(&provider, overdue)?;
        }

        BillingAction::Invoice { client_id, dates } => {
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

            let session_dates = if let Some(dates_str) = dates {
                dates_str
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .collect()
            } else {
                // Find uninvoiced sessions
                let notes_path = client_dir.join("notes.md");
                if !notes_path.exists() {
                    anyhow::bail!("No notes.md for client {}", client_id);
                }
                let notes = std::fs::read_to_string(&notes_path)?;
                let all_sessions = extract_session_dates(&notes);
                let already_invoiced = provider.invoiced_dates_for_client(&client_id)?;
                let uninvoiced = uninvoiced_sessions(&all_sessions, &already_invoiced);

                if uninvoiced.is_empty() {
                    println!("No uninvoiced sessions for {}.", client_id);
                    return Ok(());
                }
                uninvoiced
            };

            let reference = provider.next_invoice_number()?;
            let inv = build_invoice(
                reference,
                &client_id,
                &identity_path,
                &session_dates,
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

            let result = provider.create_invoice(&inv)?;
            println!("✓ Invoice {} created.", result.reference);
            if let Some(path) = &result.file_path {
                println!("  File: {}", path);
            }
        }

        BillingAction::InvoiceBatch { dry_run } => {
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
                let notes_path = client_dir.join("notes.md");

                if !identity_path.exists() || !notes_path.exists() {
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

                let notes = std::fs::read_to_string(&notes_path).unwrap_or_default();
                let all_sessions = extract_session_dates(&notes);
                let already_invoiced =
                    provider.invoiced_dates_for_client(&client_id).unwrap_or_default();
                let uninvoiced = uninvoiced_sessions(&all_sessions, &already_invoiced);

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

            // Load practitioner name from email config if available
            let practitioner = crate::email::load_email_config()
                .map(|c| c.from_name)
                .unwrap_or_else(|_| "The Practitioner".to_string());

            for (inv, tone) in &due {
                let is_insurer = !inv.bill_to_name.is_empty()
                    && inv.bill_to_name != inv.client_name;

                let reminder = if is_insurer {
                    remind::render_insurer_reminder(inv, &practitioner)
                } else {
                    remind::render_client_reminder(inv, tone, &practitioner, "")
                };

                if send {
                    if let Some(ref to_email) = inv.payment_link {
                        // This is a placeholder — real email comes from BillTo
                        println!("  Would send to {} — email integration pending", to_email);
                    }
                    println!(
                        "  ✓ {} → {} [{}] (send not yet wired to email)",
                        inv.reference, reminder.to_name, tone
                    );
                } else {
                    println!("--- {} ({}) ---", inv.reference, tone);
                    println!("To: {}", reminder.to_name);
                    println!("Subject: {}", reminder.subject);
                    println!();
                    println!("{}", reminder.body);
                    println!();
                }
            }

            if !send {
                println!(
                    "{} reminder(s) ready. Use --send to deliver via email.",
                    due.len()
                );
            }
        }

        BillingAction::Maintain => {
            let summary = status::compact_summary(&provider)?;
            println!("Billing: {}", summary);

            let overdue = provider.list_invoices(InvoiceFilter {
                overdue_only: true,
                ..Default::default()
            })?;

            if !overdue.is_empty() {
                let due = remind::due_reminders(&config, &overdue);
                if !due.is_empty() {
                    println!(
                        "  {} reminder(s) pending. Run: clinical-product billing remind",
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
