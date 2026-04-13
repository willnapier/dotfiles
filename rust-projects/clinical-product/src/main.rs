use clap::Parser;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

mod dashboard;
mod referral;
mod runpod;
pub mod session_cookies;
mod sync;
mod voice_pod;

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
    /// Used by `clinical note` as CLINICAL_LLM_CMD to integrate the voice model.
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

    /// Manage the voice inference pod lifecycle (status/start/stop).
    ///
    /// Reads the `[pod]` section of ~/.config/clinical-product/voice-config.toml
    /// to determine which pod to manage. If `managed = false` or pod_id is
    /// empty, all commands report the configured state without making changes.
    VoicePod {
        #[command(subcommand)]
        action: VoicePodAction,
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
    /// and reports new clients that need scaffolding.
    Sync,

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
enum VoicePodAction {
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
    let config = voice_pod::load_pod_config()?;
    if !config.has_pod() {
        return Ok(());
    }
    let client = runpod::Client::new()?;
    voice_pod::prepare_for_request(&client, &config).await?;
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
    let _ = voice_pod::record_activity();

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
        Command::VoicePod { action } => {
            handle_voice_pod(action).await?;
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
        Command::Sync => {
            let result = sync::sync_check()?;
            sync::display_sync_result(&result);
        }
        Command::Dashboard { port, open } => {
            dashboard::serve(port, open).await?;
        }
    }

    Ok(())
}

async fn handle_voice_pod(action: VoicePodAction) -> anyhow::Result<()> {
    use runpod::Client as RunPodClient;

    match action {
        VoicePodAction::List => {
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
        VoicePodAction::Volumes => {
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
        VoicePodAction::Status => {
            let config = voice_pod::load_pod_config()?;
            let state = voice_pod::load_state();

            println!("Voice pod configuration:");
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
                println!("See {} to configure.", voice_pod::config_path().display());
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
        VoicePodAction::Start => {
            let config = voice_pod::load_pod_config()?;
            if !config.has_pod() {
                anyhow::bail!(
                    "No managed pod configured. Set [pod] managed=true and pod_id in {}",
                    voice_pod::config_path().display()
                );
            }
            let client = RunPodClient::new()?;
            let started = voice_pod::ensure_running(&client, &config).await?;
            if started {
                println!("Pod started.");
            } else {
                println!("Pod was already running.");
            }
        }
        VoicePodAction::Stop => {
            let config = voice_pod::load_pod_config()?;
            if !config.has_pod() {
                anyhow::bail!(
                    "No managed pod configured. Set [pod] managed=true and pod_id in {}",
                    voice_pod::config_path().display()
                );
            }
            let client = RunPodClient::new()?;
            let stopped = voice_pod::ensure_stopped(&client, &config).await?;
            if stopped {
                println!("Pod stopped.");
            } else {
                println!("Pod was already stopped.");
            }
        }
        VoicePodAction::Maintain => {
            // Idle-timeout sweeper: check if pod is running AND idle-for-long-enough.
            // If so, stop it. Intended to be called periodically (cron, launchd, etc.)
            // from cross-platform schedulers the user configures themselves.
            let config = voice_pod::load_pod_config()?;
            if !config.has_pod() {
                println!("No managed pod — nothing to maintain.");
                return Ok(());
            }
            let state = voice_pod::load_state();
            let timeout = config.idle_timeout();
            if !voice_pod::is_idle(&state, timeout) {
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

fn trunc(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n.saturating_sub(1)])
    }
}
