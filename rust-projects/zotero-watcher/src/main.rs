//! zotero-watcher — the two Zotero import watchers as one binary.
//!
//! Rust port 2026-09-02 of the Nushell scripts `zotero-pdf-watcher-renu`
//! (→ `zotero-watcher pdf`) and `zotero-bridge-renu` (→ `zotero-watcher
//! bridge`). Console phrases are kept verbatim so anything grepping the
//! journal keeps working; each module's header lists its deliberate
//! differences from the oracle.
//!
//! New in the port, shared by both subcommands:
//! - PID lock in /tmp, stale iff the recorded PID is dead.
//! - Logger: stdout plus an appended log file (`--log`).
//! - Heartbeat JSON at `~/.local/state/watchers/zotero-watcher-<sub>.json`,
//!   rewritten atomically after every handled event (`--state-dir`).

mod bridge;
mod common;
mod pdf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use common::{home, take_lock, Heartbeat, Logger};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "zotero-watcher", version, about)]
struct Cli {
    /// Directory for the heartbeat JSON (zotero-watcher-<sub>.json)
    #[arg(long, global = true, default_value_os_t = default_state_dir())]
    state_dir: PathBuf,
    /// Log file (appended); default ~/.local/share/zotero-watcher-<sub>.log
    #[arg(long, global = true)]
    log: Option<PathBuf>,
    /// Do not send desktop notifications (terminal-notifier / notify-send)
    #[arg(long, global = true)]
    no_notify: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Watch a directory for new PDFs and move each into the processed directory
    Pdf {
        /// Directory to watch for PDFs (symlinks are resolved before watching)
        #[arg(long, default_value_os_t = home().join("Documents/ZoteroImport"))]
        watch_dir: PathBuf,
        /// Where processed PDFs are moved
        #[arg(long, default_value_os_t = home().join("Documents/ProcessedPDFs"))]
        output_dir: PathBuf,
        /// Quiet time after the last write before a PDF is processed
        #[arg(long, default_value_t = 2000)]
        debounce_ms: u64,
        /// Command run after each move with the processed PDF path as its only argument
        #[arg(long)]
        import_cmd: Option<PathBuf>,
    },
    /// Poll a source directory and move its PDFs into the destination directory
    Bridge {
        /// Source directory
        #[arg(long, default_value_os_t = home().join("Library/CloudStorage/Dropbox/ZoteroImport"))]
        source: PathBuf,
        /// Destination directory
        #[arg(long, default_value_os_t = home().join("Documents/ZoteroImport"))]
        destination: PathBuf,
        /// Seconds between checks
        #[arg(long, default_value_t = 30)]
        interval: u64,
        /// Maximum files to process per check
        #[arg(long, default_value_t = 5)]
        batch_size: usize,
        /// Transfer every PDF once and exit (the old `sync-now`)
        #[arg(long)]
        once: bool,
    },
}

fn default_state_dir() -> PathBuf {
    home().join(".local/state/watchers")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let sub = match cli.cmd {
        Cmd::Pdf { .. } => "pdf",
        Cmd::Bridge { .. } => "bridge",
    };
    let log_path = cli.log.clone().unwrap_or_else(|| home().join(".local/share").join(format!("zotero-watcher-{sub}.log")));
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let logger = Logger::new(log_path);
    // interval_secs: 0 marks an event-driven watcher (health check skips staleness)
    let interval_secs = match cli.cmd {
        Cmd::Pdf { .. } => 0,
        Cmd::Bridge { interval, .. } => interval,
    };
    let mut hb = Heartbeat::new(&cli.state_dir, sub, interval_secs);

    let long_running = !matches!(cli.cmd, Cmd::Bridge { once: true, .. });
    if long_running {
        take_lock(sub, &logger)?;
    }

    match cli.cmd {
        Cmd::Pdf { watch_dir, output_dir, debounce_ms, import_cmd } => {
            let cfg = pdf::PdfCfg { watch_dir, output_dir, debounce: Duration::from_millis(debounce_ms), import_cmd, notify: !cli.no_notify };
            pdf::run(cfg, &logger, &mut hb)
        }
        Cmd::Bridge { source, destination, interval, batch_size, once } => {
            let cfg = bridge::BridgeCfg { source, destination, interval: Duration::from_secs(interval), batch_size, notify: !cli.no_notify };
            if once {
                bridge::sync_now(&cfg, &logger, &mut hb)
            } else {
                bridge::run(&cfg, &logger, &mut hb)
            }
        }
    }
}
