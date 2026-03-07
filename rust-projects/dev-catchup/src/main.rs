mod activity;
mod daypage;
mod drafting;
mod matching;
mod output;
mod types;

use anyhow::Result;
use chrono::{Days, Local, NaiveDate};
use clap::Parser;

use types::*;

#[derive(Parser)]
#[command(name = "dev-catchup", about = "Catch up on missing dev:: DayPage entries")]
struct Cli {
    /// Number of days to look back (default: 7)
    #[arg(long, default_value_t = 7)]
    days: u32,

    /// Check a specific date only (YYYY-MM-DD)
    #[arg(long)]
    date: Option<NaiveDate>,

    /// Actually queue entries via daypage-append (default: dry-run)
    #[arg(long)]
    apply: bool,

    /// Skip AI drafting, just report unmatched sessions
    #[arg(long)]
    no_ai: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Build date range
    let dates: Vec<NaiveDate> = if let Some(date) = cli.date {
        vec![date]
    } else {
        let today = Local::now().date_naive();
        (0..cli.days)
            .filter_map(|i| today.checked_sub_days(Days::new(i as u64)))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    };

    // Collect all reports and unmatched sessions for drafting
    let mut reports: Vec<DayReport> = Vec::new();
    let mut unmatched_for_drafting: Vec<(NaiveDate, String)> = Vec::new();

    for date in &dates {
        let activity = match activity::fetch_activity(*date) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("Warning: skipping {}: {}", date, e);
                continue;
            }
        };

        let dev_entries = daypage::extract_dev_entries(*date)?;

        // Unify all sessions
        let mut sessions: Vec<(UnifiedSession, MatchResult)> = Vec::new();

        for cc in &activity.cc_sessions {
            let unified = matching::unify_cc_session(cc);
            if matching::is_trivial(&unified) {
                sessions.push((unified, MatchResult::Trivial));
            } else if matching::is_clinical(cc) {
                sessions.push((unified, MatchResult::Clinical));
            } else {
                let result = matching::match_session(&unified, &dev_entries);
                if matches!(result, MatchResult::Unmatched) {
                    unmatched_for_drafting.push((*date, unified.detail.clone()));
                }
                sessions.push((unified, result));
            }
        }

        for cont in &activity.continuum_sessions {
            let unified = matching::unify_continuum_session(cont);
            if matching::is_trivial(&unified) {
                sessions.push((unified, MatchResult::Trivial));
            } else {
                let result = matching::match_session(&unified, &dev_entries);
                if matches!(result, MatchResult::Unmatched) {
                    unmatched_for_drafting.push((*date, unified.detail.clone()));
                }
                sessions.push((unified, result));
            }
        }

        reports.push(DayReport {
            date: *date,
            sessions,
            drafts: vec![],
        });
    }

    // Draft entries for unmatched sessions
    if !unmatched_for_drafting.is_empty() && !cli.no_ai {
        let sessions_for_claude: Vec<(NaiveDate, &str)> = unmatched_for_drafting
            .iter()
            .map(|(d, s)| (*d, s.as_str()))
            .collect();

        match drafting::draft_entries(&sessions_for_claude) {
            Ok(drafts) => {
                // Associate drafts with their reports
                for draft in &drafts {
                    if let Some(report) = reports.iter_mut().find(|r| r.date == draft.date) {
                        report.drafts.push(draft.clone());
                    }
                }
            }
            Err(e) => {
                eprintln!("Warning: AI drafting failed: {}", e);
                eprintln!("Run with --no-ai to skip drafting.");
            }
        }
    }

    // Render report
    output::render_report(&reports, cli.apply);

    // Apply if requested
    if cli.apply {
        let mut applied = 0;
        for report in &reports {
            for draft in &report.drafts {
                match drafting::apply_entry(draft.date, &draft.entry) {
                    Ok(()) => {
                        eprintln!("Queued: {} -> {}", draft.date, draft.entry);
                        applied += 1;
                    }
                    Err(e) => {
                        eprintln!("Failed to queue entry for {}: {}", draft.date, e);
                    }
                }
            }
        }
        if applied > 0 {
            eprintln!(
                "\n{} entries queued — flush with Space+U in Helix",
                applied
            );
        }
    }

    Ok(())
}
