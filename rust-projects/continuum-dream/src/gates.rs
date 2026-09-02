use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::fs;
use std::path::PathBuf;

use crate::types::DreamState;

/// The timer fires daily at 04:00; consecutive runs are ~23h59m apart
/// and `num_hours()` truncates, so a 24 h gate skipped every other
/// night (system review D1-3, fixed 2026-09-02). 23 h keeps the intent
/// ("at most once a day") with an hour of slack for timer jitter.
const MINIMUM_MINUTES: i64 = 23 * 60;
const MINIMUM_NEW_SESSIONS: usize = 5;
const LOCK_STALE_MINUTES: u64 = 30;

fn lock_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("No home directory")?;
    Ok(home.join(".local/share/continuum-dream/continuum-dream.lock"))
}

/// Result of checking all gates
pub struct GateResult {
    pub passed: bool,
    pub reason: Option<String>,
}

/// Check time gate: 24h since last dream
pub fn check_time_gate(state: &DreamState) -> GateResult {
    match &state.last_dream_time {
        None => GateResult {
            passed: true,
            reason: None,
        },
        Some(last) => {
            let last_time = match DateTime::parse_from_rfc3339(last) {
                Ok(t) => t.with_timezone(&Utc),
                Err(_) => {
                    return GateResult {
                        passed: true,
                        reason: None,
                    }
                }
            };
            time_gate_at(last_time, Utc::now())
        }
    }
}

fn time_gate_at(last: DateTime<Utc>, now: DateTime<Utc>) -> GateResult {
    let minutes = (now - last).num_minutes();
    if minutes >= MINIMUM_MINUTES {
        GateResult {
            passed: true,
            reason: None,
        }
    } else {
        GateResult {
            passed: false,
            reason: Some(format!(
                "time gate: {}h{:02}m elapsed, {}h required",
                minutes / 60,
                minutes % 60,
                MINIMUM_MINUTES / 60
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    /// D1-3 regression: two 04:00 timer firings 23h59m apart must both run.
    #[test]
    fn consecutive_daily_firings_pass_the_gate() {
        let now = Utc::now();
        assert!(time_gate_at(now - Duration::minutes(23 * 60 + 59), now).passed);
        assert!(time_gate_at(now - Duration::hours(24), now).passed);
        assert!(time_gate_at(now - Duration::hours(23), now).passed);
    }

    #[test]
    fn a_second_run_the_same_day_is_blocked() {
        let now = Utc::now();
        let g = time_gate_at(now - Duration::hours(22), now);
        assert!(!g.passed);
        assert_eq!(g.reason.as_deref(), Some("time gate: 22h00m elapsed, 23h required"));
        assert!(!time_gate_at(now - Duration::minutes(5), now).passed);
    }
}

/// Check session gate: 5+ new sessions since last dream
pub fn check_session_gate(new_session_count: usize) -> GateResult {
    if new_session_count >= MINIMUM_NEW_SESSIONS {
        GateResult {
            passed: true,
            reason: None,
        }
    } else {
        GateResult {
            passed: false,
            reason: Some(format!(
                "session gate: {} new sessions, {} required",
                new_session_count, MINIMUM_NEW_SESSIONS
            )),
        }
    }
}

/// Acquire lock, returns a guard that releases on drop
pub fn acquire_lock() -> Result<LockGuard> {
    let path = lock_path()?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Check existing lock
    if path.exists() {
        let content = fs::read_to_string(&path).unwrap_or_default();
        let parts: Vec<&str> = content.split('\n').collect();
        if let Some(pid_str) = parts.first() {
            if let Ok(pid) = pid_str.trim().parse::<u32>() {
                // Check if process is still running
                let is_running = pid_is_running(pid);
                if is_running {
                    // Check if stale
                    if let Ok(meta) = fs::metadata(&path) {
                        if let Ok(modified) = meta.modified() {
                            let age = std::time::SystemTime::now()
                                .duration_since(modified)
                                .unwrap_or_default();
                            if age.as_secs() > LOCK_STALE_MINUTES * 60 {
                                eprintln!(
                                    "Warning: removing stale lock (PID {} held for {}min)",
                                    pid,
                                    age.as_secs() / 60
                                );
                                fs::remove_file(&path)?;
                            } else {
                                anyhow::bail!(
                                    "Lock held by PID {} for {}s. Use --force to override.",
                                    pid,
                                    age.as_secs()
                                );
                            }
                        }
                    }
                } else {
                    // PID is dead, remove stale lock
                    fs::remove_file(&path)?;
                }
            }
        }
    }

    // Write our PID
    let pid = std::process::id();
    fs::write(&path, format!("{}\n", pid))?;
    Ok(LockGuard { path })
}

/// Check if a PID is running without libc dependency
fn pid_is_running(_pid: u32) -> bool {
    // Use kill(pid, 0) via Command to check if process exists
    std::process::Command::new("kill")
        .args(["-0", &_pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
