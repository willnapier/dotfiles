use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::fs;
use std::path::PathBuf;

use crate::types::DreamState;

const MINIMUM_HOURS: i64 = 24;
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
            let elapsed = Utc::now() - last_time;
            let hours = elapsed.num_hours();
            if hours >= MINIMUM_HOURS {
                GateResult {
                    passed: true,
                    reason: None,
                }
            } else {
                GateResult {
                    passed: false,
                    reason: Some(format!(
                        "time gate: {}h elapsed, {}h required",
                        hours, MINIMUM_HOURS
                    )),
                }
            }
        }
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
