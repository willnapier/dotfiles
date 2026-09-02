//! collect-projects-watcher — near-instant entry collection (projects +
//! activities) whenever a DayPage changes.
//!
//! Rust port 2026-09-02 of the Nushell script `scripts/collect-projects-watcher`,
//! which shelled out to `fswatch` (macOS) or `inotifywait` (Linux). The script
//! ran `collect-projects-auto`, a wrapper whose only job was
//! `^collect-entries out+err> /dev/null` inside a `try` — so this binary runs
//! `collect-entries` (no arguments) directly and, unlike the wrapper, lets a
//! non-zero exit reach the log as `Collection failed: …` and the messageboard.
//!
//! Debounce: the script suppressed repeat events for the same file for 2 s
//! after a collection (fswatch itself batched with `-l 2.0`). Here a burst of
//! events settles for `--debounce-ms` (default 2000) and produces exactly one
//! collection, whichever files were touched.
//!
//! Log phrases kept verbatim, in the script's `<ts> - <msg>` layout:
//! "Change detected: <path>", "Collection completed", "Collection failed: <stderr>".

use anyhow::{bail, Context, Result};
use clap::Parser;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::{Duration, Instant};

const NAME: &str = "collect-projects-watcher";
const LOCK: &str = "/tmp/collect-projects-watcher.lock";

#[derive(Parser, Debug)]
#[command(name = NAME, version, about = "Runs collect-entries whenever a DayPage .md changes")]
struct Cli {
    /// Show detailed output (startup banner, "Collection completed")
    #[arg(long, short)]
    verbose: bool,
    /// Directory to watch (recursively) for .md changes
    #[arg(long, default_value_os_t = default_watch_dir())]
    watch_dir: PathBuf,
    /// Log file (appended)
    #[arg(long, default_value_os_t = default_log())]
    log: PathBuf,
    /// Command run (no arguments) after each settled change
    #[arg(long, default_value = "collect-entries")]
    collector: String,
    /// Milliseconds a burst of changes must settle before one collection runs
    #[arg(long, default_value_t = 2000)]
    debounce_ms: u64,
    /// Directory for the heartbeat file `collect-projects-watcher.json`
    #[arg(long, default_value_os_t = default_state_dir())]
    state_dir: PathBuf,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}
fn default_watch_dir() -> PathBuf {
    home().join("Forge/NapierianLogs/DayPages")
}
fn default_log() -> PathBuf {
    home().join(".local/share/collect-entries-watcher.log")
}
fn default_state_dir() -> PathBuf {
    home().join(".local/state/watchers")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let logger = Logger { path: Some(cli.log.clone()) };
    if let Some(dir) = cli.log.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    if !cli.watch_dir.exists() {
        println!("Error: Watch path not found: {}", cli.watch_dir.display());
        println!("Creating directory...");
        std::fs::create_dir_all(&cli.watch_dir).with_context(|| format!("creating {}", cli.watch_dir.display()))?;
    }
    take_lock(&logger)?;
    if cli.verbose {
        println!("Starting unified entry collection watcher (projects and activities)");
        println!("Platform: {}", std::env::consts::OS);
        println!("Watching: {}", cli.watch_dir.display());
        println!("Log file: {}", cli.log.display());
        println!();
    }
    let cfg = Config { watch_dir: cli.watch_dir, collector: cli.collector, debounce: Duration::from_millis(cli.debounce_ms), verbose: cli.verbose, notify: true };
    let mut hb = Heartbeat::new(cli.state_dir, NAME);
    let never = AtomicBool::new(false);
    run(&cfg, &logger, &mut hb, &never)
}

// ── lock ────────────────────────────────────────────────────────────
/// PID lock; stale iff the recorded PID is dead (a SIGKILL never runs cleanup).
fn take_lock(logger: &Logger) -> Result<()> {
    if let Ok(s) = std::fs::read_to_string(LOCK) {
        if let Ok(pid) = s.trim().parse::<u32>() {
            if pid != std::process::id() && pid_alive(pid) {
                logger.log(&format!("❌ already running — pid {pid}"));
                std::process::exit(1);
            }
            logger.log(&format!("Removing stale lock file — pid {pid} not running"));
        }
    }
    std::fs::write(LOCK, std::process::id().to_string()).with_context(|| format!("writing {LOCK}"))
}
fn pid_alive(pid: u32) -> bool {
    Command::new("kill").args(["-0", &pid.to_string()]).output().map(|o| o.status.success()).unwrap_or(false)
}

// ── core ────────────────────────────────────────────────────────────
pub struct Config {
    pub watch_dir: PathBuf,
    pub collector: String,
    pub debounce: Duration,
    pub verbose: bool,
    /// Send a messageboard notice on failure (off in tests)
    pub notify: bool,
}

/// fswatch `--event Updated` on macOS; modify / close_write / moved_to on Linux.
fn wanted(kind: &EventKind) -> bool {
    use notify::event::{AccessKind, AccessMode, ModifyKind, RenameMode};
    matches!(
        kind,
        EventKind::Create(_)
            | EventKind::Modify(ModifyKind::Data(_))
            | EventKind::Modify(ModifyKind::Any)
            | EventKind::Modify(ModifyKind::Name(RenameMode::To))
            | EventKind::Modify(ModifyKind::Name(RenameMode::Both))
            | EventKind::Modify(ModifyKind::Name(RenameMode::Any))
            | EventKind::Access(AccessKind::Close(AccessMode::Write))
    )
}

fn is_md(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("md")
}

/// Watch loop. Returns when `stop` is set (tests); the daemon never sets it.
pub fn run(cfg: &Config, logger: &Logger, hb: &mut Heartbeat, stop: &AtomicBool) -> Result<()> {
    let (tx, rx) = channel();
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            if let Ok(ev) = res {
                let _ = tx.send(ev);
            }
        },
        notify::Config::default(),
    )
    .context("creating watcher")?;
    watcher.watch(&cfg.watch_dir, RecursiveMode::Recursive).with_context(|| format!("watching {}", cfg.watch_dir.display()))?;
    hb.write().ok();

    let mut pending: BTreeSet<PathBuf> = BTreeSet::new();
    let mut last_event: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(ev) if wanted(&ev.kind) => {
                let md: Vec<PathBuf> = ev.paths.into_iter().filter(|p| is_md(p)).collect();
                if !md.is_empty() {
                    pending.extend(md);
                    last_event = Some(Instant::now());
                }
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => bail!("watcher channel closed"),
        }
        let settled = matches!(last_event, Some(t) if t.elapsed() >= cfg.debounce) && !pending.is_empty();
        if settled {
            let paths: Vec<PathBuf> = std::mem::take(&mut pending).into_iter().collect();
            last_event = None;
            hb.cycle();
            for p in &paths {
                logger.log(&format!("Change detected: {}", p.display()));
            }
            match collect(cfg) {
                Ok(()) => {
                    hb.action();
                    if cfg.verbose {
                        logger.log("Collection completed");
                    }
                }
                Err(e) => {
                    let msg = format!("Collection failed: {e}");
                    logger.log(&msg);
                    hb.error(msg);
                    if cfg.notify && on_path("messageboard-edit") {
                        let _ = Command::new("messageboard-edit").args(["insert", &format!("collect-entries FAILED on {}", hostname())]).output();
                    }
                }
            }
            if let Err(e) = hb.write() {
                logger.log(&format!("heartbeat write failed: {e:#}"));
            }
        }
    }
    Ok(())
}

/// One collection: the collector with no arguments; non-zero exit is an
/// error carrying its stderr (the script's `Collection failed: <stderr>`).
fn collect(cfg: &Config) -> Result<()> {
    let out = Command::new(&cfg.collector).output().with_context(|| format!("running {}", cfg.collector))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    bail!("{err}")
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file())).unwrap_or(false)
}

// ── heartbeat ───────────────────────────────────────────────────────
/// `<state_dir>/<name>.json`, written atomically (tmp + rename) after every
/// handled event. `last_error` is cleared by the next successful action.
pub struct Heartbeat {
    dir: PathBuf,
    name: &'static str,
    started_at: chrono::DateTime<chrono::Local>,
    last_cycle: Option<chrono::DateTime<chrono::Local>>,
    last_action: Option<chrono::DateTime<chrono::Local>>,
    actions: u64,
    last_error: Option<String>,
    host: String,
}
impl Heartbeat {
    pub fn new(dir: PathBuf, name: &'static str) -> Self {
        Heartbeat { dir, name, started_at: chrono::Local::now(), last_cycle: None, last_action: None, actions: 0, last_error: None, host: hostname() }
    }
    pub fn cycle(&mut self) {
        self.last_cycle = Some(chrono::Local::now());
    }
    pub fn action(&mut self) {
        self.actions += 1;
        self.last_action = Some(chrono::Local::now());
        self.last_error = None;
    }
    pub fn error(&mut self, e: String) {
        self.last_error = Some(e);
    }
    pub fn path(&self) -> PathBuf {
        self.dir.join(format!("{}.json", self.name))
    }
    pub fn to_json(&self) -> String {
        let t = |o: &Option<chrono::DateTime<chrono::Local>>| o.map(|d| format!("\"{}\"", d.to_rfc3339())).unwrap_or_else(|| "null".into());
        format!(
            "{{\"watcher\":{},\"version\":{},\"started_at\":\"{}\",\"last_cycle\":{},\"last_action\":{},\"actions\":{},\"last_error\":{},\"host\":{}}}\n",
            json_str(self.name),
            json_str(env!("CARGO_PKG_VERSION")),
            self.started_at.to_rfc3339(),
            t(&self.last_cycle),
            t(&self.last_action),
            self.actions,
            self.last_error.as_deref().map(json_str).unwrap_or_else(|| "null".into()),
            json_str(&self.host),
        )
    }
    pub fn write(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir).with_context(|| format!("creating {}", self.dir.display()))?;
        let dest = self.path();
        let tmp = self.dir.join(format!(".{}.json.{}.tmp", self.name, std::process::id()));
        std::fs::write(&tmp, self.to_json()).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &dest).with_context(|| format!("renaming to {}", dest.display()))
    }
}
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
fn hostname() -> String {
    Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

// ── logger ──────────────────────────────────────────────────────────
/// Prints and appends, in the script's `YYYY-mm-dd HH:MM:SS - msg` layout.
pub struct Logger {
    pub path: Option<PathBuf>,
}
impl Logger {
    fn log(&self, msg: &str) {
        let line = format!("{} - {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
        println!("{line}");
        if let Some(p) = &self.path {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                let _ = std::io::Write::write_all(&mut f, format!("{line}\n").as_bytes());
            }
        }
    }
}

// ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    const DEBOUNCE: Duration = Duration::from_millis(300);

    struct Rig {
        _d: tempfile::TempDir,
        watch: PathBuf,
        record: PathBuf,
        log: PathBuf,
        state: PathBuf,
        stop: Arc<AtomicBool>,
        handle: Option<std::thread::JoinHandle<Result<()>>>,
    }
    impl Rig {
        /// Watcher on a tempdir with a collector script that appends one
        /// line per run to `record`, exiting `exit_code`.
        fn start(exit_code: i32) -> Rig {
            let d = tempfile::tempdir().unwrap();
            let watch = d.path().join("DayPages");
            std::fs::create_dir_all(&watch).unwrap();
            let record = d.path().join("record");
            let script = d.path().join("collector.sh");
            std::fs::write(&script, format!("#!/bin/sh\necho run >> '{}'\necho boom >&2\nexit {exit_code}\n", record.display())).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            let log = d.path().join("watcher.log");
            let state = d.path().join("state");
            let stop = Arc::new(AtomicBool::new(false));
            let cfg = Config { watch_dir: watch.clone(), collector: script.to_string_lossy().into_owned(), debounce: DEBOUNCE, verbose: true, notify: false };
            let logger = Logger { path: Some(log.clone()) };
            let handle = {
                let (stop, state) = (stop.clone(), state.clone());
                std::thread::spawn(move || {
                    let mut hb = Heartbeat::new(state, NAME);
                    run(&cfg, &logger, &mut hb, &stop)
                })
            };
            std::thread::sleep(Duration::from_millis(500)); // watcher registration
            Rig { _d: d, watch, record, log, state, stop, handle: Some(handle) }
        }
        fn runs(&self) -> usize {
            std::fs::read_to_string(&self.record).map(|s| s.lines().count()).unwrap_or(0)
        }
        fn wait_for_runs(&self, n: usize) -> usize {
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline && self.runs() < n {
                std::thread::sleep(Duration::from_millis(50));
            }
            self.runs()
        }
        fn log(&self) -> String {
            std::fs::read_to_string(&self.log).unwrap_or_default()
        }
    }
    impl Drop for Rig {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(h) = self.handle.take() {
                h.join().unwrap().unwrap();
            }
        }
    }

    #[test]
    fn burst_of_writes_collects_once() {
        let rig = Rig::start(0);
        for i in 0..8 {
            std::fs::write(rig.watch.join("2026-09-02.md"), format!("line {i}\n")).unwrap();
            std::fs::write(rig.watch.join("2026-09-01.md"), format!("other {i}\n")).unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(rig.wait_for_runs(1), 1);
        // long past the debounce: still one run for the whole burst
        std::thread::sleep(DEBOUNCE * 4);
        assert_eq!(rig.runs(), 1);
        let log = rig.log();
        assert!(log.contains("- Change detected: ") && log.contains("2026-09-02.md"), "{log}");
        assert!(log.contains("- Collection completed"), "{log}");
        let hb = std::fs::read_to_string(rig.state.join("collect-projects-watcher.json")).unwrap();
        assert!(hb.contains("\"watcher\":\"collect-projects-watcher\"") && hb.contains("\"actions\":1,\"last_error\":null"), "{hb}");
    }

    #[test]
    fn non_md_change_is_ignored() {
        let rig = Rig::start(0);
        std::fs::write(rig.watch.join("notes.txt"), "x").unwrap();
        std::fs::write(rig.watch.join(".2026-09-02.md.swp"), "x").unwrap();
        std::thread::sleep(DEBOUNCE * 4 + Duration::from_millis(500));
        assert_eq!(rig.runs(), 0);
        assert!(!rig.log().contains("Change detected"), "{}", rig.log());
        // and an .md change afterwards still works
        std::fs::write(rig.watch.join("2026-09-02.md"), "x").unwrap();
        assert_eq!(rig.wait_for_runs(1), 1);
    }

    #[test]
    fn collector_failure_is_logged_and_watcher_survives() {
        let rig = Rig::start(1);
        std::fs::write(rig.watch.join("2026-09-02.md"), "one").unwrap();
        assert_eq!(rig.wait_for_runs(1), 1);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !rig.log().contains("Collection failed") {
            std::thread::sleep(Duration::from_millis(50));
        }
        let log = rig.log();
        assert!(log.contains("- Collection failed: boom"), "{log}");
        assert!(!log.contains("Collection completed"), "{log}");
        std::thread::sleep(DEBOUNCE * 2);
        std::fs::write(rig.watch.join("2026-09-02.md"), "two").unwrap();
        assert_eq!(rig.wait_for_runs(2), 2, "watcher died after the failure");
        let hb = std::fs::read_to_string(rig.state.join("collect-projects-watcher.json")).unwrap();
        assert!(hb.contains("\"actions\":0,\"last_error\":\"Collection failed: boom\""), "{hb}");
    }

    #[test]
    fn heartbeat_json_escapes_and_is_atomic() {
        let d = tempfile::tempdir().unwrap();
        let mut hb = Heartbeat::new(d.path().join("state"), NAME);
        hb.error("a \"b\"\n".into());
        hb.write().unwrap();
        let j = std::fs::read_to_string(hb.path()).unwrap();
        assert!(j.contains("\"last_error\":\"a \\\"b\\\"\\n\""), "{j}");
        assert!(j.contains("\"last_cycle\":null,\"last_action\":null,\"actions\":0"), "{j}");
        assert_eq!(std::fs::read_dir(d.path().join("state")).unwrap().count(), 1);
    }
}
