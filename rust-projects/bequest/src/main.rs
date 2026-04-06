mod config;
mod disclose;
mod enrol;
mod gf256;
mod heartbeat;
mod page;
mod shamir;
mod vault;

use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "bequest", about = "Digital estate orchestrator")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Split a secret into shares using Shamir's Secret Sharing
    Split {
        /// Minimum shares needed to reconstruct
        #[arg(short = 'k', long, default_value = "2")]
        threshold: u8,

        /// Total number of shares to generate
        #[arg(short = 'n', long, default_value = "3")]
        shares: u8,

        /// Write each share to a separate file in this directory
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Reconstruct a secret from shares (read from stdin, one per line)
    Reconstruct,

    /// Generate a self-contained HTML reconstruction page
    #[command(name = "generate-page")]
    GeneratePage {
        /// Output file path
        #[arg(short, long, default_value = "reconstruction.html")]
        output: PathBuf,
    },

    /// Run self-tests to verify split/reconstruct round-trip
    Test {
        /// Number of random test rounds
        #[arg(long, default_value = "10")]
        rounds: usize,
    },

    /// Manage the encrypted estate vault
    Vault {
        #[command(subcommand)]
        command: VaultCommands,
    },

    /// Dead man's switch — heartbeat monitoring
    Heartbeat {
        #[command(subcommand)]
        command: HeartbeatCommands,
    },

    /// Enrol trustees — split vault key and create bundles
    Enrol {
        /// Minimum shares needed to reconstruct
        #[arg(short = 'k', long, default_value = "2")]
        threshold: u8,

        /// Total number of shares (must match number of trustees in config)
        #[arg(short = 'n', long, default_value = "3")]
        shares: u8,

        /// Email bundles to trustees via himalaya
        #[arg(long)]
        send: bool,
    },

    /// Send disclosure notification to all trustees
    Disclose {
        /// Preview emails without sending
        #[arg(long)]
        dry_run: bool,
    },

    /// Show current configuration
    Config,
}

#[derive(Subcommand)]
enum VaultCommands {
    /// Create a new vault with generated passphrase
    Init,

    /// Decrypt the vault for editing
    Open,

    /// Encrypt the vault and remove plaintext
    Seal,

    /// Show vault state and contents
    Status,

    /// Pull Estate folder from Vaultwarden and update the vault export
    Update,

    /// Split the vault passphrase into Shamir shares
    Split {
        /// Minimum shares needed to reconstruct
        #[arg(short = 'k', long, default_value = "2")]
        threshold: u8,

        /// Total number of shares to generate
        #[arg(short = 'n', long, default_value = "3")]
        shares: u8,

        /// Write each share to a separate file in this directory
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum HeartbeatCommands {
    /// Record a heartbeat (I'm still here)
    Ping,

    /// Show heartbeat status and all detected signals
    Status {
        /// Days of inactivity before warning
        #[arg(long, default_value = "14")]
        threshold: u64,

        /// Days of grace period after threshold
        #[arg(long, default_value = "7")]
        grace: u64,
    },

    /// Check heartbeat state (for automated use). Exit: 0=normal, 1=warning, 2=triggered
    Check {
        /// Days of inactivity before warning
        #[arg(long, default_value = "14")]
        threshold: u64,

        /// Days of grace period after threshold
        #[arg(long, default_value = "7")]
        grace: u64,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Split {
            threshold,
            shares: n,
            output,
        } => cmd_split(threshold, n, output),
        Commands::Reconstruct => cmd_reconstruct(),
        Commands::GeneratePage { output } => cmd_generate_page(output),
        Commands::Test { rounds } => cmd_test(rounds),
        Commands::Vault { command } => match command {
            VaultCommands::Init => vault::init(),
            VaultCommands::Open => vault::open(),
            VaultCommands::Seal => vault::seal(),
            VaultCommands::Status => vault::status(),
            VaultCommands::Update => vault::update(),
            VaultCommands::Split {
                threshold,
                shares,
                output,
            } => vault::split_key(threshold, shares, output),
        },
        Commands::Heartbeat { command } => match command {
            HeartbeatCommands::Ping => heartbeat::record(),
            HeartbeatCommands::Status { threshold, grace } => {
                heartbeat::status(threshold, grace)
            }
            HeartbeatCommands::Check { threshold, grace } => {
                let state = heartbeat::check(threshold, grace)?;
                // On WARNING, send warning email to William
                if state == heartbeat::State::Warning {
                    let elapsed = threshold; // approximate
                    let remaining = grace;
                    let _ = disclose::warn(elapsed, remaining);
                }
                // On TRIGGERED, auto-disclose
                if state == heartbeat::State::Triggered {
                    eprintln!("Auto-disclosure triggered...");
                    let _ = disclose::run(false);
                }
                std::process::exit(match state {
                    heartbeat::State::Normal => 0,
                    heartbeat::State::Warning => 1,
                    heartbeat::State::Triggered => 2,
                });
            }
        },
        Commands::Enrol {
            threshold,
            shares,
            send,
        } => {
            enrol::run(threshold, shares)?;
            if send {
                enrol::send_bundles()?;
            }
            Ok(())
        }
        Commands::Disclose { dry_run } => disclose::run(dry_run),
        Commands::Config => {
            let config = config::Config::load()?;
            println!("{}", toml::to_string_pretty(&config)?);
            Ok(())
        }
    }
}

fn read_secret() -> Result<Vec<u8>> {
    if io::stdin().is_terminal() {
        eprint!("Enter secret: ");
        io::stderr().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        // Trim the trailing newline from interactive input
        let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
        Ok(trimmed.as_bytes().to_vec())
    } else {
        let mut buf = Vec::new();
        io::stdin().read_to_end(&mut buf)?;
        // Trim trailing newline if piped from echo
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        Ok(buf)
    }
}

fn cmd_split(k: u8, n: u8, output: Option<PathBuf>) -> Result<()> {
    let secret = read_secret()?;
    if secret.is_empty() {
        bail!("secret is empty");
    }

    let shares = shamir::split(&secret, k, n)?;

    if let Some(dir) = output {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating output directory: {}", dir.display()))?;
        for (i, share) in shares.iter().enumerate() {
            let encoded = STANDARD.encode(share);
            let label = format!("Share {} of {} (threshold: {}): {}\n", i + 1, n, k, encoded);
            let path = dir.join(format!("share-{}.txt", i + 1));
            std::fs::write(&path, &label)
                .with_context(|| format!("writing {}", path.display()))?;
            eprintln!("Wrote {}", path.display());
        }
    } else {
        let stdout = io::stdout();
        let mut out = stdout.lock();
        for (i, share) in shares.iter().enumerate() {
            let encoded = STANDARD.encode(share);
            writeln!(out, "Share {} of {} (threshold: {}): {}", i + 1, n, k, encoded)?;
        }
    }

    Ok(())
}

fn cmd_reconstruct() -> Result<()> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;

    let mut shares: Vec<Vec<u8>> = Vec::new();
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Strip label prefix if present
        let b64 = if let Some(pos) = line.rfind(": ") {
            &line[pos + 2..]
        } else {
            line
        };
        let bytes = STANDARD
            .decode(b64.trim())
            .context("invalid base64 in share")?;
        if bytes.len() < 2 {
            bail!("share too short");
        }
        shares.push(bytes);
    }

    if shares.is_empty() {
        bail!("no shares provided");
    }

    let secret = shamir::reconstruct(&shares)?;

    match String::from_utf8(secret.clone()) {
        Ok(s) => print!("{}", s),
        Err(_) => {
            // Binary secret — output as hex
            for byte in &secret {
                print!("{:02x}", byte);
            }
            println!();
        }
    }

    Ok(())
}

fn cmd_generate_page(output: PathBuf) -> Result<()> {
    let html = page::generate_reconstruction_html();
    std::fs::write(&output, &html)
        .with_context(|| format!("writing {}", output.display()))?;
    eprintln!("Generated {}", output.display());
    Ok(())
}

fn cmd_test(rounds: usize) -> Result<()> {
    use rand::rngs::OsRng;
    use rand::RngCore;

    let mut passed = 0;

    for round in 0..rounds {
        // Random secret length 1..256
        let secret_len = (OsRng.next_u32() % 256 + 1) as usize;
        let mut secret = vec![0u8; secret_len];
        OsRng.fill_bytes(&mut secret);

        // Random threshold and shares
        let k = (OsRng.next_u32() % 5 + 1) as u8; // 1..=5
        let n = (OsRng.next_u32() % 4 + k as u32).min(255) as u8; // k..=min(k+3, 255)

        let shares = shamir::split(&secret, k, n)?;

        // Reconstruct with exactly k shares (first k)
        let subset: Vec<Vec<u8>> = shares[..k as usize].to_vec();
        let recovered = shamir::reconstruct(&subset)?;
        if recovered != secret {
            bail!(
                "round {}: reconstruction failed (k={}, n={}, secret_len={})",
                round,
                k,
                n,
                secret_len
            );
        }

        // Also reconstruct with all shares
        let recovered_all = shamir::reconstruct(&shares)?;
        if recovered_all != secret {
            bail!("round {}: reconstruction with all shares failed", round);
        }

        passed += 1;
    }

    // Edge cases
    let edge_cases = [
        ("k=1 n=1", 1u8, 1u8),
        ("k=1 n=5", 1, 5),
        ("k=5 n=5", 5, 5),
        ("k=2 n=2", 2, 2),
    ];
    for (label, k, n) in &edge_cases {
        let secret = b"edge case test";
        let shares = shamir::split(secret, *k, *n)?;
        let subset: Vec<Vec<u8>> = shares[..*k as usize].to_vec();
        let recovered = shamir::reconstruct(&subset)?;
        if recovered != secret {
            bail!("edge case '{}' failed", label);
        }
        passed += 1;
    }

    println!("{} tests passed (all OK)", passed);
    Ok(())
}
