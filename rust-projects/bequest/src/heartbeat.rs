use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

fn bequest_dir() -> PathBuf {
    dirs::home_dir()
        .expect("could not find home directory")
        .join(".bequest")
}

fn heartbeat_file() -> PathBuf {
    // Syncthing-shared location — visible to all machines
    let shared = dirs::home_dir()
        .expect("could not find home directory")
        .join("Assistants/shared/bequest-heartbeat");
    if shared.parent().is_some_and(|p| p.exists()) {
        return shared;
    }
    // Fallback to local if shared dir doesn't exist
    bequest_dir().join("last-heartbeat")
}

/// Record a heartbeat (touch the heartbeat file).
pub fn record() -> Result<()> {
    let path = heartbeat_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("creating heartbeat file directory")?;
    }
    // Write current timestamp as content, and the mtime updates too
    let now = humantime::format_rfc3339_seconds(SystemTime::now()).to_string();
    fs::write(&path, &now).context("writing heartbeat file")?;
    eprintln!("Heartbeat recorded: {}", now);
    Ok(())
}

/// Scan multiple activity signals and return the most recent timestamp.
fn latest_activity() -> Result<(SystemTime, Vec<Signal>)> {
    let mut signals = Vec::new();

    // Signal 1: explicit heartbeat file
    if let Some(t) = file_mtime(&heartbeat_file()) {
        signals.push(Signal {
            name: "heartbeat file".into(),
            time: t,
        });
    }

    // Signal 2: git index (any dotfiles activity)
    // Fetch first so we see pushes from other machines
    let home = dirs::home_dir().unwrap();
    let dotfiles_dir = home.join("dotfiles");
    if dotfiles_dir.join(".git").exists() {
        let _ = Command::new("git")
            .args(["-C", &dotfiles_dir.to_string_lossy(), "fetch", "--quiet"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    if let Some(t) = file_mtime(&dotfiles_dir.join(".git/FETCH_HEAD")) {
        signals.push(Signal {
            name: "dotfiles git activity".into(),
            time: t,
        });
    }
    // Also check local index (local commits that haven't been pushed)
    if let Some(t) = file_mtime(&dotfiles_dir.join(".git/index")) {
        signals.push(Signal {
            name: "dotfiles local git".into(),
            time: t,
        });
    }

    // Signal 3: most recent DayPage modification
    let daypage_dir = home.join("Forge/NapierianLogs/DayPages");
    if daypage_dir.exists() {
        if let Some(t) = most_recent_file_in(&daypage_dir, "md") {
            signals.push(Signal {
                name: "DayPage edit".into(),
                time: t,
            });
        }
    }

    // Signal 4: nushell history
    let nu_history = home.join(".config/nushell/history.sqlite3");
    if let Some(t) = file_mtime(&nu_history) {
        signals.push(Signal {
            name: "shell history".into(),
            time: t,
        });
    }

    // Signal 5: SSH auth log (last login)
    if let Some(t) = last_login() {
        signals.push(Signal {
            name: "system login".into(),
            time: t,
        });
    }

    // Signal 6: Sent email (proves human action, not just incoming spam)
    if let Some(t) = last_sent_email() {
        signals.push(Signal {
            name: "sent email".into(),
            time: t,
        });
    }

    // Signal 7: explicit heartbeat via SSH from iPhone
    // iPhone Shortcut → "Run Script Over SSH" → nimbini via Tailscale
    // → bequest heartbeat ping (touches the heartbeat file)
    // This is picked up by Signal 1 (heartbeat file) — no separate signal needed.

    Ok((
        signals.iter().map(|s| s.time).max().unwrap_or(SystemTime::UNIX_EPOCH),
        signals,
    ))
}

struct Signal {
    name: String,
    time: SystemTime,
}

fn file_mtime(path: &PathBuf) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

fn most_recent_file_in(dir: &PathBuf, ext: &str) -> Option<SystemTime> {
    let mut latest = None;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == ext) {
                if let Ok(meta) = entry.metadata() {
                    if let Ok(mtime) = meta.modified() {
                        latest = Some(latest.map_or(mtime, |l: SystemTime| l.max(mtime)));
                    }
                }
            }
        }
    }
    latest
}

fn last_sent_email() -> Option<SystemTime> {
    // Query himalaya for the most recent sent email
    let output = Command::new("himalaya")
        .args([
            "--quiet",
            "envelope",
            "list",
            "-f",
            "[Google Mail]/Sent Mail",
            "-s",
            "1",
            "-o",
            "json",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // Find the JSON array in the output (skip any non-JSON lines)
    let json_start = text.find("[{")?;
    let json_text = &text[json_start..];
    // Extract the date field: "date":"2026-04-04 13:51+01:00"
    let date_start = json_text.find("\"date\":\"")?;
    let date_val = &json_text[date_start + 8..];
    let date_end = date_val.find('"')?;
    let date_str = &date_val[..date_end];
    // Format: "2026-04-04 13:51+01:00" — normalize to RFC 3339
    // Find the date/time split (space), and the timezone part
    let space = date_str.find(' ')?;
    let date_part = &date_str[..space];
    let rest = &date_str[space + 1..];
    // rest is "13:51+01:00" — find where time ends and tz begins
    // Time is HH:MM or HH:MM:SS, tz starts with + or - after the time
    let tz_start = rest[5..].find(['+', '-']).map(|i| i + 5)?;
    let time_part = &rest[..tz_start];
    let tz_part = &rest[tz_start..];
    let time_with_secs = if time_part.len() == 5 {
        format!("{}:00", time_part) // HH:MM → HH:MM:SS
    } else {
        time_part.to_string()
    };
    // humantime only accepts Z (UTC), so convert the offset to UTC
    // Parse timezone offset like "+01:00" or "-05:00"
    let tz_sign: i64 = if tz_part.starts_with('+') { 1 } else { -1 };
    let tz_hours: i64 = tz_part[1..3].parse().ok()?;
    let tz_mins: i64 = tz_part[4..6].parse().ok()?;
    let tz_offset_secs = tz_sign * (tz_hours * 3600 + tz_mins * 60);
    // Parse as UTC then adjust
    let utc_str = format!("{}T{}Z", date_part, time_with_secs);
    let t = humantime::parse_rfc3339(&utc_str).ok()?;
    // Subtract the offset to get actual UTC
    if tz_offset_secs >= 0 {
        t.checked_sub(Duration::from_secs(tz_offset_secs as u64))
    } else {
        t.checked_add(Duration::from_secs((-tz_offset_secs) as u64))
    }
}

fn last_login() -> Option<SystemTime> {
    let output = Command::new("last")
        .args(["-1", "will", "--time-format", "iso"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // First line looks like: will     pts/0    ... 2026-04-05T10:48:00+01:00 ...
    let first_line = text.lines().next()?;
    // Find an ISO-ish timestamp
    for field in first_line.split_whitespace() {
        if field.len() >= 19 && field.contains('T') {
            if let Ok(t) = humantime::parse_rfc3339(field) {
                return Some(t);
            }
            // Try without timezone offset by appending Z
            if let Ok(t) = humantime::parse_rfc3339(&format!("{}Z", &field[..19])) {
                return Some(t);
            }
        }
    }
    None
}

fn format_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    if days > 0 {
        format!("{} days, {} hours", days, hours)
    } else if hours > 0 {
        format!("{} hours", hours)
    } else {
        let mins = total_secs / 60;
        format!("{} minutes", mins)
    }
}

fn format_time(t: SystemTime) -> String {
    humantime::format_rfc3339_seconds(t).to_string()
}

/// State of the dead man's switch.
#[derive(Debug, PartialEq)]
pub enum State {
    Normal,
    Warning,
    Triggered,
}

/// Show heartbeat status with all signal details.
pub fn status(threshold_days: u64, grace_days: u64) -> Result<()> {
    let (latest, signals) = latest_activity()?;
    let now = SystemTime::now();
    let elapsed = now.duration_since(latest).unwrap_or(Duration::ZERO);
    let state = classify(elapsed, threshold_days, grace_days);

    println!("Signals detected:");
    if signals.is_empty() {
        println!("  (none)");
    } else {
        let mut sorted = signals;
        sorted.sort_by(|a, b| b.time.cmp(&a.time));
        for s in &sorted {
            let age = now.duration_since(s.time).unwrap_or(Duration::ZERO);
            println!("  {} — {} ago ({})", s.name, format_duration(age), format_time(s.time));
        }
    }

    println!();
    println!("Latest activity: {} ago", format_duration(elapsed));
    println!("Threshold:       {} days", threshold_days);
    println!("Grace period:    {} days", grace_days);
    println!("State:           {}", state_label(&state));

    if state == State::Warning {
        let warning_elapsed = elapsed.as_secs() / 86400 - threshold_days;
        let remaining = grace_days.saturating_sub(warning_elapsed);
        println!("Grace remaining: {} days", remaining);
    }

    Ok(())
}

/// Check heartbeat state. Returns exit code: 0=normal, 1=warning, 2=triggered.
pub fn check(threshold_days: u64, grace_days: u64) -> Result<State> {
    let (latest, _) = latest_activity()?;
    let now = SystemTime::now();
    let elapsed = now.duration_since(latest).unwrap_or(Duration::ZERO);
    let state = classify(elapsed, threshold_days, grace_days);

    match &state {
        State::Normal => {
            println!("OK — last activity {} ago", format_duration(elapsed));
        }
        State::Warning => {
            let warning_elapsed = elapsed.as_secs() / 86400 - threshold_days;
            let remaining = grace_days.saturating_sub(warning_elapsed);
            eprintln!(
                "WARNING — no activity for {}. Grace period: {} days remaining.",
                format_duration(elapsed),
                remaining
            );
        }
        State::Triggered => {
            eprintln!(
                "TRIGGERED — no activity for {}. Threshold + grace period exceeded.",
                format_duration(elapsed)
            );
        }
    }

    Ok(state)
}

fn classify(elapsed: Duration, threshold_days: u64, grace_days: u64) -> State {
    let days = elapsed.as_secs() / 86400;
    if days < threshold_days {
        State::Normal
    } else if days < threshold_days + grace_days {
        State::Warning
    } else {
        State::Triggered
    }
}

fn state_label(state: &State) -> &'static str {
    match state {
        State::Normal => "NORMAL",
        State::Warning => "WARNING — inside grace period",
        State::Triggered => "TRIGGERED — disclosure threshold exceeded",
    }
}
