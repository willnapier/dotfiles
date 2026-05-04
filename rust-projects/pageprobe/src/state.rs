//! Persistent state for `pageprobe` — tracks the debug-Chrome PID, port,
//! and currently-attached tab id between subcommand invocations.
//!
//! Stored at `~/.config/pageprobe/state.json` (or platform equivalent via
//! the `directories` crate). State is per-machine and not synced.
use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_PORT: u16 = 9222;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct State {
    /// PID of the spawned debug-Chrome process, if any.
    pub chrome_pid: Option<u32>,
    /// Remote-debugging port that Chrome is listening on.
    pub port: Option<u16>,
    /// CDP target id of the currently-attached tab, if any.
    pub attached_tab_id: Option<String>,
    /// Path to the user-data-dir Chrome was launched with.
    pub user_data_dir: Option<PathBuf>,
}

impl State {
    /// Returns the resolved port, falling back to the default.
    pub fn port_or_default(&self) -> u16 {
        self.port.unwrap_or(DEFAULT_PORT)
    }
}

/// Returns the configuration directory for `pageprobe`.
/// Creates it if it does not exist.
pub fn config_dir() -> Result<PathBuf> {
    let proj = ProjectDirs::from("", "", "pageprobe")
        .context("could not resolve user config directory")?;
    let dir = proj.config_dir().to_path_buf();
    fs::create_dir_all(&dir)
        .with_context(|| format!("creating config dir {}", dir.display()))?;
    Ok(dir)
}

/// Returns the path to the state file (creating the parent dir if needed).
pub fn state_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("state.json"))
}

/// Returns the default user-data-dir path used when `pageprobe start`
/// is invoked without an explicit `--user-data-dir`.
pub fn default_user_data_dir() -> Result<PathBuf> {
    Ok(config_dir()?.join("chrome-profile"))
}

/// Loads the state file. Returns `State::default()` if the file is missing.
pub fn load() -> Result<State> {
    let path = state_path()?;
    load_from(&path)
}

/// Loads state from an explicit path (for testability).
pub fn load_from(path: &Path) -> Result<State> {
    if !path.exists() {
        return Ok(State::default());
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("reading state file {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(State::default());
    }
    let state: State = serde_json::from_str(&raw)
        .with_context(|| format!("parsing state file {}", path.display()))?;
    Ok(state)
}

/// Saves state to disk.
pub fn save(state: &State) -> Result<()> {
    let path = state_path()?;
    save_to(&path, state)
}

/// Saves state to an explicit path (for testability).
pub fn save_to(path: &Path, state: &State) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating state dir {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(state)?;
    fs::write(path, raw)
        .with_context(|| format!("writing state file {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn round_trip_state() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");
        let s = State {
            chrome_pid: Some(12345),
            port: Some(9222),
            attached_tab_id: Some("abcd1234".into()),
            user_data_dir: Some(PathBuf::from("/tmp/profile")),
        };
        save_to(&path, &s).unwrap();
        let loaded = load_from(&path).unwrap();
        assert_eq!(s, loaded);
    }

    #[test]
    fn load_missing_returns_default() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nope.json");
        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded, State::default());
    }

    #[test]
    fn load_empty_returns_default() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("empty.json");
        std::fs::write(&path, "").unwrap();
        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded, State::default());
    }

    #[test]
    fn port_or_default_uses_fallback() {
        let s = State::default();
        assert_eq!(s.port_or_default(), 9222);
        let s2 = State {
            port: Some(9333),
            ..s
        };
        assert_eq!(s2.port_or_default(), 9333);
    }
}
