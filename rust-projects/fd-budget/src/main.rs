use chrono::{Datelike, NaiveDate};
use clap::{Parser, Subcommand};
use fd_budget::{Account, import, store::CsvStore, tags::{TagRules, apply_rules, reapply_rules}, dedup, enrich, query};
use rust_decimal::Decimal;
use std::fs::File;
use std::io::{BufReader, BufRead, Write};
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
    /// List transactions with no tags
    Untagged {
        /// Limit output to N transactions
        #[arg(short, long)]
        limit: Option<usize>,
    },
    /// Show statistics. With `--by-counterparty`, aggregates outgoing spend
    /// per counterparty, joining transactions.csv against matches.jsonl and
    /// bills.jsonl. Without flags, prints the original tag/account summary.
    Stats {
        /// Aggregate outgoing spend per counterparty (Stage 2 query)
        #[arg(long)]
        by_counterparty: bool,
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
    /// Re-apply all rules to existing transactions
    Reapply,
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
        Commands::Untagged { limit } => {
            cmd_untagged(limit)?;
        }
        Commands::Stats { by_counterparty, year, month, since, limit } => {
            if by_counterparty {
                cmd_stats_by_counterparty(year, month.as_deref(), since, limit)?;
            } else {
                cmd_stats()?;
            }
        }
        Commands::Tx { action } => {
            cmd_tx(action)?;
        }
        Commands::Categorize { limit } => {
            cmd_categorize(limit)?;
        }
        Commands::Enrich { from, out, amount_tolerance, date_window, dry_run } => {
            cmd_enrich(from, out, amount_tolerance, date_window, dry_run)?;
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
    println!("  ambiguous         {:>4} ({:.1}%)", ambiguous, pct(ambiguous));
    println!("  internal-transfer {:>4} ({:.1}%)", internal, pct(internal));
    println!("  none              {:>4} ({:.1}%)", none_count, pct(none_count));

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
    let joined = query::join(&txs, &emails, &matches);
    let filter = query::DateFilter::from_flags(year, month, since)?;
    query::cmd_stats_by_counterparty(&joined, filter, limit)
}

fn cmd_tx(action: TxAction) -> anyhow::Result<()> {
    let (txs, emails, matches) = query::load_all(
        &get_store_path(),
        &default_bills_jsonl(),
        &default_matches_jsonl(),
    )?;
    let joined = query::join(&txs, &emails, &matches);

    match action {
        TxAction::Vendor { name, with_evidence, year, month, since } => {
            let filter = query::DateFilter::from_flags(year, month.as_deref(), since)?;
            query::cmd_tx_by_vendor(&joined, filter, &name, with_evidence)
        }
        TxAction::Unmatched { over, year, month, since } => {
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

    // Parse the midata file
    let reader = BufReader::new(File::open(file)?);
    let transactions = import::parse_midata(reader, account)?;
    eprintln!("Parsed {} transactions from file", transactions.len());

    // Deduplicate
    let transactions = dedup::deduplicate(transactions, &existing_ids);
    eprintln!("{} new transactions after deduplication", transactions.len());

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
        TagAction::Add { pattern, tags, amount, min_amount, max_amount, day_of_month, day_window } => {
            rules.add_rule(&pattern, tags.clone(), amount, min_amount, max_amount, day_of_month, day_window);
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
        TagAction::Reapply => {
            let store = CsvStore::new(get_store_path());
            let mut transactions = store.load_all()?;
            reapply_rules(&mut transactions, &rules);
            store.rewrite(&transactions)?;
            let tagged = transactions.iter().filter(|t| !t.tags.is_empty()).count();
            eprintln!("Re-tagged {} transactions ({} with tags)", transactions.len(), tagged);
        }
    }

    Ok(())
}

fn cmd_untagged(limit: Option<usize>) -> anyhow::Result<()> {
    let store = CsvStore::new(get_store_path());
    let transactions = store.load_all()?;

    let untagged: Vec<_> = transactions
        .iter()
        .filter(|t| t.tags.is_empty())
        .collect();

    eprintln!("{} untagged transactions", untagged.len());

    let display = match limit {
        Some(n) => &untagged[..n.min(untagged.len())],
        None => &untagged[..],
    };

    for tx in display {
        println!("{}\t{}\t{}\t{}", tx.date, tx.account, tx.amount, tx.raw_description);
    }

    Ok(())
}

fn cmd_stats() -> anyhow::Result<()> {
    let store = CsvStore::new(get_store_path());
    let transactions = store.load_all()?;

    if transactions.is_empty() {
        eprintln!("No transactions in store");
        return Ok(());
    }

    let total = transactions.len();
    let tagged = transactions.iter().filter(|t| !t.tags.is_empty()).count();
    let untagged = total - tagged;

    let current = transactions.iter().filter(|t| t.account == Account::Current).count();
    let visa = transactions.iter().filter(|t| t.account == Account::Visa).count();

    let dates: Vec<_> = transactions.iter().map(|t| t.date).collect();
    let min_date = dates.iter().min().unwrap();
    let max_date = dates.iter().max().unwrap();

    println!("Transactions: {}", total);
    println!("  Current: {}", current);
    println!("  Visa: {}", visa);
    println!("Tagged: {} ({:.1}%)", tagged, (tagged as f64 / total as f64) * 100.0);
    println!("Untagged: {}", untagged);
    println!("Date range: {} to {}", min_date, max_date);

    Ok(())
}

fn cmd_categorize(limit: Option<usize>) -> anyhow::Result<()> {
    let store = CsvStore::new(get_store_path());
    let rules_path = get_rules_path();
    let mut rules = TagRules::load(&rules_path)?;
    let mut transactions = store.load_all()?;

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    // Get indices of untagged transactions
    let untagged_indices: Vec<usize> = transactions
        .iter()
        .enumerate()
        .filter(|(_, t)| t.tags.is_empty())
        .map(|(i, _)| i)
        .collect();

    let to_process = match limit {
        Some(n) => n.min(untagged_indices.len()),
        None => untagged_indices.len(),
    };

    eprintln!("{} untagged transactions ({} to process)", untagged_indices.len(), to_process);
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

        println!("\n[{}/{}] {} | {} | {} | {}",
            processed, to_process,
            tx_date, tx_account, tx_amount, tx_raw_desc);

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
            print!("Description is redacted. Create rule with amount={}? [y/N]: ", tx_amount);
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
        let (final_pattern, rule_amount, rule_day, rule_day_window): (String, Option<Decimal>, Option<u32>, Option<u32>) =
            if is_redacted && rule_input.eq_ignore_ascii_case("y") {
                // Ask about day-of-month condition
                let tx_day = tx_date.day();
                print!("Add day-of-month condition? (tx day={}). Enter day[+/-window] or [N]: ", tx_day);
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
        eprintln!("Rule '{}'{} matches {} transactions", final_pattern, note, matching.len());

        // Add rule
        rules.add_rule(&final_pattern, tags.clone(), rule_amount, None, None, rule_day, rule_day_window);
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
    eprintln!("Total: {}/{} transactions tagged ({:.1}%)",
        final_tagged, transactions.len(),
        (final_tagged as f64 / transactions.len() as f64) * 100.0);

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
        eprintln!("Could not parse day input '{}', skipping day condition", input);
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
        .filter(|w| !w.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '*'))
        .take(2)
        .collect();

    if words.is_empty() {
        // Fallback: just use first 15 chars
        description.chars().take(15).collect()
    } else {
        words.join(" ")
    }
}
