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

mod bills_cli;
mod bookings_cli;
mod config;
mod coverage;
mod eval;
mod extract;
mod extractors;
mod journeys_cli;
mod leader;
mod llm;
mod llm_cache;
mod notmuch;
mod orders_cli;
mod policy;
mod store;
mod subscriptions;
mod tesla_cli;

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

        /// Ignore each policy's age thresholds — process all matching
        /// messages immediately, regardless of `archive_after_days` /
        /// `delete_after_days`. Useful for "I've seen these, destroy
        /// them now" sweeps (e.g. clearing accumulated OTP codes the
        /// moment they've been used). The extractor gate is preserved
        /// — uncaptured data is still never destroyed.
        #[arg(long)]
        now: bool,

        /// Only run this named policy (match by .name field)
        #[arg(long)]
        only: Option<String>,

        /// Disable LLM fallback for this run. Vendor modules that would
        /// normally call Claude when their deterministic extractor leaves
        /// a required field missing will simply skip that field instead.
        #[arg(long)]
        llm_disable: bool,

        /// Cap on total LLM calls in this run, across all policies.
        /// Default 100 — enough for steady-state and a reasonable
        /// backfill chunk; protects against runaway loops where a
        /// regressed deterministic extractor leaves every message
        /// needing LLM. Set higher (e.g. 5000) when doing a one-shot
        /// historical backfill.
        #[arg(long, default_value_t = 100)]
        llm_budget: usize,
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
    /// Sample messages from the inbox and classify via Claude, building the
    /// labelled corpus used by `eval` and `improve`.
    Label {
        /// How many messages to sample + classify
        #[arg(short, long, default_value_t = 50)]
        sample: usize,
        /// Date window (notmuch date: syntax)
        #[arg(long, default_value = "6M")]
        window: String,
    },
    /// Compute precision/recall for each policy against the labelled corpus.
    /// Only policies with `intended_categories` set are scored.
    Eval,
    /// Run the Karpathy-loop on one policy: propose edits, test, keep-or-revert.
    /// Writes successful edits to policies.toml; logs all attempts to
    /// ~/.local/share/mailcurator/eval/tried_edits.jsonl.
    Improve {
        /// Policy name to improve
        name: String,
        /// Number of proposal/test rounds to attempt
        #[arg(short, long, default_value_t = 5)]
        rounds: usize,
    },
    /// Subscription monitoring: list known subscriptions, alert on upcoming
    /// renewals approaching their cancellation window, discover new ones.
    /// Schema and module contract: see SUBSCRIPTIONS.md.
    Subscriptions {
        #[command(subcommand)]
        action: SubscriptionsAction,
    },
    /// Show messages each policy with `delete_after_days` set would trash on
    /// the next live run. Helpful before enabling destruction on a policy:
    /// see exactly which messages are caught by the age + extracted gate
    /// before flipping the flag.
    DestroyPreview {
        /// Optionally limit to one named policy
        #[arg(short, long)]
        only: Option<String>,
        /// Max messages to show per policy
        #[arg(short, long, default_value_t = 30)]
        limit: usize,
    },
    /// Report extractor field-population rates per policy. When a policy
    /// declares a `vendor_module`, the module's required_fields drive a
    /// "health" % — sustained drops in this number indicate a vendor
    /// changed their email template.
    ///
    /// Each run also appends a snapshot to coverage-history.jsonl so that
    /// `--drift` can compare current rates to the previous snapshot and
    /// flag regressions. Drop the `--drift` flag while iterating on a new
    /// extractor — first snapshot for a policy is always silent.
    Coverage {
        /// Limit to one policy
        #[arg(short, long)]
        policy: Option<String>,
        /// Compare current coverage to the most recent prior snapshot per
        /// policy and flag drops >= --threshold pp. Exits with non-zero
        /// status when drift is detected (cron-friendly).
        #[arg(long)]
        drift: bool,
        /// Drift threshold in percentage points. Default 10.
        #[arg(long, default_value_t = 10.0)]
        threshold: f64,
        /// Skip writing today's snapshot to coverage-history.jsonl. Useful
        /// when you're spot-checking and don't want to pollute the history.
        #[arg(long)]
        no_snapshot: bool,
    },
    /// Query the orders.jsonl extracted store. The point of
    /// extract-and-destroy is that this becomes the authoritative source
    /// of truth for past Amazon orders — reach for `mailcurator orders`
    /// before grepping email.
    Orders {
        #[command(subcommand)]
        action: OrdersAction,
    },
    /// Query the journeys.jsonl extracted store (Trainline + future
    /// rail/coach vendors).
    Journeys {
        #[command(subcommand)]
        action: JourneysAction,
    },
    /// Query the bookings.jsonl extracted store (Airbnb + future
    /// hotel/villa vendors).
    Bookings {
        #[command(subcommand)]
        action: BookingsAction,
    },
    /// Query the tesla.jsonl extracted store. Tesla emails span auth,
    /// service appointments, supercharger receipts, software releases —
    /// most useful queries filter on the `kind` field.
    Tesla {
        #[command(subcommand)]
        action: TeslaAction,
    },
    /// Query the bills.jsonl extracted store. Multi-vendor (Octopus,
    /// Vodafone, BT, Direct Line, PayPal merchants, etc.) so this is a
    /// flat lookup rather than the subcommand pattern other stores use.
    /// Vendor matching is case-insensitive substring against vendor
    /// (utility rows) or counterparty (PayPal rows).
    Bills {
        /// Substring match against vendor / counterparty (case-insensitive).
        #[arg(long)]
        vendor: Option<String>,
        /// Filter to a specific received year (UTC).
        #[arg(long)]
        year: Option<i32>,
        /// Maximum rows to print.
        #[arg(short, long, default_value_t = 30)]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum JourneysAction {
    List {
        #[arg(long)]
        year: Option<i32>,
        #[arg(short, long, default_value_t = 30)]
        limit: usize,
    },
    Find {
        query: String,
        #[arg(short, long, default_value_t = 30)]
        limit: usize,
    },
    Recent {
        #[arg(long, default_value_t = 30)]
        days: i64,
        #[arg(short, long, default_value_t = 30)]
        limit: usize,
    },
    Total {
        #[arg(long)]
        year: Option<i32>,
    },
}

#[derive(Subcommand)]
enum BookingsAction {
    /// Bookings whose check-in date is in the future. The most-useful
    /// daily query — answers "what's coming up?".
    Upcoming {
        #[arg(short, long, default_value_t = 20)]
        limit: usize,
    },
    List {
        #[arg(long)]
        year: Option<i32>,
        #[arg(short, long, default_value_t = 30)]
        limit: usize,
    },
    Find {
        query: String,
        #[arg(short, long, default_value_t = 30)]
        limit: usize,
    },
    /// Show ALL extracted fields for a booking matching the query.
    /// Default groups by identity; --all shows every email's record.
    Show {
        query: String,
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
enum TeslaAction {
    /// Filtered list — e.g. --kind service, --kind supercharger.
    List {
        #[arg(long)]
        year: Option<i32>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(short, long, default_value_t = 30)]
        limit: usize,
    },
    /// Per-kind summary + total amount captured.
    Summary,
    Find {
        query: String,
        #[arg(short, long, default_value_t = 30)]
        limit: usize,
    },
}

#[derive(Subcommand)]
enum OrdersAction {
    /// List orders, newest first.
    List {
        /// Filter to a specific year.
        #[arg(long)]
        year: Option<i32>,
        /// Maximum rows.
        #[arg(short, long, default_value_t = 30)]
        limit: usize,
    },
    /// Find orders whose subject, order_id, or items contain a substring.
    Find {
        /// Substring to search (case-insensitive).
        query: String,
        #[arg(short, long, default_value_t = 30)]
        limit: usize,
    },
    /// Show orders received in the last N days.
    Recent {
        #[arg(long, default_value_t = 30)]
        days: i64,
        #[arg(short, long, default_value_t = 30)]
        limit: usize,
    },
    /// Sum totals (where populated). Useful for tax-year aggregation.
    Total {
        #[arg(long)]
        year: Option<i32>,
    },
}

#[derive(Subcommand)]
enum SubscriptionsAction {
    /// Print all known subscriptions sorted by next_renewal.
    List,
    /// Flag subscriptions approaching their cancellation window.
    Check {
        /// Write actionable alerts to today's DayPage via daypage-append.
        #[arg(long)]
        alert: bool,
        /// Extra buffer days beyond cancellation_notice_days.
        #[arg(long, default_value_t = 7)]
        buffer_days: i64,
    },
    /// Periodic hygiene digest: totals by frequency, dormant services.
    Report {
        /// Date window for activity (notmuch date: syntax).
        #[arg(long, default_value = "30d")]
        period: String,
    },
    /// Heuristic scan for new subscription candidates (Track A).
    Discover {
        /// Persist candidates to subscriptions.jsonl. Without this flag, prints only.
        #[arg(long)]
        commit: bool,
        /// Date window to scan.
        #[arg(long, default_value = "6M")]
        window: String,
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
        Command::Run { dry_run, now, only, llm_disable, llm_budget } => {
            // Multi-machine leadership: skip clean if another machine has been
            // active more recently. Bypass with MAILCURATOR_FORCE=1 (e.g. for
            // backfill or explicit operator runs). Dry runs also honour the
            // gate — easier to reason about, and cheap to override when
            // genuinely needed. See src/leader.rs for the protocol.
            let decision = leader::should_run();
            let id = leader::machine_id();
            match &decision {
                leader::RunDecision::Run | leader::RunDecision::ForcedRun => {
                    eprintln!("{}", leader::explain_decision(&decision, &id));
                }
                leader::RunDecision::SkipOtherActive { .. }
                | leader::RunDecision::SkipNoRecentActivity { .. } => {
                    eprintln!("{}", leader::explain_decision(&decision, &id));
                    return Ok(());
                }
            }

            let cfg = config::load(&path)?;
            extract::set_llm_budget(llm_budget);
            if llm_disable {
                extract::disable_llm_fallback();
            }
            let mut total_tagged = 0u64;
            let mut total_archived = 0u64;
            let mut total_deleted = 0u64;

            for pol in &cfg.policies {
                if let Some(name) = &only {
                    if &pol.name != name {
                        continue;
                    }
                }
                let stats = policy::apply(pol, dry_run, now)
                    .with_context(|| format!("policy '{}' failed", pol.name))?;
                total_tagged += stats.tagged_on_arrival;
                total_archived += stats.archived;
                total_deleted += stats.deleted;
                let quarantine_note = if pol.quarantine { "  [QUARANTINE]" } else { "" };
                println!(
                    "{:<30}  +arrival={:<4}  ⤓extracted={:<4}  →archive={:<4}  →trash={:<4}{}",
                    pol.name,
                    stats.tagged_on_arrival,
                    stats.extracted,
                    stats.archived,
                    stats.deleted,
                    quarantine_note
                );
            }

            println!();
            let llm_used = extract::llm_calls_made();
            let llm_note = if llm_used > 0 {
                format!("  llm-calls={} (budget={})", llm_used, llm_budget)
            } else if llm_disable {
                "  [llm fallback disabled]".to_string()
            } else {
                String::new()
            };
            println!(
                "TOTAL  tagged-on-arrival={}  archived={}  trashed={}{}{}",
                total_tagged,
                total_archived,
                total_deleted,
                if dry_run { "  [DRY RUN — no changes made]" } else { "" },
                llm_note
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
        Command::Label { sample, window } => {
            eval::label(sample, &window)?;
        }
        Command::Eval => {
            let cfg = config::load(&path)?;
            let labels = eval::load_labels()?;
            if labels.is_empty() {
                println!("no labels found — run `mailcurator label --sample N` first");
                return Ok(());
            }
            println!("labels: {}", labels.len());
            let scores = eval::score_all(&cfg.policies, &labels)?;
            if scores.is_empty() {
                println!("no policies have `intended_categories` set — add the field to opt a policy into the eval loop");
                return Ok(());
            }
            println!();
            println!(
                "{:<25}  {:>9}  {:>7}  {:>3}  {:>3}  {:>3}  {:>3}  {}",
                "policy", "precision", "recall", "TP", "FP", "FN", "TN", "intended"
            );
            for s in &scores {
                println!(
                    "{:<25}  {:>9.3}  {:>7.3}  {:>3}  {:>3}  {:>3}  {:>3}  {}",
                    s.policy_name,
                    s.precision(),
                    s.recall(),
                    s.tp, s.fp, s.fn_count, s.tn,
                    s.intended_categories.join(",")
                );
            }
            // Show FPs/FNs inline for diagnosis.
            for s in &scores {
                if s.fp > 0 || s.fn_count > 0 {
                    println!("\n--- {} ---", s.policy_name);
                    if !s.fp_examples.is_empty() {
                        println!("  false positives:");
                        for e in &s.fp_examples {
                            println!("    [{}] {} | {}",
                                e.category,
                                truncate(&e.from, 40),
                                truncate(&e.subject, 80));
                        }
                    }
                    if !s.fn_examples.is_empty() {
                        println!("  false negatives:");
                        for e in &s.fn_examples {
                            println!("    [{}] {} | {}",
                                e.category,
                                truncate(&e.from, 40),
                                truncate(&e.subject, 80));
                        }
                    }
                }
            }
        }
        Command::Improve { name, rounds } => {
            eval::improve(&name, rounds)?;
        }
        Command::DestroyPreview { only, limit } => {
            let cfg = config::load(&path)?;
            let mut total_caught = 0u64;
            let mut policies_with_delete = 0u64;
            for pol in &cfg.policies {
                let Some(delete_days) = pol.delete_after_days else {
                    continue;
                };
                if let Some(name) = &only {
                    if &pol.name != name {
                        continue;
                    }
                }
                policies_with_delete += 1;
                let base = pol.base_query();
                let seen = pol.seen_tag();
                let mut parts: Vec<String> =
                    vec![format!("({base})"), format!("tag:{seen}")];
                if !pol.extractors.is_empty() {
                    parts.push(format!("tag:{}", pol.extracted_tag()));
                }
                parts.push(format!("date:..-{}d", delete_days));
                parts.push("not tag:trash".to_string());
                let query = parts.join(" and ");
                let messages = store::list_messages(&query)?;
                println!(
                    "=== {} (delete_after_days={}) ===",
                    pol.name, delete_days
                );
                println!("query: {query}");
                println!("would trash: {} messages", messages.len());
                for m in messages.iter().take(limit) {
                    println!(
                        "  {:<12}  {:<40}  {}",
                        truncate(&m.date, 12),
                        truncate(&m.from, 40),
                        truncate(&m.subject, 100)
                    );
                }
                if messages.len() > limit {
                    println!("  ... and {} more", messages.len() - limit);
                }
                println!();
                total_caught += messages.len() as u64;
            }
            if policies_with_delete == 0 {
                println!("no policies with delete_after_days set");
            } else {
                println!(
                    "TOTAL across {} delete-enabled policies: {} messages would trash",
                    policies_with_delete, total_caught
                );
            }
        }
        Command::Coverage { policy, drift, threshold, no_snapshot } => {
            let cfg = config::load(&path)?;
            let reports = coverage::report_all(&cfg.policies, policy.as_deref())?;
            coverage::print_reports(&reports);
            if drift {
                println!();
                let d = coverage::drift(&reports, threshold)?;
                coverage::print_drift(&d);
                if !d.findings.is_empty() {
                    std::process::exit(1);
                }
            }
            if !no_snapshot {
                coverage::snapshot(&reports)?;
            }
        }
        Command::Orders { action } => match action {
            OrdersAction::List { year, limit } => orders_cli::list(year, limit)?,
            OrdersAction::Find { query, limit } => orders_cli::find(&query, limit)?,
            OrdersAction::Recent { days, limit } => orders_cli::recent(days, limit)?,
            OrdersAction::Total { year } => orders_cli::total(year)?,
        },
        Command::Journeys { action } => match action {
            JourneysAction::List { year, limit } => journeys_cli::list(year, limit)?,
            JourneysAction::Find { query, limit } => journeys_cli::find(&query, limit)?,
            JourneysAction::Recent { days, limit } => journeys_cli::recent(days, limit)?,
            JourneysAction::Total { year } => journeys_cli::total(year)?,
        },
        Command::Bookings { action } => match action {
            BookingsAction::Upcoming { limit } => bookings_cli::upcoming(limit)?,
            BookingsAction::List { year, limit } => bookings_cli::list(year, limit)?,
            BookingsAction::Find { query, limit } => bookings_cli::find(&query, limit)?,
            BookingsAction::Show { query, all } => bookings_cli::show(&query, all)?,
        },
        Command::Tesla { action } => match action {
            TeslaAction::List { year, kind, limit } => {
                tesla_cli::list(year, kind.as_deref(), limit)?
            }
            TeslaAction::Summary => tesla_cli::summary()?,
            TeslaAction::Find { query, limit } => tesla_cli::find(&query, limit)?,
        },
        Command::Bills { vendor, year, limit } => {
            bills_cli::list(vendor.as_deref(), year, limit)?
        }
        Command::Subscriptions { action } => match action {
            SubscriptionsAction::List => subscriptions::list()?,
            SubscriptionsAction::Check { alert, buffer_days } => {
                subscriptions::check(alert, buffer_days)?
            }
            SubscriptionsAction::Report { period } => subscriptions::report(&period)?,
            SubscriptionsAction::Discover { commit, window } => {
                subscriptions::discover(commit, &window)?
            }
        },
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
