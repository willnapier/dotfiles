use chrono::{Datelike, NaiveDate};
use clap::{Parser, Subcommand};
use fd_budget::{
    dedup, enrich, import, paypal, query,
    store::CsvStore,
    tags::{apply_rules, apply_rules_with_recovery, reapply_rules, TagRules},
    Account,
};
use rust_decimal::Decimal;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "fd-budget")]
#[command(about = "First Direct budget analysis tool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Import transactions from a midata CSV file
    Import {
        /// Path to the midata CSV file
        file: PathBuf,
        /// Account type (current or visa)
        #[arg(short, long)]
        account: Account,
    },
    /// Manage tag rules
    Tag {
        #[command(subcommand)]
        action: TagAction,
    },
    /// List untagged transactions. By default only debits (the spend-
    /// categorisation worklist) are shown; credits (income/refunds) are
    /// separable by sign and excluded unless `--include-credits` is given.
    Untagged {
        /// Limit output to N transactions
        #[arg(short, long)]
        limit: Option<usize>,
        /// Also show untagged credit (incoming) rows
        #[arg(long)]
        include_credits: bool,
    },
    /// Show statistics. With `--by-counterparty`, aggregates outgoing spend
    /// per counterparty, joining transactions.csv against matches.jsonl and
    /// bills.jsonl. With `--by-category`, breaks the personal Spend floor down
    /// per category (the row's primary tag), summing EXACTLY to the Spend floor.
    /// Without flags, prints the original tag/account summary.
    Stats {
        /// Aggregate outgoing spend per counterparty (Stage 2 query)
        #[arg(long, conflicts_with = "by_category")]
        by_counterparty: bool,
        /// Break the Spend floor down per category (primary tag). Each spend row
        /// is attributed to exactly one bucket; untagged spend -> "uncategorised".
        /// Per-category totals reconcile exactly to the Spend floor.
        #[arg(long)]
        by_category: bool,
        /// With --by-category, also print a super-category roll-up (Home, Bills,
        /// Food, Transport, …). Mapping is a refinable default. No effect otherwise.
        #[arg(long)]
        rollup: bool,
        /// Filter to a single calendar year
        #[arg(long)]
        year: Option<i32>,
        /// Filter to a single calendar month (YYYY-MM)
        #[arg(long)]
        month: Option<String>,
        /// Filter rows on or after this date (YYYY-MM-DD)
        #[arg(long)]
        since: Option<NaiveDate>,
        /// Maximum rows to print (default 30)
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// Drill into individual bank rows.
    ///
    /// Two sub-actions: `vendor <NAME>` (filter by counterparty substring)
    /// and `unmatched` (rows with confidence == "none").
    ///
    /// Note: the spec sketches `tx --vendor <NAME>` as a flag, but `vendor`
    /// and `unmatched` are mutually exclusive sub-actions, so clap-idiomatic
    /// subcommands are clearer. Functionally equivalent.
    Tx {
        #[command(subcommand)]
        action: TxAction,
    },
    /// Interactively categorize untagged transactions
    Categorize {
        /// Maximum number of transactions to process
        #[arg(short, long)]
        limit: Option<usize>,
        /// Also categorize untagged credit (incoming) rows
        #[arg(long)]
        include_credits: bool,
    },
    /// Reconcile bank rows against mailcurator email-evidence
    Enrich {
        /// Path to mailcurator bills.jsonl
        #[arg(long)]
        from: Option<PathBuf>,
        /// Path to write matches.jsonl
        #[arg(long)]
        out: Option<PathBuf>,
        /// Amount tolerance (default 0.01)
        #[arg(long)]
        amount_tolerance: Option<Decimal>,
        /// Date window in days (default 3)
        #[arg(long)]
        date_window: Option<i64>,
        /// Print summary only; don't write file
        #[arg(long)]
        dry_run: bool,
    },
    /// Recover PayPal merchants stripped by First Direct's export.
    ///
    /// FD posts every PayPal purchase as a bare `PAYPAL PAYMENT  -£X`. PayPal's
    /// own CSV export carries the merchant. `paypal import` loads that export
    /// into a sidecar (`paypal.csv`); `paypal recover` joins it back to the bank
    /// rows and writes `paypal_matches.jsonl` (a sidecar keyed by
    /// bank_import_id) — `transactions.csv` is never rewritten.
    Paypal {
        #[command(subcommand)]
        action: PaypalAction,
    },
}

#[derive(Subcommand)]
enum PaypalAction {
    /// Import one or more PayPal activity CSV exports into the sidecar
    /// (`paypal.csv`). Idempotent by PayPal `Transaction ID` across overlapping
    /// date-range exports.
    Import {
        /// Path(s) to PayPal CSV export(s) (UTF-8-with-BOM, 15 columns).
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Join the PayPal sidecar to bank `PAYPAL PAYMENT` rows and recover the
    /// merchant for each, writing `paypal_matches.jsonl`.
    Recover {
        /// Path to write the recovery sidecar (default
        /// ~/.config/fd-budget/paypal_matches.jsonl).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Bank↔PayPal date window in days (default 5).
        #[arg(long)]
        window: Option<i64>,
        /// Print the summary only; don't write the sidecar.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum TxAction {
    /// List bank rows matching a vendor (substring, case-insensitive).
    Vendor {
        /// Vendor substring (case-insensitive).
        name: String,
        /// Show resolved email-evidence message-id for each row.
        #[arg(long)]
        with_evidence: bool,
        /// Filter to a single calendar year.
        #[arg(long)]
        year: Option<i32>,
        /// Filter to a single calendar month (YYYY-MM).
        #[arg(long)]
        month: Option<String>,
        /// Filter rows on or after this date (YYYY-MM-DD).
        #[arg(long)]
        since: Option<NaiveDate>,
    },
    /// List bank rows with no email evidence (confidence == "none").
    Unmatched {
        /// Only show rows where |amount| >= this threshold.
        #[arg(long)]
        over: Option<Decimal>,
        /// Filter to a single calendar year.
        #[arg(long)]
        year: Option<i32>,
        /// Filter to a single calendar month (YYYY-MM).
        #[arg(long)]
        month: Option<String>,
        /// Filter rows on or after this date (YYYY-MM-DD).
        #[arg(long)]
        since: Option<NaiveDate>,
    },
}

#[derive(Subcommand)]
enum TagAction {
    /// Add a tag rule
    Add {
        /// Pattern to match (case-insensitive substring)
        pattern: String,
        /// Tags to apply
        #[arg(required = true)]
        tags: Vec<String>,
        /// Exact amount to match
        #[arg(long)]
        amount: Option<Decimal>,
        /// Minimum amount for range match
        #[arg(long)]
        min_amount: Option<Decimal>,
        /// Maximum amount for range match
        #[arg(long)]
        max_amount: Option<Decimal>,
        /// Day of month to match (1-31)
        #[arg(long)]
        day_of_month: Option<u32>,
        /// Tolerance in days around day-of-month (default 0)
        #[arg(long)]
        day_window: Option<u32>,
    },
    /// Remove tag(s) from a pattern
    Remove {
        /// Pattern
        pattern: String,
        /// Tags to remove
        #[arg(required = true)]
        tags: Vec<String>,
    },
    /// List all rules
    List,
    /// Test what tags would apply to a description
    Test {
        /// Description to test
        description: String,
    },
    /// Re-apply all rules to existing transactions. Additive by default
    /// (preserves manual / `categorize` tags); pass --reset to clear all
    /// tags first and rebuild purely from rules.
    Reapply {
        /// Clear all existing tags before re-applying. DESTRUCTIVE: drops any
        /// manual tags not reproducible from rules. Without this flag, reapply
        /// only ADDS rule matches to existing tags.
        #[arg(long)]
        reset: bool,
    },
}

fn get_data_dir() -> PathBuf {
    // Use ~/.config/fd-budget for cross-platform consistency
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("fd-budget")
}

fn get_store_path() -> PathBuf {
    get_data_dir().join("transactions.csv")
}

fn get_rules_path() -> PathBuf {
    get_data_dir().join("rules.toml")
}

fn ensure_data_dir() -> std::io::Result<()> {
    std::fs::create_dir_all(get_data_dir())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    ensure_data_dir()?;

    match cli.command {
        Commands::Import { file, account } => {
            cmd_import(&file, account)?;
        }
        Commands::Tag { action } => {
            cmd_tag(action)?;
        }
        Commands::Untagged {
            limit,
            include_credits,
        } => {
            cmd_untagged(limit, include_credits)?;
        }
        Commands::Stats {
            by_counterparty,
            by_category,
            rollup,
            year,
            month,
            since,
            limit,
        } => {
            if by_counterparty {
                cmd_stats_by_counterparty(year, month.as_deref(), since, limit)?;
            } else if by_category {
                cmd_stats_by_category(year, month.as_deref(), since, limit, rollup)?;
            } else {
                cmd_stats(year, month.as_deref(), since)?;
            }
        }
        Commands::Tx { action } => {
            cmd_tx(action)?;
        }
        Commands::Categorize {
            limit,
            include_credits,
        } => {
            cmd_categorize(limit, include_credits)?;
        }
        Commands::Enrich {
            from,
            out,
            amount_tolerance,
            date_window,
            dry_run,
        } => {
            cmd_enrich(from, out, amount_tolerance, date_window, dry_run)?;
        }
        Commands::Paypal { action } => {
            cmd_paypal(action)?;
        }
    }

    Ok(())
}

fn default_bills_jsonl() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/share/mailcurator/bills.jsonl")
}

fn default_matches_jsonl() -> PathBuf {
    get_data_dir().join("matches.jsonl")
}

/// The PayPal sidecar CSV (typed PayPal rows). Mirrors `get_store_path()`.
fn get_paypal_store_path() -> PathBuf {
    get_data_dir().join("paypal.csv")
}

/// The PayPal merchant-recovery sidecar (keyed by bank_import_id). Mirrors
/// `default_matches_jsonl()`.
fn default_paypal_matches_jsonl() -> PathBuf {
    get_data_dir().join("paypal_matches.jsonl")
}

/// Snapshot `transactions.csv` to `/tmp` before a destructive rewrite (primer
/// rule 3). The canonical bank timeline is immutable in spirit; tag writes are
/// the one allowed mutation, and they must be recoverable. No-op if the store
/// does not exist yet.
fn snapshot_transactions(store_path: &PathBuf) -> std::io::Result<()> {
    if !store_path.exists() {
        return Ok(());
    }
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let backup = std::env::temp_dir().join(format!("fd-budget-transactions.backup-{stamp}.csv"));
    std::fs::copy(store_path, &backup)?;
    eprintln!("Snapshotted transactions.csv -> {}", backup.display());
    Ok(())
}

fn cmd_enrich(
    from: Option<PathBuf>,
    out: Option<PathBuf>,
    amount_tolerance: Option<Decimal>,
    date_window: Option<i64>,
    dry_run: bool,
) -> anyhow::Result<()> {
    use enrich::{Confidence, MatchOptions};

    let bills_path = from.unwrap_or_else(default_bills_jsonl);
    let out_path = out.unwrap_or_else(default_matches_jsonl);

    let store = CsvStore::new(get_store_path());
    let transactions = store.load_all()?;
    if transactions.is_empty() {
        eprintln!("No transactions in store; run `fd-budget import` first.");
        return Ok(());
    }

    let email_rows = enrich::load_email_rows(&bills_path)
        .map_err(|e| anyhow::anyhow!("failed to load {}: {}", bills_path.display(), e))?;

    let mut opts = MatchOptions::default();
    if let Some(t) = amount_tolerance {
        opts.amount_tolerance = t;
    }
    if let Some(w) = date_window {
        opts.date_window_days = w;
    }

    let (results, summary) = enrich::enrich(&transactions, &email_rows, opts);

    let total = summary.bank_rows;
    let none_count = summary.count(Confidence::None);
    let enriched = total.saturating_sub(none_count);

    let pct = |n: usize| -> f64 {
        if total == 0 {
            0.0
        } else {
            (n as f64 / total as f64) * 100.0
        }
    };

    println!(
        "Reconciled {} bank rows against {} email rows.",
        summary.bank_rows, summary.email_rows
    );
    let high = summary.count(Confidence::High);
    let medium = summary.count(Confidence::Medium);
    let ambiguous = summary.count(Confidence::Ambiguous);
    let internal = summary.count(Confidence::InternalTransfer);
    println!("  high              {:>4} ({:.1}%)", high, pct(high));
    println!("  medium            {:>4} ({:.1}%)", medium, pct(medium));
    println!(
        "  ambiguous         {:>4} ({:.1}%)",
        ambiguous,
        pct(ambiguous)
    );
    println!(
        "  internal-transfer {:>4} ({:.1}%)",
        internal,
        pct(internal)
    );
    println!(
        "  none              {:>4} ({:.1}%)",
        none_count,
        pct(none_count)
    );

    if dry_run {
        println!("(dry-run; no file written)");
    } else {
        enrich::write_matches(&out_path, &results)?;
        // All rows are written (none-rows too) so downstream queries can join cleanly;
        // the "omitted" wording mirrors the spec's user-facing summary.
        println!(
            "Wrote {} ({} enriched rows; {} none rows omitted)",
            out_path.display(),
            enriched,
            none_count
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// PayPal merchant recovery (paypal import / paypal recover)
// ---------------------------------------------------------------------------

fn cmd_paypal(action: PaypalAction) -> anyhow::Result<()> {
    match action {
        PaypalAction::Import { files } => cmd_paypal_import(&files),
        PaypalAction::Recover {
            out,
            window,
            dry_run,
        } => cmd_paypal_recover(out, window, dry_run),
    }
}

fn cmd_paypal_import(files: &[PathBuf]) -> anyhow::Result<()> {
    let store = paypal::PayPalStore::new(get_paypal_store_path());
    let mut existing = store.load_transaction_ids()?;
    eprintln!("Loaded {} existing PayPal rows", existing.len());

    let mut total_parsed = 0usize;
    let mut total_imported = 0usize;
    for file in files {
        let reader = BufReader::new(File::open(file)?);
        let parsed = paypal::parse_paypal_csv(reader)
            .map_err(|e| anyhow::anyhow!("failed to parse {}: {}", file.display(), e))?;
        total_parsed += parsed.len();
        // Dedup against the store AND against rows already imported this run
        // (overlapping export files), then grow the seen-set.
        let fresh = paypal::deduplicate(parsed, &existing);
        for r in &fresh {
            existing.insert(r.transaction_id.clone());
        }
        let n = store.append(&fresh)?;
        total_imported += n;
        eprintln!("  {}: {} new rows", file.display(), n);
    }

    println!(
        "Imported {} new PayPal rows ({} parsed across {} file(s)).",
        total_imported,
        total_parsed,
        files.len()
    );
    Ok(())
}

fn cmd_paypal_recover(
    out: Option<PathBuf>,
    window: Option<i64>,
    dry_run: bool,
) -> anyhow::Result<()> {
    let out_path = out.unwrap_or_else(default_paypal_matches_jsonl);

    let store = CsvStore::new(get_store_path());
    let transactions = store.load_all()?;
    if transactions.is_empty() {
        eprintln!("No transactions in store; run `fd-budget import` first.");
        return Ok(());
    }

    let pp_store = paypal::PayPalStore::new(get_paypal_store_path());
    let paypal_rows = pp_store.load_all()?;
    if paypal_rows.is_empty() {
        eprintln!(
            "No PayPal rows in {}; run `fd-budget paypal import <file>` first.",
            get_paypal_store_path().display()
        );
        return Ok(());
    }

    let mut opts = paypal::RecoverOptions::default();
    if let Some(w) = window {
        opts.bank_window_days = w;
    }

    let (recoveries, summary) = paypal::recover(&transactions, &paypal_rows, opts);

    println!(
        "Scanned {} bank PAYPAL rows against {} PayPal rows.",
        summary.bare_paypal_rows,
        paypal_rows.len()
    );
    println!(
        "  recovered        {:>4} / {} merchants",
        summary.recovered, summary.bare_paypal_rows
    );
    println!("    direct-gbp     {:>4}", summary.direct_gbp);
    println!("    two-leg        {:>4}", summary.two_leg);
    println!("    fx-chain       {:>4}", summary.fx_chain);
    println!(
        "  £-value recovered  £{:.2} / £{:.2} ({:.1}%)",
        summary.recovered_value,
        summary.total_value,
        summary.pct_value_recovered()
    );

    if dry_run {
        println!("(dry-run; no file written)");
    } else {
        paypal::write_recoveries(&out_path, &recoveries)?;
        println!(
            "Wrote {} ({} recoveries).",
            out_path.display(),
            recoveries.len()
        );
        println!(
            "Tip: `fd-budget tag reapply` now also tags PAYPAL rows by their recovered merchant."
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Stage 2 query commands (stats --by-counterparty, tx vendor, tx unmatched)
// ---------------------------------------------------------------------------

fn cmd_stats_by_counterparty(
    year: Option<i32>,
    month: Option<&str>,
    since: Option<NaiveDate>,
    limit: usize,
) -> anyhow::Result<()> {
    let (txs, emails, matches) = query::load_all(
        &get_store_path(),
        &default_bills_jsonl(),
        &default_matches_jsonl(),
    )?;
    let recoveries = query::load_recovery_index(&default_paypal_matches_jsonl())?;
    let joined = query::join_with_recovery(&txs, &emails, &matches, &recoveries);
    let filter = query::DateFilter::from_flags(year, month, since)?;
    query::cmd_stats_by_counterparty(&joined, filter, limit)
}

fn cmd_stats_by_category(
    year: Option<i32>,
    month: Option<&str>,
    since: Option<NaiveDate>,
    limit: usize,
    rollup: bool,
) -> anyhow::Result<()> {
    // Category = the row's primary tag, which lives on the transaction itself, so
    // this needs only transactions.csv — no matches.jsonl / bills.jsonl join.
    let store = CsvStore::new(get_store_path());
    let transactions = store.load_all()?;
    if transactions.is_empty() {
        eprintln!("No transactions in store");
        return Ok(());
    }
    let filter = query::DateFilter::from_flags(year, month, since)?;
    query::cmd_stats_by_category(&transactions, filter, limit, rollup)
}

fn cmd_tx(action: TxAction) -> anyhow::Result<()> {
    let (txs, emails, matches) = query::load_all(
        &get_store_path(),
        &default_bills_jsonl(),
        &default_matches_jsonl(),
    )?;
    let recoveries = query::load_recovery_index(&default_paypal_matches_jsonl())?;
    let joined = query::join_with_recovery(&txs, &emails, &matches, &recoveries);

    match action {
        TxAction::Vendor {
            name,
            with_evidence,
            year,
            month,
            since,
        } => {
            let filter = query::DateFilter::from_flags(year, month.as_deref(), since)?;
            query::cmd_tx_by_vendor(&joined, filter, &name, with_evidence)
        }
        TxAction::Unmatched {
            over,
            year,
            month,
            since,
        } => {
            let filter = query::DateFilter::from_flags(year, month.as_deref(), since)?;
            query::cmd_tx_unmatched(&joined, filter, over)
        }
    }
}

fn cmd_import(file: &PathBuf, account: Account) -> anyhow::Result<()> {
    let store = CsvStore::new(get_store_path());
    let rules = TagRules::load(get_rules_path())?;

    // Load existing IDs for deduplication
    let existing_ids = store.load_import_ids()?;
    eprintln!("Loaded {} existing transactions", existing_ids.len());

    // Parse the export. The Visa card uses a 4-column schema
    // (Date, Description, Amount, Reference). The current account's midata
    // changed over time: the legacy export is 5-column (Date, Type,
    // Merchant/Description, Debit/Credit, Balance); FD's current export is a
    // leaner 4-column one (Date, Description, Amount, Balance) — no Type, a
    // single signed Amount. Peek the header to pick the right layout so both
    // old and new current-account downloads import. `--account` stays
    // authoritative for the account label.
    let header = {
        let f = BufReader::new(File::open(file)?);
        f.lines().next().transpose()?.unwrap_or_default()
    };
    let reader = BufReader::new(File::open(file)?);
    let transactions = match account {
        Account::Visa => import::parse_midata_visa(reader, account)?,
        Account::Current => {
            let has_type = header
                .split(',')
                .any(|h| h.trim().eq_ignore_ascii_case("type"));
            if has_type {
                import::parse_midata(reader, account)?
            } else {
                import::parse_midata_current_4col(reader, account)?
            }
        }
    };
    eprintln!("Parsed {} transactions from file", transactions.len());

    // Deduplicate
    let transactions = dedup::deduplicate(transactions, &existing_ids);
    eprintln!(
        "{} new transactions after deduplication",
        transactions.len()
    );

    if transactions.is_empty() {
        eprintln!("No new transactions to import");
        return Ok(());
    }

    // Apply tag rules
    let mut transactions = transactions;
    apply_rules(&mut transactions, &rules);

    // Count tagged
    let tagged_count = transactions.iter().filter(|t| !t.tags.is_empty()).count();
    eprintln!("{} transactions auto-tagged", tagged_count);

    // Append to store
    let count = store.append(&transactions)?;
    eprintln!("Imported {} transactions", count);

    Ok(())
}

fn cmd_tag(action: TagAction) -> anyhow::Result<()> {
    let rules_path = get_rules_path();
    let mut rules = TagRules::load(&rules_path)?;

    match action {
        TagAction::Add {
            pattern,
            tags,
            amount,
            min_amount,
            max_amount,
            day_of_month,
            day_window,
        } => {
            rules.add_rule(
                &pattern,
                tags.clone(),
                amount,
                min_amount,
                max_amount,
                day_of_month,
                day_window,
            );
            rules.save(&rules_path)?;
            let mut msg = format!("Added rule: {}", pattern);
            if let Some(a) = amount {
                msg.push_str(&format!(" [amount={}]", a));
            }
            if let Some(min) = min_amount {
                msg.push_str(&format!(" [min={}]", min));
            }
            if let Some(max) = max_amount {
                msg.push_str(&format!(" [max={}]", max));
            }
            if let Some(day) = day_of_month {
                let window = day_window.unwrap_or(0);
                if window > 0 {
                    msg.push_str(&format!(" [day={}+/-{}]", day, window));
                } else {
                    msg.push_str(&format!(" [day={}]", day));
                }
            }
            msg.push_str(&format!(" -> {:?}", tags));
            eprintln!("{}", msg);
        }
        TagAction::Remove { pattern, tags } => {
            rules.remove_tags(&pattern, &tags);
            rules.save(&rules_path)?;
            eprintln!("Removed tags {:?} from pattern '{}'", tags, pattern);
        }
        TagAction::List => {
            if rules.rules.is_empty() {
                eprintln!("No rules defined");
            } else {
                for rule in &rules.rules {
                    let mut conditions = vec![format!("pattern={}", rule.pattern)];
                    if let Some(a) = rule.amount {
                        conditions.push(format!("amount={}", a));
                    }
                    if let Some(min) = rule.min_amount {
                        conditions.push(format!("min={}", min));
                    }
                    if let Some(max) = rule.max_amount {
                        conditions.push(format!("max={}", max));
                    }
                    if let Some(day) = rule.day_of_month {
                        let window = rule.day_window.unwrap_or(0);
                        if window > 0 {
                            conditions.push(format!("day={}+/-{}", day, window));
                        } else {
                            conditions.push(format!("day={}", day));
                        }
                    }
                    println!("{} -> {}", conditions.join(", "), rule.tags.join(", "));
                }
            }
        }
        TagAction::Test { description } => {
            // Test with zero amount and today's date since we don't have a full transaction
            let today = chrono::Local::now().date_naive();
            let tags = rules.get_tags(&description, Decimal::ZERO, today);
            if tags.is_empty() {
                println!("No matching rules for: {}", description);
                println!("(Note: tested with amount=0, date={})", today);
            } else {
                println!("Matched tags: {}", tags.join(", "));
                println!("(Note: tested with amount=0, date={})", today);
            }
        }
        TagAction::Reapply { reset } => {
            let store = CsvStore::new(get_store_path());
            let mut transactions = store.load_all()?;
            // The PayPal recovery sidecar lets rules tag bare `PAYPAL PAYMENT`
            // rows by their recovered merchant (e.g. a "Streamflix" rule fires
            // on the recovered name even though the bank text is just
            // "PAYPAL PAYMENT"). Empty if `paypal recover` has not been run.
            let recoveries = query::load_recovery_index(&default_paypal_matches_jsonl())?;
            if reset {
                // DESTRUCTIVE: clear every tag, then rebuild purely from rules
                // (raw_description), then layer recovered-merchant matches.
                reapply_rules(&mut transactions, &rules);
            }
            // Additive in both modes: append rule matches against the
            // raw_description AND the recovered PayPal merchant. (After --reset
            // the base tags are already raw_description-only; this adds the
            // recovered-merchant matches on top.)
            apply_rules_with_recovery(&mut transactions, &rules, &recoveries);

            // This mutates transactions.csv — snapshot first (primer rule 3).
            snapshot_transactions(&get_store_path())?;
            store.rewrite(&transactions)?;
            let tagged = transactions.iter().filter(|t| !t.tags.is_empty()).count();
            let via_recovery = if recoveries.is_empty() {
                String::new()
            } else {
                format!(" [{} PayPal recoveries consulted]", recoveries.len())
            };
            eprintln!(
                "Re-tagged {} transactions ({} with tags){}{}",
                transactions.len(),
                tagged,
                if reset {
                    " [--reset: prior tags cleared]"
                } else {
                    " [additive: manual tags preserved]"
                },
                via_recovery
            );
        }
    }

    Ok(())
}

fn cmd_untagged(limit: Option<usize>, include_credits: bool) -> anyhow::Result<()> {
    let store = CsvStore::new(get_store_path());
    let transactions = store.load_all()?;

    let untagged_all: Vec<_> = transactions.iter().filter(|t| t.tags.is_empty()).collect();

    // By default the worklist is debits only — credits are income/refunds,
    // separable by sign, and shouldn't be categorised into spend buckets.
    let credits_hidden = if include_credits {
        0
    } else {
        untagged_all.iter().filter(|t| t.is_credit()).count()
    };
    let untagged: Vec<_> = untagged_all
        .into_iter()
        .filter(|t| include_credits || t.is_debit())
        .collect();

    if credits_hidden > 0 {
        eprintln!(
            "{} untagged transactions ({} credit rows hidden; use --include-credits)",
            untagged.len(),
            credits_hidden
        );
    } else {
        eprintln!("{} untagged transactions", untagged.len());
    }

    let display = match limit {
        Some(n) => &untagged[..n.min(untagged.len())],
        None => &untagged[..],
    };

    for tx in display {
        println!(
            "{}\t{}\t{}\t{}",
            tx.date, tx.account, tx.amount, tx.raw_description
        );
    }

    Ok(())
}

fn cmd_stats(
    year: Option<i32>,
    month: Option<&str>,
    since: Option<NaiveDate>,
) -> anyhow::Result<()> {
    let store = CsvStore::new(get_store_path());
    let transactions = store.load_all()?;

    if transactions.is_empty() {
        eprintln!("No transactions in store");
        return Ok(());
    }

    let filter = query::DateFilter::from_flags(year, month, since)?;
    let rows: Vec<_> = transactions
        .iter()
        .filter(|t| filter.matches(t.date))
        .collect();
    if rows.is_empty() {
        eprintln!("No transactions in the selected period");
        return Ok(());
    }

    let total = rows.len();
    let tagged = rows.iter().filter(|t| !t.tags.is_empty()).count();
    let untagged = total - tagged;

    let current = rows
        .iter()
        .filter(|t| t.account == Account::Current)
        .count();
    let visa = rows.iter().filter(|t| t.account == Account::Visa).count();

    let dates: Vec<_> = rows.iter().map(|t| t.date).collect();
    let min_date = dates.iter().min().unwrap();
    let max_date = dates.iter().max().unwrap();

    // Spend floor: debits that are NOT transfers/income/tax/one-off. Income is
    // every credit (separable by sign). Excluded = non-spend-tagged debits.
    // Spend + Excluded == all debits; Income == all credits.
    let spend: Decimal = rows
        .iter()
        .filter(|t| t.counts_as_spend())
        .map(|t| t.amount.abs())
        .sum();
    let income: Decimal = rows
        .iter()
        .filter(|t| t.is_credit())
        .map(|t| t.amount)
        .sum();
    // Business / professional costs (a subset of nonspend) shown on their own
    // line; the generic Excluded line then carries only transfer/income/tax/one-off.
    let business: Decimal = rows
        .iter()
        .filter(|t| t.is_debit() && t.is_business())
        .map(|t| t.amount.abs())
        .sum();
    let excluded: Decimal = rows
        .iter()
        .filter(|t| t.is_debit() && t.is_nonspend() && !t.is_business())
        .map(|t| t.amount.abs())
        .sum();
    let untagged_debits = rows
        .iter()
        .filter(|t| t.is_debit() && t.tags.is_empty())
        .count();

    println!("Transactions: {}", total);
    println!("  Current: {}", current);
    println!("  Visa: {}", visa);
    println!(
        "Tagged: {} ({:.1}%)",
        tagged,
        (tagged as f64 / total as f64) * 100.0
    );
    println!("Untagged: {}", untagged);
    println!("Date range: {} to {}", min_date, max_date);
    println!();
    println!(
        "{:<54} £{:.2}",
        "Spend (recurring personal living cost):", spend
    );
    println!("{:<54} £{:.2}", "Income (all credits):", income);
    println!(
        "{:<54} £{:.2}",
        "Business (professional — excluded from floor):", business
    );
    println!(
        "{:<54} £{:.2}",
        "Excluded (transfer/income/tax/one-off):", excluded
    );
    if untagged_debits > 0 {
        println!(
            "  note: {} untagged debit(s) still counted as spend — tag any one-off lumps (tax, gym, etc.) so the Spend floor settles",
            untagged_debits
        );
    }

    Ok(())
}

fn cmd_categorize(limit: Option<usize>, include_credits: bool) -> anyhow::Result<()> {
    let store = CsvStore::new(get_store_path());
    let rules_path = get_rules_path();
    let mut rules = TagRules::load(&rules_path)?;
    let mut transactions = store.load_all()?;

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    // Get indices of untagged transactions. By default only debits (the spend
    // worklist); credits are income/refunds, excluded unless --include-credits.
    let untagged_indices: Vec<usize> = transactions
        .iter()
        .enumerate()
        .filter(|(_, t)| t.tags.is_empty() && (include_credits || t.is_debit()))
        .map(|(i, _)| i)
        .collect();

    let to_process = match limit {
        Some(n) => n.min(untagged_indices.len()),
        None => untagged_indices.len(),
    };

    eprintln!(
        "{} untagged transactions ({} to process)",
        untagged_indices.len(),
        to_process
    );
    eprintln!("Commands: [tags...] to tag, [s]kip, [q]uit, [r]ule to create rule\n");

    let mut processed = 0;
    let mut tagged_count = 0;

    for &idx in untagged_indices.iter().take(to_process) {
        // Copy data we need before any mutation
        let tx_date = transactions[idx].date;
        let tx_account = transactions[idx].account;
        let tx_amount = transactions[idx].amount;
        let tx_raw_desc = transactions[idx].raw_description.clone();

        processed += 1;

        println!(
            "\n[{}/{}] {} | {} | {} | {}",
            processed, to_process, tx_date, tx_account, tx_amount, tx_raw_desc
        );

        print!("Tags (space-separated), s=skip, q=quit: ");
        stdout.flush()?;

        let mut input = String::new();
        stdin.lock().read_line(&mut input)?;
        let input = input.trim();

        if input.eq_ignore_ascii_case("q") {
            eprintln!("Quitting...");
            break;
        }

        if input.eq_ignore_ascii_case("s") || input.is_empty() {
            continue;
        }

        // Parse tags
        let tags: Vec<String> = input.split_whitespace().map(String::from).collect();

        // Apply tags to this transaction
        transactions[idx].tags = tags.clone();
        tagged_count += 1;

        // Ask about creating a rule
        let is_redacted = is_description_redacted(&tx_raw_desc);
        if is_redacted {
            print!(
                "Description is redacted. Create rule with amount={}? [y/N]: ",
                tx_amount
            );
        } else {
            print!("Create rule for similar transactions? [y/N/pattern]: ");
        }
        stdout.flush()?;

        let mut rule_input = String::new();
        stdin.lock().read_line(&mut rule_input)?;
        let rule_input = rule_input.trim();

        if rule_input.eq_ignore_ascii_case("n") || rule_input.is_empty() {
            continue;
        }

        // Determine pattern and amount condition
        let (final_pattern, rule_amount, rule_day, rule_day_window): (
            String,
            Option<Decimal>,
            Option<u32>,
            Option<u32>,
        ) = if is_redacted && rule_input.eq_ignore_ascii_case("y") {
            // Ask about day-of-month condition
            let tx_day = tx_date.day();
            print!(
                "Add day-of-month condition? (tx day={}). Enter day[+/-window] or [N]: ",
                tx_day
            );
            stdout.flush()?;

            let mut day_input = String::new();
            stdin.lock().read_line(&mut day_input)?;
            let day_input = day_input.trim();

            let (day, window) = if day_input.eq_ignore_ascii_case("n") || day_input.is_empty() {
                (None, None)
            } else {
                parse_day_input(day_input, tx_day)
            };

            (tx_raw_desc.clone(), Some(tx_amount), day, window)
        } else {
            let pattern = if rule_input.eq_ignore_ascii_case("y") {
                suggest_pattern(&tx_raw_desc)
            } else {
                rule_input.to_string()
            };

            // Show pattern and ask for confirmation
            print!("Pattern '{}' - confirm? [Y/n/edit]: ", pattern);
            stdout.flush()?;

            let mut confirm = String::new();
            stdin.lock().read_line(&mut confirm)?;
            let confirm = confirm.trim();

            if confirm.eq_ignore_ascii_case("n") {
                continue;
            }
            let p = if confirm.is_empty() || confirm.eq_ignore_ascii_case("y") {
                pattern
            } else {
                confirm.to_string()
            };
            (p, None, None, None)
        };

        // Count how many transactions would match
        let pattern_lower = final_pattern.to_lowercase();
        let matching: Vec<usize> = transactions
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                if !t.raw_description.to_lowercase().contains(&pattern_lower) {
                    return false;
                }
                if let Some(amt) = rule_amount {
                    if t.amount != amt {
                        return false;
                    }
                }
                // Note: for the preview count we don't filter by day — the rule will do that at apply time
                true
            })
            .map(|(i, _)| i)
            .collect();

        let mut note = String::new();
        if let Some(a) = rule_amount {
            note.push_str(&format!(" [amount={}]", a));
        }
        if let Some(day) = rule_day {
            let w = rule_day_window.unwrap_or(0);
            if w > 0 {
                note.push_str(&format!(" [day={}+/-{}]", day, w));
            } else {
                note.push_str(&format!(" [day={}]", day));
            }
        }
        eprintln!(
            "Rule '{}'{} matches {} transactions",
            final_pattern,
            note,
            matching.len()
        );

        // Add rule
        rules.add_rule(
            &final_pattern,
            tags.clone(),
            rule_amount,
            None,
            None,
            rule_day,
            rule_day_window,
        );
        rules.save(&rules_path)?;

        // Apply to all matching transactions
        for &match_idx in &matching {
            for tag in &tags {
                if !transactions[match_idx].tags.contains(tag) {
                    transactions[match_idx].tags.push(tag.clone());
                }
            }
        }

        tagged_count += matching.len().saturating_sub(1); // -1 because we already counted the current one
        eprintln!("Tagged {} transactions with rule", matching.len());
    }

    // Save updated transactions
    store.rewrite(&transactions)?;

    let final_tagged = transactions.iter().filter(|t| !t.tags.is_empty()).count();
    eprintln!("\nSession: tagged {} transactions", tagged_count);
    eprintln!(
        "Total: {}/{} transactions tagged ({:.1}%)",
        final_tagged,
        transactions.len(),
        (final_tagged as f64 / transactions.len() as f64) * 100.0
    );

    Ok(())
}

/// Check if a description is redacted/generic (e.g. "****", "**********")
fn is_description_redacted(description: &str) -> bool {
    let trimmed = description.trim();
    trimmed.chars().all(|c| c == '*') && !trimmed.is_empty()
}

/// Parse day-of-month input like "28", "28+/-3", or "y" (use tx_day)
fn parse_day_input(input: &str, tx_day: u32) -> (Option<u32>, Option<u32>) {
    if input.eq_ignore_ascii_case("y") {
        return (Some(tx_day), None);
    }
    if let Some((day_str, window_str)) = input.split_once("+/-") {
        let day = day_str.trim().parse::<u32>().unwrap_or(tx_day);
        let window = window_str.trim().parse::<u32>().unwrap_or(0);
        (Some(day), if window > 0 { Some(window) } else { None })
    } else if let Ok(day) = input.trim().parse::<u32>() {
        (Some(day), None)
    } else {
        eprintln!(
            "Could not parse day input '{}', skipping day condition",
            input
        );
        (None, None)
    }
}

/// Suggest a pattern from a transaction description
/// Tries to extract the meaningful merchant name part
fn suggest_pattern(description: &str) -> String {
    // Remove common noise patterns
    let cleaned = description
        .replace("INT'L **********", "")
        .replace("**********", "")
        .replace("****", "");

    // Split on whitespace and take significant words
    let words: Vec<&str> = cleaned
        .split_whitespace()
        .filter(|w| w.len() > 2)
        .filter(|w| {
            !w.chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == '*')
        })
        .take(2)
        .collect();

    if words.is_empty() {
        // Fallback: just use first 15 chars
        description.chars().take(15).collect()
    } else {
        words.join(" ")
    }
}
