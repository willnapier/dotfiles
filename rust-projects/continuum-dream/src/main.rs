mod apply;
mod consolidate;
mod diff;
mod gather;
mod gates;
mod orient;
mod prune;
mod state;
mod types;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::io::{self, Write};

#[derive(Parser)]
#[command(
    name = "continuum-dream",
    about = "Automated memory consolidation across AI assistant conversation logs"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Bypass all gates (time, session count)
    #[arg(long)]
    force: bool,

    /// Preview changes without writing to disk
    #[arg(long)]
    dry_run: bool,

    /// AI backend command (default: "claude -p")
    #[arg(long, default_value = "claude -p")]
    model: String,

    /// Only consider sessions within this window (e.g. "7d", "24h")
    #[arg(long)]
    since: Option<String>,

    /// Dump the context document to stdout instead of sending to AI
    #[arg(long, hide = true)]
    dump_context: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Show gate status, last dream time, memory health
    Status,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Some(Command::Status) => run_status(),
        None => run_dream(&cli),
    }
}

fn run_status() -> Result<()> {
    let dream_state = state::load()?;
    let memory_state = orient::scan_memory()?;
    let new_count = gather::count_new_sessions(&dream_state)?;

    println!("continuum-dream status");
    println!("======================\n");

    // Last dream
    match &dream_state.last_dream_time {
        Some(t) => println!("Last dream:      {}", t),
        None => println!("Last dream:      never"),
    }
    println!("Total dreams:    {}", dream_state.total_dreams);
    println!("Sessions tracked: {}", dream_state.sessions_processed.len());

    // Gates
    println!("\nGates:");
    let time_gate = gates::check_time_gate(&dream_state);
    println!(
        "  Time (24h):    {}",
        if time_gate.passed {
            "PASS".to_string()
        } else {
            format!("BLOCKED - {}", time_gate.reason.unwrap_or_default())
        }
    );
    let session_gate = gates::check_session_gate(new_count);
    println!(
        "  Sessions (5+): {}",
        if session_gate.passed {
            format!("PASS ({} new)", new_count)
        } else {
            format!("BLOCKED - {}", session_gate.reason.unwrap_or_default())
        }
    );

    // Memory health
    println!("\nMemory health:");
    println!("  MEMORY.md:     {} lines (limit: 200)", memory_state.index_line_count);
    if memory_state.index_line_count > 200 {
        println!("                 \x1b[33mOVER LIMIT\x1b[0m");
    }
    println!("  Memory files:  {}", memory_state.files.len());

    if !memory_state.orphaned_index_refs.is_empty() {
        println!("  \x1b[33mOrphaned refs:\x1b[0m");
        for r in &memory_state.orphaned_index_refs {
            println!("    - {}", r);
        }
    }

    if !memory_state.unindexed_files.is_empty() {
        println!("  \x1b[33mUnindexed files:\x1b[0m");
        for f in &memory_state.unindexed_files {
            println!("    - {}", f);
        }
    }

    // Last dream summary
    if let Some(summary) = &dream_state.last_dream_summary {
        println!("\nLast dream summary:");
        println!("  {}", summary);
    }

    // New sessions breakdown
    println!("\nNew sessions:    {}", new_count);

    Ok(())
}

fn run_dream(cli: &Cli) -> Result<()> {
    let mut dream_state = state::load()?;

    // Check gates (unless --force)
    if !cli.force {
        let new_count = gather::count_new_sessions(&dream_state)?;

        let time_gate = gates::check_time_gate(&dream_state);
        if !time_gate.passed {
            println!("{}", time_gate.reason.unwrap());
            return Ok(());
        }

        let session_gate = gates::check_session_gate(new_count);
        if !session_gate.passed {
            println!("{}", session_gate.reason.unwrap());
            return Ok(());
        }
    }

    // Acquire lock
    let _lock = gates::acquire_lock()?;

    // Orient: scan memory
    let memory_state = orient::scan_memory()?;
    eprintln!(
        "Memory: {} files, MEMORY.md {} lines",
        memory_state.files.len(),
        memory_state.index_line_count
    );

    // Gather: collect new sessions
    let sessions = gather::collect_sessions(
        &dream_state,
        cli.since.as_deref(),
    )?;

    if sessions.is_empty() {
        println!("No new sessions to consolidate.");
        return Ok(());
    }

    let session_paths: Vec<String> = sessions.iter().map(|s| s.relative_path.clone()).collect();
    eprintln!("Found {} new sessions", sessions.len());

    let session_context = gather::format_sessions(&sessions);

    // --dump-context: print and exit
    if cli.dump_context {
        let memory_context = orient::format_memory_state(&memory_state);
        println!(
            "# Current Memory State\n\n{}\n---\n\n# New Sessions Since Last Dream\n\n{}",
            memory_context, session_context
        );
        return Ok(());
    }

    // Consolidate: call AI
    eprintln!("Consolidating via '{}'...", cli.model);
    let mut response = consolidate::run(&cli.model, &memory_state, &session_context)?;

    // Prune: validate
    let warnings = prune::validate(&mut response, &memory_state);
    for w in &warnings {
        eprintln!("\x1b[33m{}\x1b[0m", w);
    }

    // Check if any changes remain
    let has_changes = !response.files_to_update.is_empty()
        || !response.files_to_create.is_empty()
        || !response.files_to_delete.is_empty()
        || response.memory_index != "UNCHANGED";

    if !has_changes {
        println!("No changes proposed (or all rejected by validation).");
        // Still record the sessions as processed
        state::record_dream(&mut dream_state, &session_paths, "No changes needed.")?;
        return Ok(());
    }

    // Diff: display
    let changes = diff::build_changes(&response, &memory_state);
    diff::display(&changes, &response, &memory_state);

    // Apply decision
    if cli.dry_run {
        println!("\n\x1b[90m(dry run — no changes written)\x1b[0m");
        return Ok(());
    }

    // Check if we're in a TTY
    let is_tty = atty_check();

    if is_tty {
        print!("\nApply these changes? [y/N] ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    } else {
        eprintln!("Non-interactive mode — applying automatically");
    }

    // Apply
    apply::write_changes(&changes, &response, &memory_state)?;

    // Record dream
    state::record_dream(&mut dream_state, &session_paths, &response.summary)?;
    eprintln!("Dream complete. State saved.");

    Ok(())
}

/// Simple TTY check without the atty crate
fn atty_check() -> bool {
    // Use isatty via libc-free approach: try to get terminal size
    std::process::Command::new("test")
        .args(["-t", "0"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
