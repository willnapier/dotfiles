mod checks;

use anyhow::Result;
use clap::Parser;
use std::process;

#[derive(Parser)]
#[command(
    name = "cross-machine-sync-check",
    about = "Cross-machine code propagation drift detection"
)]
struct Cli {
    /// Remote SSH target (e.g. will@nimbini or williamnapier@williams-macbook-air)
    #[arg(short, long, env = "SYNC_CHECK_REMOTE")]
    remote: Option<String>,

    /// Output as JSON
    #[arg(long)]
    json: bool,

    /// Suppress output, exit code only (0=clean, 1=drift)
    #[arg(short, long)]
    quiet: bool,

    /// Run only local checks (no SSH required)
    #[arg(long)]
    local_only: bool,
}

fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(&cli) {
        eprintln!("error: {:#}", e);
        process::exit(2);
    }
}

fn run(cli: &Cli) -> Result<()> {
    let remote = if cli.local_only {
        None
    } else {
        match &cli.remote {
            Some(r) => Some(r.clone()),
            None => detect_remote(),
        }
    };

    let mut results = Vec::new();

    // 1. Dotfiles git sync
    results.push(checks::dotfiles_uncommitted()?);

    if let Some(ref remote) = remote {
        results.push(checks::dotfiles_remote_sync(remote)?);
    }

    // 2. Rust binary freshness
    results.extend(checks::rust_binary_freshness()?);

    if let Some(ref remote) = remote {
        results.extend(checks::rust_binary_freshness_remote(remote)?);
    }

    // 3. Skill file parity (requires SSH)
    if let Some(ref remote) = remote {
        results.extend(checks::skill_parity(remote)?);
    }

    // 4. Messageboard staleness
    results.push(checks::messageboard_staleness()?);

    // Output
    let has_drift = results.iter().any(|r| r.status == checks::Status::Drift);

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else if !cli.quiet {
        for r in &results {
            let icon = match r.status {
                checks::Status::Clean => "  ",
                checks::Status::Drift => "! ",
                checks::Status::Skipped => "- ",
            };
            let label = match r.status {
                checks::Status::Clean => "clean",
                checks::Status::Drift => "DRIFT",
                checks::Status::Skipped => "skipped",
            };
            if r.status == checks::Status::Drift {
                println!("{}{} — {}", icon, r.name, label);
                for detail in &r.details {
                    println!("    {}", detail);
                }
            } else {
                println!("{}{} — {}", icon, r.name, label);
            }
        }

        let drift_count = results.iter().filter(|r| r.status == checks::Status::Drift).count();
        let clean_count = results.iter().filter(|r| r.status == checks::Status::Clean).count();
        println!();
        if has_drift {
            println!("{} drift, {} clean", drift_count, clean_count);
        } else {
            println!("All {} checks clean", clean_count);
        }
    }

    if has_drift {
        process::exit(1);
    }

    Ok(())
}

/// Auto-detect remote based on hostname
fn detect_remote() -> Option<String> {
    let hostname = std::process::Command::new("hostname")
        .arg("-s")
        .output()
        .ok()?;
    let hostname = String::from_utf8_lossy(&hostname.stdout).trim().to_string();

    if hostname.to_lowercase().contains("macbook") || hostname.to_lowercase().contains("william") {
        Some("will@nimbini".to_string())
    } else {
        // Assume we're on nimbini, target the Mac
        Some("williamnapier@williams-macbook-air".to_string())
    }
}
