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

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

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

/// A (date, project) aggregation bucket.
#[derive(Default)]
struct Bucket {
    /// abs file path -> summed edit count
    files: BTreeMap<String, u32>,
    repo_root: Option<PathBuf>,
    /// session_id -> session detail (dedup per session)
    details: BTreeMap<String, String>,
}

/// Generate the weekly DevLog for the ISO weeks covered by `dates`.
///
/// Grain is (date × project): all of a day's CC sessions are bucketed by the
/// project each modified file belongs to, so one entry == one project's work on
/// one day. Incidental buckets (memory/config/notes churn) are dropped on any
/// day that also has substantive work; a day with only incidental work collapses
/// to a single "misc" entry. git commits are scoped to the exact files touched.
fn run_devlog(dates: &[NaiveDate], no_ai: bool, write: bool) -> Result<()> {
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

        // Bucket every file of every non-trivial, non-clinical CC session by
        // (date, project). A session that spans projects contributes its files
        // to each project's bucket. continuum_sessions are ignored (v1).
        let mut buckets: BTreeMap<(NaiveDate, String), Bucket> = BTreeMap::new();
        for date in &week_dates {
            let activity = match activity::fetch_activity(*date) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("Warning: skipping {}: {}", date, e);
                    continue;
                }
            };
            for cc in &activity.cc_sessions {
                if matching::is_clinical(cc) {
                    continue;
                }
                let unified = matching::unify_cc_session(cc);
                if matching::is_trivial(&unified) {
                    continue;
                }
                for (path, edits) in &cc.files_modified {
                    let (project, repo_root) =
                        project::classify(path).unwrap_or_else(|| ("misc".to_string(), None));
                    let b = buckets.entry((*date, project)).or_default();
                    *b.files.entry(path.clone()).or_insert(0) += *edits;
                    if b.repo_root.is_none() {
                        b.repo_root = repo_root;
                    }
                    b.details
                        .entry(cc.session_id.clone())
                        .or_insert_with(|| unified.detail.clone());
                }
            }
        }

        // Which dates have at least one substantive bucket?
        let mut substantive_dates: BTreeSet<NaiveDate> = BTreeSet::new();
        for (date, project) in buckets.keys() {
            if project::is_substantive(project) {
                substantive_dates.insert(*date);
            }
        }

        // Keep substantive buckets on substantive days; collapse an all-incidental
        // day into one "misc" entry so it isn't silently dropped.
        let mut entries: Vec<devlog::DevLogEntry> = Vec::new();
        let mut incidental_merge: BTreeMap<NaiveDate, Bucket> = BTreeMap::new();
        for ((date, project), bucket) in buckets {
            if substantive_dates.contains(&date) {
                if project::is_substantive(&project) {
                    entries.push(build_entry(date, project, bucket, no_ai));
                }
            } else {
                let m = incidental_merge.entry(date).or_default();
                for (f, e) in bucket.files {
                    *m.files.entry(f).or_insert(0) += e;
                }
                for (sid, d) in bucket.details {
                    m.details.entry(sid).or_insert(d);
                }
            }
        }
        for (date, bucket) in incidental_merge {
            entries.push(build_entry(date, "misc".to_string(), bucket, no_ai));
        }

        let rendered = devlog::render_week(&week_label, &entries);
        let path = devlog::week_path(monday);
        if write {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| anyhow::anyhow!("Failed to create DevLog dir: {}", e))?;
            }
            std::fs::write(&path, &rendered)
                .map_err(|e| anyhow::anyhow!("Failed to write {}: {}", path.display(), e))?;
            eprintln!("Wrote {} ({} entries)", path.display(), entries.len());
        } else {
            println!("--- {} ---", path.display());
            print!("{}", rendered);
        }
    }

    Ok(())
}

/// Build a DevLog entry from a (date, project) bucket: scope git to the exact
/// files touched, derive a stable prose-cache key from the contributing session
/// set (so a finished day's prose is cached, but the current day re-drafts as it
/// grows).
fn build_entry(date: NaiveDate, project: String, bucket: Bucket, no_ai: bool) -> devlog::DevLogEntry {
    let files_count = bucket.files.len();
    let edits: u32 = bucket.files.values().sum();

    let (commits, prs) = match &bucket.repo_root {
        Some(root) => {
            let rel: Vec<String> = bucket
                .files
                .keys()
                .filter_map(|f| git::relativize(root, f))
                .collect();
            if rel.is_empty() {
                (Vec::new(), Vec::new())
            } else {
                let day_start = date.and_hms_opt(0, 0, 0).unwrap().and_utc();
                let day_end = date
                    .succ_opt()
                    .unwrap_or(date)
                    .and_hms_opt(0, 0, 0)
                    .unwrap()
                    .and_utc();
                let found = git::commits_in_window(root, day_start, day_end, &rel);
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
        }
        None => (Vec::new(), Vec::new()),
    };

    // Stable cache key: date + project + hash of the contributing session set.
    let mut hasher = DefaultHasher::new();
    for sid in bucket.details.keys() {
        sid.hash(&mut hasher);
    }
    let cache_key = format!("{}-{}-{:x}", date, project, hasher.finish());
    let detail = bucket
        .details
        .values()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n");
    let (prose, topics) = devlog::resolve_prose(&cache_key, &detail, no_ai);

    devlog::DevLogEntry {
        date,
        primary_project: project.clone(),
        all_projects: vec![project],
        prs,
        commits,
        topics,
        prose,
        files_count,
        edits,
    }
}
