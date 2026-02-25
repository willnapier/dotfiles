mod cc_logs;
mod continuum;
mod dump;
mod output;
mod types;

use anyhow::Result;
use chrono::{Local, NaiveDate};
use clap::{Parser, Subcommand};

use types::DayActivity;

#[derive(Parser)]
#[command(name = "continuum-activity")]
#[command(about = "Extract daily AI activity from Claude Code logs and Continuum archive")]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    report: ReportArgs,
}

#[derive(clap::Args)]
struct ReportArgs {
    /// Target date (YYYY-MM-DD). Defaults to today.
    date: Option<NaiveDate>,

    /// Output as JSON instead of markdown
    #[arg(long)]
    json: bool,

    /// Show full user messages, not truncated
    #[arg(long)]
    verbose: bool,

    /// Only show Claude Code sessions (skip Continuum archive)
    #[arg(long)]
    cc_only: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Dump a session's full conversation text (for LLM context injection)
    Dump(DumpArgs),
}

#[derive(clap::Args)]
struct DumpArgs {
    /// Session ID (or prefix) to dump
    session_id: Option<String>,

    /// Dump the most recent session
    #[arg(long)]
    last: bool,

    /// Filter by assistant name (e.g. gemini-cli, claude-code)
    #[arg(long)]
    assistant: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Dump(args)) => dump::dump_session(
            args.session_id.as_deref(),
            args.last,
            args.assistant.as_deref(),
        ),
        None => run_report(cli.report),
    }
}

fn run_report(args: ReportArgs) -> Result<()> {
    let target_date = args.date.unwrap_or_else(|| Local::now().date_naive());
    let date_str = target_date.format("%Y-%m-%d").to_string();

    let cc_sessions = cc_logs::extract_cc_sessions(target_date, args.verbose)?;

    let continuum_sessions = if args.cc_only {
        Vec::new()
    } else {
        continuum::extract_continuum_sessions(target_date)?
    };

    if cc_sessions.is_empty() && continuum_sessions.is_empty() {
        eprintln!("No activity found for {}", date_str);
        return Ok(());
    }

    let activity = DayActivity {
        date: date_str,
        cc_sessions,
        continuum_sessions,
    };

    if args.json {
        println!("{}", output::render_json(&activity));
    } else {
        print!("{}", output::render_markdown(&activity));
    }

    Ok(())
}
