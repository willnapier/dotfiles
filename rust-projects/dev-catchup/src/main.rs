mod activity;
mod daypage;
mod devlog;
mod drafting;
mod git;
mod matching;
mod output;
mod project;
mod types;

use anyhow::Result;
use chrono::{Datelike, Days, Local, NaiveDate, Weekday};
use clap::Parser;

use std::collections::BTreeSet;

use types::*;

#[derive(Parser)]
#[command(
    name = "dev-catchup",
    about = "Catch up on missing dev:: DayPage entries"
)]
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

    /// Generate a queryable weekly DevLog instead of the DayPage-catchup report
    #[arg(long)]
    devlog: bool,

    /// In --devlog mode, actually write the week files (default: dry-run to stdout)
    #[arg(long)]
    write: bool,
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

    // DevLog mode is fully additive: handle it and return before the existing
    // DayPage-catchup code path runs.
    if cli.devlog {
        return run_devlog(&dates, cli.no_ai, cli.write);
    }

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
            eprintln!("\n{} entries queued — flush with Space+U in Helix", applied);
        }
    }

    Ok(())
}

/// Generate the weekly DevLog for the ISO weeks covered by `dates`.
fn run_devlog(dates: &[NaiveDate], no_ai: bool, write: bool) -> Result<()> {
    use devlog::DevLogEntry;

    // Distinct set of ISO weeks (iso_year, iso_week) covered by the dates.
    let mut weeks: BTreeSet<(i32, u32)> = BTreeSet::new();
    for d in dates {
        let iso = d.iso_week();
        weeks.insert((iso.year(), iso.week()));
    }

    for (iso_year, iso_week) in weeks {
        let week_label = format!("{}-W{:02}", iso_year, iso_week);

        // Enumerate the 7 dates (Mon..Sun) of this ISO week.
        let monday = match NaiveDate::from_isoywd_opt(iso_year, iso_week, Weekday::Mon) {
            Some(m) => m,
            None => continue,
        };
        let week_dates: Vec<NaiveDate> = (0..7)
            .filter_map(|i| monday.checked_add_days(Days::new(i)))
            .collect();

        let mut entries: Vec<DevLogEntry> = Vec::new();

        for date in &week_dates {
            let activity = match activity::fetch_activity(*date) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("Warning: skipping {}: {}", date, e);
                    continue;
                }
            };

            // CC sessions only for v1 — ignore continuum_sessions entirely.
            for cc in &activity.cc_sessions {
                if matching::is_clinical(cc) {
                    continue;
                }
                let unified = matching::unify_cc_session(cc);
                if matching::is_trivial(&unified) {
                    continue;
                }

                let (primary, all_projects, repo_root) =
                    project::primary_project(&cc.files_modified);

                // Commits/PRs only when we have a repo root and both timestamps.
                let (commits, prs) = match (&repo_root, cc.start_time, cc.end_time) {
                    (Some(root), Some(start), Some(end)) => {
                        let found = git::commits_in_window(root, start, end);
                        let mut shas: Vec<String> = Vec::new();
                        let mut pr_set: BTreeSet<u32> = BTreeSet::new();
                        for c in &found {
                            if !shas.contains(&c.short_sha) {
                                shas.push(c.short_sha.clone());
                            }
                            if let Some(pr) = c.pr {
                                pr_set.insert(pr);
                            }
                        }
                        (shas, pr_set.into_iter().collect::<Vec<_>>())
                    }
                    _ => (Vec::new(), Vec::new()),
                };

                let minutes = match (cc.start_time, cc.end_time) {
                    (Some(start), Some(end)) => (end - start).num_minutes().max(0),
                    _ => 0,
                };

                let (prose, topics) = devlog::resolve_prose(&cc.session_id, &unified.detail, no_ai);

                entries.push(DevLogEntry {
                    date: *date,
                    start: cc.start_time,
                    primary_project: primary,
                    all_projects,
                    prs,
                    commits,
                    topics,
                    prose,
                    minutes,
                    msg_count: unified.message_count,
                });
            }
        }

        let rendered = devlog::render_week(&week_label, &entries);

        if write {
            let path = devlog::week_path(monday);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| anyhow::anyhow!("Failed to create DevLog dir: {}", e))?;
            }
            std::fs::write(&path, &rendered)
                .map_err(|e| anyhow::anyhow!("Failed to write {}: {}", path.display(), e))?;
            eprintln!("Wrote {} ({} entries)", path.display(), entries.len());
        } else {
            let path = devlog::week_path(monday);
            println!("--- {} ---", path.display());
            print!("{}", rendered);
        }
    }

    Ok(())
}
