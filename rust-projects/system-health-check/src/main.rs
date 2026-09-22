//! system-health-check — daily system health validator.
//!
//! Catches dead timers/agents, failed services, uncommitted dotfiles, dirty or
//! unpushed `~/Code` repositories, missing Rust tool binaries, DNA drift
//! (state-capture), dotter drift (dotter-drift-monitor), and orphaned headless
//! Chrome processes. systemd on Linux, launchd on macOS. Runs via a systemd
//! timer (Linux) or launchd plist (macOS) daily at 08:00.
//!
//! Rust port 2026-09-01 of the Nushell script (which crashed on every Mac run
//! from 2026-07-17 to 2026-09-01 on `first` over an empty list — a compile
//! error here). CLI, log format, problem strings and notifications are
//! unchanged; the Nushell version was the oracle.
//!
//! Exit: 0 when every check ran and found nothing, 1 otherwise. Note that the
//! tool's OWN unit therefore goes `failed` whenever it finds a problem; the
//! Linux services check skips itself for that reason.

mod checks;
mod exec;
mod git_audit;
mod history;

use clap::Parser;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

#[derive(Parser, Debug)]
#[command(name = "system-health-check", version, about = "Daily system health validator (systemd / launchd)")]
struct Cli {
    /// Show all checks even when healthy
    #[arg(short, long)]
    verbose: bool,
    /// Attempt auto-repair: restart dead timers/services, reload agents
    #[arg(short, long)]
    fix: bool,
    /// Do not post confirmed problems to, or archive cleared ones from, the Messageboard
    #[arg(long)]
    no_board: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let is_macos = cfg!(target_os = "macos");
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    let log_path = home.join(".local/share/system-health-check.log");

    let log = |level: &str, message: &str| {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&log_path) {
            let _ = writeln!(f, "[{ts}] {level} {message}");
        }
    };

    if cli.verbose {
        println!("System Health Check [{}]", if is_macos { "macOS" } else { "Linux" });
        println!("────────────────────────────────────────");
        println!();
    }

    let real = exec::Real;
    let ctx = checks::Ctx { exec: &real, verbose: cli.verbose, fix: cli.fix, home: home.clone(), log: &log };
    let hostname = short_hostname();
    let host = if is_macos { "macos".to_string() } else { hostname.clone() };
    let nu_version = {
        use exec::Exec;
        let r = real.run("nu", &["--version"]);
        r.ok().then(|| r.out()).filter(|v| !v.is_empty())
    };

    let mut problems: Vec<String> = vec![];
    if is_macos {
        problems.extend(checks::check_launchagents(&ctx));
        problems.extend(checks::check_mac_services(&ctx));
    } else {
        problems.extend(checks::check_timers(&ctx));
        problems.extend(checks::check_services(&ctx));
    }
    problems.extend(checks::check_dotfiles(&ctx));
    problems.extend(checks::check_code_repositories(&ctx));
    problems.extend(checks::check_rust_tools(&ctx));
    problems.extend(checks::check_dna_drift(&ctx, is_macos));
    problems.extend(checks::check_dotter_drift(&ctx));
    problems.extend(checks::check_nu_watch_flag(&ctx, nu_version.as_deref(), &host));
    problems.extend(checks::check_derived_docs(&ctx));
    problems.extend(checks::check_watcher_heartbeats(&ctx));
    problems.extend(checks::check_stray_chrome(&ctx));
    problems.extend(checks::check_undelivered_alerts(&ctx));

    // Status file for the session-start kernel (ai-brief "Host health"):
    // one writer per file, namespaced by machine, under the Syncthing-carried
    // ~/Assistants tree so every session on either machine sees both hosts and
    // the AGE of each result. A stale file is itself the signal that this
    // check has died — the failure mode the Mac lived in for six weeks.
    // Problem persistence: fold into last run's records (same file, same
    // single writer) so the brief can say how long a red has stood, and so a
    // problem confirmed on its second run reaches the Messageboard.
    let now = chrono::Local::now().to_rfc3339();
    let (previous, previous_board) = read_previous_history(&home, &host);
    let (records, cleared) = history::merge_history(&previous, &problems, &now);
    let board_keys = if cli.no_board { previous_board } else { post_to_board(&host, &records, &previous_board, &log) };
    let _ = cleared;
    if let Err(e) = write_status(&home, &host, &hostname, nu_version.as_deref(), &problems, &records, &board_keys) {
        eprintln!("system-health-check: could not write status file: {e}");
    }

    if problems.is_empty() {
        log("INFO", "All checks passed");
        if cli.verbose {
            println!("All checks passed.");
        }
        return ExitCode::SUCCESS;
    }

    let count = problems.len();
    let label = if count == 1 { "problem" } else { "problems" };
    log("WARN", &format!("{count} {label} found"));
    for p in &problems {
        log("WARN", &format!("  {p}"));
    }
    if cli.verbose {
        println!("{count} {label} found.");
    } else {
        println!("system-health-check: {count} {label}:");
        for p in &problems {
            println!("  - {p}");
        }
    }
    notify(&problems);
    ExitCode::from(1)
}

#[derive(serde::Serialize)]
struct Status<'a> {
    schema: u32,
    /// Orientation machine-layer key: "macos" or the short hostname ("nimbini")
    host: String,
    hostname: String,
    checked_at: String,
    count: usize,
    problems: &'a [String],
    tool_version: &'static str,
    /// `nu --version` on this host; read by peers' watch-flag check (Check 7).
    nu_version: Option<&'a str>,
    /// Same problems with key / first_seen / consecutive runs (2026-09-22; additive).
    problem_history: &'a [history::ProblemRecord],
    /// Confirmed keys the Messageboard `HEALTH-<host>` section currently lists.
    board_keys: &'a [String],
}

fn read_previous_history(home: &std::path::Path, host: &str) -> (Vec<history::ProblemRecord>, Vec<String>) {
    let path = home.join("Assistants/health").join(format!("{host}.json"));
    let Some(v) = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else {
        return (vec![], vec![]);
    };
    let history = v.get("problem_history").cloned().and_then(|h| serde_json::from_value(h).ok()).unwrap_or_default();
    let board = v.get("board_keys").cloned().and_then(|b| serde_json::from_value(b).ok()).unwrap_or_default();
    (history, board)
}

/// Messageboard delivery through the repo's own atomic editor: one rolling
/// `HEALTH-<host>` section, touched only when the confirmed set changes.
/// Best effort — a missing editor or a failed call is logged, never fatal; the
/// status file still carries the history, and `board_keys` records what the
/// board is believed to show so the next run can reconcile.
/// Returns the keys the board shows after this run.
fn post_to_board(host: &str, records: &[history::ProblemRecord], previously_posted: &[String], log: &dyn Fn(&str, &str)) -> Vec<String> {
    let confirmed = history::confirmed_keys(records);
    let action = history::board_action(previously_posted, &confirmed);
    if action == history::BoardAction::Nothing {
        return previously_posted.to_vec();
    }
    let editor = "messageboard-edit";
    let tag = format!("HEALTH-{host} —");
    let run = |args: &[&str]| -> Result<(), String> {
        match Command::new(editor).args(args).output() {
            Ok(o) if o.status.success() => Ok(()),
            Ok(o) => Err(format!("exit {}", o.status.code().unwrap_or(-1))),
            Err(e) => Err(format!("{editor} unavailable: {e}")),
        }
    };
    if !previously_posted.is_empty() {
        if let Err(e) = run(&["archive-containing", &tag]) {
            log("WARN", &format!("board: could not archive {tag} [{e}]"));
            return previously_posted.to_vec();
        }
    }
    if action == history::BoardAction::Archive {
        log("INFO", &format!("board: archived {tag} — no confirmed problems"));
        return vec![];
    }
    match run(&["insert", &history::board_summary(host, records)]) {
        Ok(()) => {
            log("INFO", &format!("board: posted {tag} {} confirmed", confirmed.len()));
            confirmed
        }
        Err(e) => {
            log("WARN", &format!("board: post failed [{e}]"));
            vec![]
        }
    }
}

/// /etc/hostname first (Arch ships no `hostname` binary by default), then the
/// command, then $HOSTNAME. Lower-cased, domain stripped.
fn short_hostname() -> String {
    let candidates: [Option<String>; 3] = [
        std::fs::read_to_string("/etc/hostname").ok(),
        Command::new("hostname").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()),
        std::env::var("HOSTNAME").ok(),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(|h| h.trim().to_lowercase())
        .filter_map(|h| h.split('.').next().map(String::from))
        .find(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn write_status(home: &std::path::Path, host: &str, hostname: &str, nu_version: Option<&str>, problems: &[String], problem_history: &[history::ProblemRecord], board_keys: &[String]) -> std::io::Result<()> {
    let dir = home.join("Assistants/health");
    std::fs::create_dir_all(&dir)?;
    let status = Status {
        schema: 1,
        host: host.to_string(),
        hostname: hostname.to_string(),
        checked_at: chrono::Local::now().to_rfc3339(),
        count: problems.len(),
        problems,
        tool_version: env!("CARGO_PKG_VERSION"),
        nu_version,
        problem_history,
        board_keys,
    };
    let json = serde_json::to_string_pretty(&status).map_err(std::io::Error::other)?;
    // atomic replace so a reader (or Syncthing) never sees a half-written file
    let tmp = dir.join(format!(".{host}.json.tmp"));
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, dir.join(format!("{host}.json")))
}

/// The health check's own desktop alert, through `notify-user` so the attempt
/// is recorded and a failed delivery shows up in Check 11 next run.
fn notify(problems: &[String]) {
    let body = problems.join("\n");
    let _ = Command::new("notify-user")
        .args(["--tool", "system-health-check", "--urgency", "critical", "System Health Check", &body])
        .status();
}
