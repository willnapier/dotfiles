//! Timestamped logger: stdout plus an appended log file (modelled on
//! `git-auto-push-watcher`). The oracle phrases (`🔍 Monitoring Forge for file
//! events...`, `📝 Modified:`, …) are kept verbatim after the timestamp.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct Logger {
    /// Appended to when set.
    pub file: Option<PathBuf>,
    /// Prefixed to stdout lines only (used when both watchers share one stdout).
    pub tag: Option<String>,
    /// Suppress stdout (tests).
    pub quiet: bool,
}

impl Logger {
    pub fn silent() -> Self {
        Logger { file: None, tag: None, quiet: true }
    }

    pub fn log(&self, msg: &str) {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        if !self.quiet {
            match &self.tag {
                Some(t) => println!("[{stamp}] [{t}] {msg}"),
                None => println!("[{stamp}] {msg}"),
            }
        }
        if let Some(path) = &self.file {
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(f, "[{stamp}] {msg}");
            }
        }
    }
}
