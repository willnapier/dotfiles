//! notify-user — the one way unattended tools raise a desktop alert.
//!
//! Audit finding D2-23 (2026-08-01): every tool called `notify-send` /
//! `osascript` behind `| ignore` or `|| true`, so a failed delivery vanished
//! and nobody could tell an alert that was never seen from one that was
//! dismissed. This binary tries the platform channel, then appends one JSON
//! line per attempt — delivered or not, and why not — to
//! `~/.local/state/alerts/<tool>.jsonl`. `system-health-check` reads those
//! files and reports undelivered alerts as a problem, which the Messageboard
//! summary then carries. Exit 0 = delivered, 1 = recorded but not delivered,
//! 2 = bad arguments. Never panics on a missing notifier.
//!
//! Callers keep their `|| true`: the record is made here, not by them.

use clap::{Parser, ValueEnum};
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

#[derive(Parser)]
#[command(name = "notify-user", version, about = "Deliver a desktop alert and record whether it arrived")]
struct Cli {
    /// Name of the tool raising the alert (file name under ~/.local/state/alerts/)
    #[arg(long)]
    tool: String,
    #[arg(long, value_enum, default_value_t = Urgency::Normal)]
    urgency: Urgency,
    /// Where to record attempts (default ~/.local/state/alerts)
    #[arg(long)]
    state_dir: Option<PathBuf>,
    title: String,
    body: String,
}

#[derive(Clone, Copy, ValueEnum, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Urgency {
    Normal,
    Critical,
}

#[derive(Serialize, Debug)]
struct Attempt<'a> {
    ts: String,
    tool: &'a str,
    title: &'a str,
    urgency: Urgency,
    delivered: bool,
    /// Channel that delivered, or the one that was tried.
    channel: &'static str,
    /// Why it was not delivered, when it was not.
    error: Option<String>,
}

/// Result of trying the platform channels in order.
#[derive(Debug, PartialEq, Eq)]
struct Delivery {
    delivered: bool,
    channel: &'static str,
    error: Option<String>,
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file())).unwrap_or(false)
}

/// Linux: libnotify needs a session bus and a display; without them
/// `notify-send` blocks or fails, so say so instead of trying.
fn linux_session_present() -> bool {
    (std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some())
        && std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    match Command::new(program).args(args).output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(format!("{program} exit {}: {}", o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Err(format!("{program}: {e}")),
    }
}

fn deliver(title: &str, body: &str, urgency: Urgency) -> Delivery {
    if cfg!(target_os = "macos") {
        if on_path("terminal-notifier") {
            let mut args = vec!["-title", title, "-message", body];
            if urgency == Urgency::Critical {
                args.extend(["-sound", "Basso"]);
            }
            return match run("terminal-notifier", &args) {
                Ok(()) => Delivery { delivered: true, channel: "terminal-notifier", error: None },
                Err(e) => Delivery { delivered: false, channel: "terminal-notifier", error: Some(e) },
            };
        }
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let sound = if urgency == Urgency::Critical { " sound name \"Basso\"" } else { "" };
        let script = format!("display notification \"{}\" with title \"{}\"{sound}", esc(body), esc(title));
        return match run("osascript", &["-e", &script]) {
            Ok(()) => Delivery { delivered: true, channel: "osascript", error: None },
            Err(e) => Delivery { delivered: false, channel: "osascript", error: Some(e) },
        };
    }
    if !on_path("notify-send") {
        return Delivery { delivered: false, channel: "notify-send", error: Some("notify-send not on PATH".into()) };
    }
    if !linux_session_present() {
        return Delivery { delivered: false, channel: "notify-send", error: Some("no graphical session (DISPLAY/WAYLAND_DISPLAY + DBUS_SESSION_BUS_ADDRESS)".into()) };
    }
    let u = if urgency == Urgency::Critical { "critical" } else { "normal" };
    match run("notify-send", &["--urgency", u, title, body]) {
        Ok(()) => Delivery { delivered: true, channel: "notify-send", error: None },
        Err(e) => Delivery { delivered: false, channel: "notify-send", error: Some(e) },
    }
}

/// Append one JSON line; the directory is created on first use.
fn record(dir: &Path, attempt: &Attempt) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("{}.jsonl", attempt.tool)))?;
    writeln!(f, "{}", serde_json::to_string(attempt).map_err(std::io::Error::other)?)
}

fn default_state_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/")).join(".local/state/alerts")
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.tool.is_empty() || cli.tool.contains('/') {
        eprintln!("notify-user: --tool must be a plain name");
        return ExitCode::from(2);
    }
    let d = deliver(&cli.title, &cli.body, cli.urgency);
    let attempt = Attempt {
        ts: chrono::Local::now().to_rfc3339(),
        tool: &cli.tool,
        title: &cli.title,
        urgency: cli.urgency,
        delivered: d.delivered,
        channel: d.channel,
        error: d.error.clone(),
    };
    let dir = cli.state_dir.unwrap_or_else(default_state_dir);
    if let Err(e) = record(&dir, &attempt) {
        eprintln!("notify-user: could not record attempt in {}: {e}", dir.display());
    }
    if d.delivered {
        ExitCode::SUCCESS
    } else {
        eprintln!("notify-user: not delivered via {}: {}", d.channel, d.error.unwrap_or_default());
        ExitCode::from(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_appends_one_json_line_per_attempt_and_creates_the_dir() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("alerts");
        let a = Attempt { ts: "2026-09-22T18:00:00+01:00".into(), tool: "rustic-backup", title: "Rustic backup", urgency: Urgency::Critical, delivered: false, channel: "notify-send", error: Some("no graphical session".into()) };
        record(&dir, &a).unwrap();
        let b = Attempt { delivered: true, error: None, channel: "osascript", ..a };
        record(&dir, &b).unwrap();
        let text = std::fs::read_to_string(dir.join("rustic-backup.jsonl")).unwrap();
        let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["delivered"], false);
        assert_eq!(lines[0]["error"], "no graphical session");
        assert_eq!(lines[0]["urgency"], "critical");
        assert_eq!(lines[1]["delivered"], true);
        assert!(lines[1]["error"].is_null());
    }

    #[test]
    fn linux_session_rule_needs_display_and_bus() {
        // Environment is process-global; only assert the rule's shape on the
        // values present, never mutate the environment in a test.
        let has_display = std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
        let has_bus = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some();
        assert_eq!(linux_session_present(), has_display && has_bus);
    }
}
