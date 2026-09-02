use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

fn heartbeat_file() -> PathBuf {
    heartbeat_file_in(&dirs::home_dir().expect("could not find home directory"))
}

fn heartbeat_file_in(home: &Path) -> PathBuf {
    // Syncthing-shared location — visible to all machines
    let shared = home.join("Assistants/shared/bequest-heartbeat");
    if shared.parent().is_some_and(|p| p.exists()) {
        return shared;
    }
    // Fallback to local if shared dir doesn't exist
    home.join(".bequest").join("last-heartbeat")
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
    latest_activity_in(&dirs::home_dir().expect("could not find home directory"), true)
}

/// The rule every signal must satisfy: **nothing this check does itself may
/// count as activity.** Until 2026-09-02 the scan ran `git fetch` on
/// `~/dotfiles` and then read `.git/FETCH_HEAD`'s mtime — a file the fetch
/// always rewrites — so every check reported "0 minutes ago" and the switch
/// could never fire (system review D2-7). The fetch and that signal are gone.
///
/// `system_signals` = false skips the host-wide probes (`last`, the Sent
/// maildir) so a test can drive the scan from a fixture home.
fn latest_activity_in(home: &Path, system_signals: bool) -> Result<(SystemTime, Vec<Signal>)> {
    let mut signals = Vec::new();

    // Signal 1: explicit heartbeat file
    if let Some(t) = file_mtime(&heartbeat_file_in(home)) {
        signals.push(Signal {
            name: "heartbeat file".into(),
            time: t,
        });
    }

    // Signal 2: dotfiles git index. Local commits and checkouts move it —
    // including the auto-commit watchers and state-capture's nightly commit,
    // so it is presence-of-the-machines as much as presence-of-Will.
    let dotfiles_dir = home.join("dotfiles");
    if let Some(t) = file_mtime(&dotfiles_dir.join(".git/index")) {
        signals.push(Signal {
            name: "dotfiles local git".into(),
            time: t,
        });
    }

    // Signal 3: most recent DayPage modification (collect-entries also
    // writes these on a schedule — same caveat as the git index)
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

    if system_signals {
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
    // Signal: filesystem mtime of the Gmail Sent-Mail maildir's `cur/`
    // directory. Maildir convention — `cur/` receives a new file every
    // time a message is classified as "seen" (lieer pulls the sent copy
    // from Gmail shortly after any outgoing send, regardless of which
    // client sent it: msmtp, the Gmail web UI, phone, etc.). Folder
    // mtime updates whenever a file is added or removed.
    //
    // This replaces an earlier himalaya subprocess path that parsed
    // JSON envelope lists. Simpler, faster, works offline, covers all
    // send channels (not just CLI). Returns None if the maildir isn't
    // present on this machine — fine, heartbeat has other signals.
    let sent_cur = dirs::home_dir()?
        .join("Mail/personal/[Google Mail]/Sent Mail/cur");
    fs::metadata(&sent_cur).ok()?.modified().ok()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn touch_at(path: &Path, t: SystemTime) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let f = File::create(path).unwrap();
        f.set_modified(t).unwrap();
    }

    /// D2-7 regression: the check's own side effects never count as activity,
    /// and repeated checks do not advance the latest-activity timestamp.
    #[test]
    fn a_fresh_fetch_head_is_not_activity_and_checks_do_not_advance_time() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let now = SystemTime::now();
        let ten_days_ago = now - Duration::from_secs(10 * 86400);
        let twenty_days_ago = now - Duration::from_secs(20 * 86400);

        touch_at(&home.join(".bequest/last-heartbeat"), ten_days_ago);
        touch_at(&home.join("dotfiles/.git/index"), twenty_days_ago);
        touch_at(&home.join("dotfiles/.git/FETCH_HEAD"), now); // what a fetch leaves behind

        let (latest, signals) = latest_activity_in(home, false).unwrap();
        assert!(!signals.iter().any(|s| s.name.contains("git activity")), "FETCH_HEAD must not be a signal: {:?}", signals.iter().map(|s| &s.name).collect::<Vec<_>>());
        let drift = latest.duration_since(ten_days_ago).unwrap_or(Duration::ZERO);
        assert!(drift < Duration::from_secs(2), "latest must be the 10-day-old heartbeat, not now");
        assert_eq!(classify(now.duration_since(latest).unwrap(), 7, 7), State::Warning);

        let (again, _) = latest_activity_in(home, false).unwrap();
        assert_eq!(again, latest, "a second check must not move the timestamp");
        assert!(!home.join("dotfiles/.git/FETCH_HEAD").exists() || file_mtime(&home.join("dotfiles/.git/FETCH_HEAD")).unwrap() >= now, "fixture untouched");
    }
}
