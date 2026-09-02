//! system-health-check — daily system health validator.
//!
//! Catches dead timers/agents, failed services, uncommitted dotfiles, missing
//! Rust tool binaries, DNA drift (state-capture) and dotter drift
//! (dotter-drift-monitor). systemd on Linux, launchd on macOS. Runs via
//! systemd timer (Linux) or launchd plist (macOS) daily at 08:00.
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
    problems.extend(checks::check_rust_tools(&ctx));
    problems.extend(checks::check_dna_drift(&ctx, is_macos));
    problems.extend(checks::check_dotter_drift(&ctx));
    problems.extend(checks::check_nu_watch_flag(&ctx, nu_version.as_deref(), &host));

    // Status file for the session-start kernel (ai-brief "Host health"):
    // one writer per file, namespaced by machine, under the Syncthing-carried
    // ~/Assistants tree so every session on either machine sees both hosts and
    // the AGE of each result. A stale file is itself the signal that this
    // check has died — the failure mode the Mac lived in for six weeks.
    if let Err(e) = write_status(&home, &host, &hostname, nu_version.as_deref(), &problems) {
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

fn write_status(home: &std::path::Path, host: &str, hostname: &str, nu_version: Option<&str>, problems: &[String]) -> std::io::Result<()> {
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
    };
    let json = serde_json::to_string_pretty(&status).map_err(std::io::Error::other)?;
    // atomic replace so a reader (or Syncthing) never sees a half-written file
    let tmp = dir.join(format!(".{host}.json.tmp"));
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, dir.join(format!("{host}.json")))
}

/// Desktop notification, platform-aware. Best effort; failures are ignored.
fn notify(problems: &[String]) {
    let on_path = |bin: &str| {
        std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
            .unwrap_or(false)
    };
    if on_path("notify-send") {
        let body = problems.join("\n");
        let _ = Command::new("notify-send").args(["--urgency=critical", "System Health Check", &body]).status();
    } else if on_path("osascript") {
        let body = problems.join(", ").replace('"', "'");
        let script = format!("display notification \"{body}\" with title \"System Health Check\" sound name \"Basso\"");
        let _ = Command::new("osascript").args(["-e", &script]).status();
    }
}
