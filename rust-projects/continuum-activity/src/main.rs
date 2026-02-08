mod cc_logs;
mod continuum;
mod output;
mod types;

use anyhow::Result;
use chrono::{Local, NaiveDate};
use clap::Parser;

use types::DayActivity;

#[derive(Parser)]
#[command(name = "continuum-activity")]
#[command(about = "Extract daily AI activity from Claude Code logs and Continuum archive")]
struct Cli {
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

fn main() -> Result<()> {
    let cli = Cli::parse();

    let target_date = cli.date.unwrap_or_else(|| Local::now().date_naive());
    let date_str = target_date.format("%Y-%m-%d").to_string();

    let cc_sessions = cc_logs::extract_cc_sessions(target_date, cli.verbose)?;

    let continuum_sessions = if cli.cc_only {
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

    if cli.json {
        println!("{}", output::render_json(&activity));
    } else {
        print!("{}", output::render_markdown(&activity));
    }

    Ok(())
}
