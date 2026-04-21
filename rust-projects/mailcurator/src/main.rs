// mailcurator — per-sender email lifecycle + structured extraction driven by notmuch
//
// Invocation:
//   mailcurator run                       apply all policies
//   mailcurator run --dry-run             show what would happen, don't touch
//   mailcurator run --only <name>         run just one policy
//   mailcurator validate                  sanity-check config
//   mailcurator list-policies             show loaded policies
//   mailcurator preview <name>            show sample matches for a policy
//   mailcurator unmatched [--window 6M]   show top uncategorised senders/subjects
//
// Design: policies live in ~/.config/mailcurator/policies.toml. Each policy
// describes which messages it applies to (from / subject / …), what tags to
// add on arrival, and how the lifecycle progresses (archive_after N days,
// delete_after N days). Extraction (content → structured JSONL) is v2; v1
// just does lifecycle.
//
// Idempotency: each policy uses a "seen" tag (curator-<policy>-seen) to mark
// messages it has already processed. Subsequent runs skip them.
//
// Safety:
//   - `quarantine = true` on a policy disables archive/delete (observe-only).
//   - Every delete action writes an audit record to
//     ~/.local/share/mailcurator/deletions.jsonl before applying the tag.

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
    /// Show sample messages a policy would match (for FP detection)
    Preview {
        /// Policy name to preview
        name: String,
        /// Maximum number of matches to show
        #[arg(short, long, default_value_t = 30)]
        limit: usize,
    },
    /// Show inbox messages not matched by any policy, grouped by sender
    /// (for FN detection — senders you might want to codify)
    Unmatched {
        /// Date window (notmuch date: syntax), e.g. "6M" = last 6 months
        #[arg(long, default_value = "6M")]
        window: String,
        /// Maximum senders to list
        #[arg(short, long, default_value_t = 30)]
        limit: usize,
    },
}

fn config_path(cli_arg: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = cli_arg {
        return Ok(p);
    }
    // Use XDG-style ~/.config rather than dirs::config_dir() which returns
    // macOS's ~/Library/Application Support. Matches the convention used by
    // practiceforge, clinical, and other tools in this system.
    let home = dirs::home_dir().context("couldn't resolve $HOME")?;
    Ok(home.join(".config").join("mailcurator").join("policies.toml"))
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
                let quarantine_note = if pol.quarantine { "  [QUARANTINE]" } else { "" };
                println!(
                    "{:<30}  +arrival={:<4}  →archive={:<4}  →trash={:<4}{}",
                    pol.name,
                    stats.tagged_on_arrival,
                    stats.archived,
                    stats.deleted,
                    quarantine_note
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
        Command::Preview { name, limit } => {
            let cfg = config::load(&path)?;
            let pol = cfg.policies.iter().find(|p| p.name == name)
                .with_context(|| format!("no policy named '{name}'"))?;
            let query = pol.base_query();
            let seen = pol.seen_tag();
            println!("policy:        {}", pol.name);
            println!("base query:    {}", query);
            println!("seen-tag:      {}", seen);
            println!();

            // New matches (not yet processed by this policy)
            let new_query = format!("({query}) and not tag:{seen}");
            let news = store::list_messages(&new_query)?;
            println!("=== NEW matches (not yet marked seen): {} ===", news.len());
            for m in news.iter().take(limit) {
                println!(
                    "  {:<12}  {:<40}  {}",
                    truncate(&m.date, 12),
                    truncate(&m.from, 40),
                    truncate(&m.subject, 120)
                );
            }
            if news.len() > limit {
                println!("  ... and {} more", news.len() - limit);
            }
            println!();

            // Already-seen matches (processed before)
            let seen_query = format!("({query}) and tag:{seen}");
            let seens = store::list_messages(&seen_query)?;
            println!("=== already-seen matches: {} ===", seens.len());
            for m in seens.iter().take(limit.min(10)) {
                println!(
                    "  {:<12}  {:<40}  {}",
                    truncate(&m.date, 12),
                    truncate(&m.from, 40),
                    truncate(&m.subject, 120)
                );
            }
            if seens.len() > limit.min(10) {
                println!("  ... and {} more", seens.len() - limit.min(10));
            }
        }
        Command::Unmatched { window, limit } => {
            let cfg = config::load(&path)?;
            // Build the big OR of all policy base queries.
            let any_matches: Vec<String> = cfg.policies.iter()
                .map(|p| format!("({})", p.base_query()))
                .collect();
            let matched_by_any = if any_matches.is_empty() {
                // No policies — everything is "unmatched".
                "false".to_string()
            } else {
                any_matches.join(" or ")
            };
            let unmatched_query = format!(
                "tag:inbox and not tag:trash and date:{window}.. and not ({matched_by_any})"
            );
            let count = notmuch::count(&unmatched_query)?;
            println!("unmatched in inbox within date:{window}.. = {count}");
            println!();
            if count == 0 {
                return Ok(());
            }

            let messages = store::list_messages(&unmatched_query)?;

            // Group by normalised From (just the address, if we can find one).
            use std::collections::HashMap;
            let mut by_sender: HashMap<String, u64> = HashMap::new();
            for m in &messages {
                let key = normalise_sender(&m.from);
                *by_sender.entry(key).or_insert(0) += 1;
            }
            let mut sorted: Vec<(String, u64)> = by_sender.into_iter().collect();
            sorted.sort_by(|a, b| b.1.cmp(&a.1));

            println!("=== top {} unmatched senders ===", limit.min(sorted.len()));
            for (sender, n) in sorted.iter().take(limit) {
                println!("  {:>5}  {}", n, sender);
            }
        }
    }

    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(n.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

/// Extract just the email address from a "Name <addr>" string, or return the
/// input trimmed if no address found.
fn normalise_sender(from: &str) -> String {
    // notmuch's "authors" field for a thread can be comma-separated.
    // Take the first listed.
    let first = from.split('|').next().unwrap_or(from).trim();
    if let Some(start) = first.find('<') {
        if let Some(end) = first.find('>') {
            if end > start {
                return first[start + 1..end].trim().to_string();
            }
        }
    }
    first.to_string()
}
