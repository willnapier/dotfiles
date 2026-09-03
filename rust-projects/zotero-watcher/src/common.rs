//! Shared plumbing: home, PID lock, Logger, Heartbeat, notifications, file moves.

use anyhow::{Context, Result};
use chrono::{DateTime, Local, SecondsFormat};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

pub fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file())).unwrap_or(false)
}

pub fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .or_else(|| Command::new("hostname").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()))
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

// ── lock ────────────────────────────────────────────────────────────
/// PID lock; stale iff the recorded PID is dead (a SIGKILL never runs cleanup).
pub fn take_lock(sub: &str, logger: &Logger) -> Result<()> {
    let lock = format!("/tmp/zotero-watcher-{sub}.lock");
    if let Ok(s) = std::fs::read_to_string(&lock) {
        if let Ok(pid) = s.trim().parse::<u32>() {
            if pid != std::process::id() && pid_alive(pid) {
                logger.log(&format!("❌ already running — pid {pid}"));
                std::process::exit(1);
            }
            logger.log(&format!("Removing stale lock file — pid {pid} not running"));
        }
    }
    std::fs::write(&lock, std::process::id().to_string()).with_context(|| format!("writing {lock}"))
}
fn pid_alive(pid: u32) -> bool {
    Command::new("kill").args(["-0", &pid.to_string()]).output().map(|o| o.status.success()).unwrap_or(false)
}

// ── logger ──────────────────────────────────────────────────────────
pub struct Logger {
    path: PathBuf,
}
impl Logger {
    pub fn new(path: PathBuf) -> Self {
        Logger { path }
    }
    pub fn log(&self, msg: &str) {
        let line = format!("[{}] {msg}", Local::now().format("%Y-%m-%d %H:%M:%S"));
        println!("{line}");
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{line}");
        }
    }
}

// ── heartbeat ───────────────────────────────────────────────────────
/// `{"watcher","version","started_at","last_cycle","last_action","actions","last_error","host","interval_secs"}`,
/// written atomically (tmp + rename) so a monitor never reads a torn file.
/// Written once at startup, then after every handled event. `interval_secs`
/// is the poll period, or 0 for an event-driven watcher (the health check
/// skips staleness for 0).
pub struct Heartbeat {
    path: PathBuf,
    watcher: String,
    started_at: DateTime<Local>,
    last_cycle: DateTime<Local>,
    last_action: Option<DateTime<Local>>,
    pub actions: u64,
    last_error: Option<String>,
    host: String,
    interval_secs: u64,
}
impl Heartbeat {
    pub fn new(state_dir: &Path, sub: &str, interval_secs: u64) -> Self {
        let now = Local::now();
        Heartbeat {
            path: state_dir.join(format!("zotero-watcher-{sub}.json")),
            watcher: format!("zotero-watcher-{sub}"),
            started_at: now,
            last_cycle: now,
            last_action: None,
            actions: 0,
            last_error: None,
            host: hostname(),
            interval_secs,
        }
    }
    /// An event was handled (whether or not it led to an action).
    pub fn cycle(&mut self) {
        self.last_cycle = Local::now();
    }
    /// A PDF was imported / a file was bridged.
    pub fn action(&mut self) {
        self.last_action = Some(Local::now());
        self.actions += 1;
    }
    pub fn error(&mut self, e: &str) {
        self.last_error = Some(e.to_string());
    }
    pub fn write(&self) {
        if let Err(e) = self.try_write() {
            eprintln!("heartbeat write failed: {e:#}");
        }
    }
    fn try_write(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, self.to_json())?;
        std::fs::rename(&tmp, &self.path).with_context(|| format!("renaming {} → {}", tmp.display(), self.path.display()))
    }
    pub fn to_json(&self) -> String {
        let ts = |t: &DateTime<Local>| json_str(&t.to_rfc3339_opts(SecondsFormat::Secs, true));
        let opt = |v: Option<String>| v.unwrap_or_else(|| "null".into());
        format!(
            "{{\"watcher\":{},\"version\":{},\"started_at\":{},\"last_cycle\":{},\"last_action\":{},\"actions\":{},\"last_error\":{},\"host\":{},\"interval_secs\":{}}}\n",
            json_str(&self.watcher),
            json_str(env!("CARGO_PKG_VERSION")),
            ts(&self.started_at),
            ts(&self.last_cycle),
            opt(self.last_action.as_ref().map(ts)),
            self.actions,
            opt(self.last_error.as_deref().map(json_str)),
            json_str(&self.host),
            self.interval_secs,
        )
    }
}

pub fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

// ── notifications (best effort) ─────────────────────────────────────
/// terminal-notifier, else notify-send, else nothing. Returns the method used.
pub fn notify(title: &str, message: &str) -> Option<&'static str> {
    if on_path("terminal-notifier") {
        let ok = Command::new("terminal-notifier").args(["-title", title, "-message", message]).output().map(|o| o.status.success()).unwrap_or(false);
        return ok.then_some("terminal-notifier");
    }
    if on_path("notify-send") {
        let ok = Command::new("notify-send").args([title, message]).output().map(|o| o.status.success()).unwrap_or(false);
        return ok.then_some("notify-send");
    }
    None
}

// ── file moves ──────────────────────────────────────────────────────
#[derive(Debug, PartialEq)]
pub enum Moved {
    Renamed,
    /// rename failed (message kept), so the file was copied, size-verified and the source removed
    Copied(String),
}

#[derive(Debug)]
pub enum MoveError {
    /// rename and copy both failed; (rename error, copy error)
    CopyFailed(String, String),
    /// rename failed, the copy landed but sizes differ; both files kept
    VerifyFailed(String),
}
impl std::fmt::Display for MoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MoveError::CopyFailed(r, c) => write!(f, "move failed ({r}); copy also failed ({c})"),
            MoveError::VerifyFailed(r) => write!(f, "move failed ({r}); copy verification failed - keeping both files"),
        }
    }
}
impl std::error::Error for MoveError {}

/// Move like Nushell's `mv`: rename, and when that is refused (cross-device,
/// "Operation not permitted" on a File Provider mount) copy, verify the size
/// and remove the source.
pub fn move_file(src: &Path, dst: &Path) -> std::result::Result<Moved, MoveError> {
    let rename_err = match std::fs::rename(src, dst) {
        Ok(()) => return Ok(Moved::Renamed),
        Err(e) => e.to_string(),
    };
    if let Err(c) = std::fs::copy(src, dst) {
        return Err(MoveError::CopyFailed(rename_err, c.to_string()));
    }
    let same = match (std::fs::metadata(src), std::fs::metadata(dst)) {
        (Ok(a), Ok(b)) => a.len() == b.len(),
        _ => false,
    };
    if !same {
        return Err(MoveError::VerifyFailed(rename_err));
    }
    if let Err(e) = std::fs::remove_file(src) {
        return Err(MoveError::CopyFailed(rename_err, format!("removing source: {e}")));
    }
    Ok(Moved::Copied(rename_err))
}

/// The file name for log lines, notifications and outcomes — NFC, so both
/// hosts print the same bytes for the same file (`forge-names` boundary).
/// Never join this back into a path: a move must land under the bytes the OS
/// gave us, which on macOS may be NFD. Join sites use `Path::file_name()`.
pub fn display_name(p: &Path) -> String {
    forge_names::file_name(p)
}

pub fn is_pdf(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("pdf")).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_json_shape_and_atomic_write() {
        let d = tempfile::tempdir().unwrap();
        let state = d.path().join("watchers");
        let mut hb = Heartbeat::new(&state, "pdf", 0);
        hb.write();
        let s = std::fs::read_to_string(state.join("zotero-watcher-pdf.json")).unwrap();
        assert!(s.starts_with("{\"watcher\":\"zotero-watcher-pdf\",\"version\":\""), "{s}");
        assert!(s.contains("\"last_action\":null,\"actions\":0,\"last_error\":null,\"host\":\""), "{s}");
        assert!(s.trim_end().ends_with("\",\"interval_secs\":0}"), "{s}");
        assert_eq!(Heartbeat::new(&state, "bridge", 30).to_json().trim_end().rsplit(',').next().unwrap(), "\"interval_secs\":30}");
        assert!(!state.join("zotero-watcher-pdf.json.tmp").exists());
        hb.action();
        hb.error("boom \"quoted\"");
        hb.write();
        let s = std::fs::read_to_string(state.join("zotero-watcher-pdf.json")).unwrap();
        assert!(s.contains("\"actions\":1,\"last_error\":\"boom \\\"quoted\\\"\""), "{s}");
        assert!(s.contains("\"last_action\":\"2"), "{s}");
    }

    #[test]
    fn move_file_renames_within_a_volume() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.pdf");
        let b = d.path().join("sub").join("b.pdf");
        std::fs::create_dir_all(b.parent().unwrap()).unwrap();
        std::fs::write(&a, b"x").unwrap();
        assert_eq!(move_file(&a, &b).unwrap(), Moved::Renamed);
        assert!(!a.exists() && b.exists());
        assert!(matches!(move_file(&a, &b), Err(MoveError::CopyFailed(..))));
    }

    #[test]
    fn pdf_extension_is_case_insensitive() {
        assert!(is_pdf(Path::new("/x/a.pdf")));
        assert!(is_pdf(Path::new("/x/a.PDF")));
        assert!(!is_pdf(Path::new("/x/a.pdf.part")));
        assert!(!is_pdf(Path::new("/x/notes.txt")));
    }
}
