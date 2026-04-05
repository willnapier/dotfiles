mod gf256;
mod page;
mod shamir;

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
