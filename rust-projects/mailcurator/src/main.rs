// mailcurator — per-sender email lifecycle + structured extraction driven by notmuch
//
// Invocation:
//   mailcurator run              # apply all policies
//   mailcurator run --dry-run    # show what would happen, don't touch anything
//   mailcurator validate         # sanity-check config
//   mailcurator list-policies    # show loaded policies
//
// Design: policies live in ~/.config/mailcurator/policies.toml. Each policy
// describes which messages it applies to (from / subject / …), what tags to
// add on arrival, and how the lifecycle progresses (archive_after N days,
// delete_after N days). Extraction (content → structured JSONL) is v2; v1
// just does lifecycle.
//
// Idempotency: each policy uses a "seen" tag (curator-<policy>-seen) to mark
// messages it has already processed. Subsequent runs skip them. Lifecycle
// transitions are checked every run — safe to run repeatedly.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod config;
mod notmuch;
mod policy;
mod store;

#[derive(Parser)]
#[command(name = "mailcurator")]
#[command(about = "Email lifecycle policies + structured extraction, via notmuch")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Path to policies.toml (default: ~/.config/mailcurator/policies.toml)
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Apply all policies to currently-matching messages
    Run {
        /// Show what would happen, don't modify anything
        #[arg(long)]
        dry_run: bool,

        /// Only run this named policy (match by .name field)
        #[arg(long)]
        only: Option<String>,
    },
    /// Validate the config file without running
    Validate,
    /// List loaded policies
    ListPolicies,
}

fn config_path(cli_arg: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = cli_arg {
        return Ok(p);
    }
    let home = dirs::config_dir().context("no XDG config dir")?;
    Ok(home.join("mailcurator").join("policies.toml"))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let path = config_path(cli.config)?;

    match cli.command {
        Command::Validate => {
            let cfg = config::load(&path)?;
            println!(
                "Config OK: {} policies loaded from {}",
                cfg.policies.len(),
                path.display()
            );
        }
        Command::ListPolicies => {
            let cfg = config::load(&path)?;
            for p in &cfg.policies {
                println!("- {} ({})", p.name, p.summary());
            }
        }
        Command::Run { dry_run, only } => {
            let cfg = config::load(&path)?;
            let mut total_tagged = 0u64;
            let mut total_archived = 0u64;
            let mut total_deleted = 0u64;

            for pol in &cfg.policies {
                if let Some(name) = &only {
                    if &pol.name != name {
                        continue;
                    }
                }
                let stats = policy::apply(pol, dry_run)
                    .with_context(|| format!("policy '{}' failed", pol.name))?;
                total_tagged += stats.tagged_on_arrival;
                total_archived += stats.archived;
                total_deleted += stats.deleted;
                println!(
                    "{:<30}  +arrival={:<4}  →archive={:<4}  →trash={:<4}",
                    pol.name, stats.tagged_on_arrival, stats.archived, stats.deleted
                );
            }

            println!();
            println!(
                "TOTAL  tagged-on-arrival={}  archived={}  trashed={}{}",
                total_tagged,
                total_archived,
                total_deleted,
                if dry_run { "  [DRY RUN — no changes made]" } else { "" }
            );
        }
    }

    Ok(())
}
