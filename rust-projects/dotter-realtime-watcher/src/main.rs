//! dotter-realtime-watcher — recognition-only watcher for unmanaged configs.
//!
//! Rust port 2026-09-02 of `scripts/dotter-realtime-watcher-renu` (Nushell),
//! which was made recognition-only the same morning: this binary NEVER runs
//! `dotter-add` and never writes into `~/dotfiles`. It watches `~/.config`,
//! `~/.local/bin`, `~/.zshrc` and `~/.bashrc` with `notify`, debounces about
//! a second, and for each created/modified config-looking file that is not
//! referenced in `~/dotfiles/.dotter/global.toml` (plain substring check of
//! the `~`-relative path against the raw file text, exactly as the oracle)
//! logs `🆕 Unmanaged config candidate: <path> — run: dotter-add <path>` and
//! posts a best-effort desktop notification.
//!
//! Differences from the oracle: all four paths are watched (the oracle only
//! ever watched `~/.config`); `dotter-add` on PATH is no longer a startup
//! requirement; directories, `.git` contents and anything under `~/dotfiles`
//! are ignored; `--check <path>` replaces `--once` (there is no cycle to run
//! once); the heartbeat JSON in `~/.local/state/watchers/` is written at
//! startup and after every handled event.

mod heartbeat;

use anyhow::{Context, Result};
use clap::Parser;
use heartbeat::Heartbeat;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const NAME: &str = "dotter-realtime-watcher";

#[derive(Parser, Debug)]
#[command(name = NAME, version, about)]
struct Cli {
    /// Path to watch, repeatable; overrides the default list
    /// (~/.config, ~/.local/bin, ~/.zshrc, ~/.bashrc)
    #[arg(long = "watch", value_name = "PATH")]
    watch: Vec<PathBuf>,
    /// Dotter config consulted for "already managed"
    #[arg(long, default_value_os_t = default_dotter_config())]
    dotter_config: PathBuf,
    /// Milliseconds a path must be quiet before its event is handled
    #[arg(long, default_value_t = 1000)]
    debounce_ms: u64,
    /// Evaluate one path as if it had just changed, then exit
    /// (exit 0 = candidate reported, 1 = managed/ignored)
    #[arg(long, value_name = "PATH")]
    check: Option<PathBuf>,
    /// Log only; post no desktop notification
    #[arg(long)]
    dry_run: bool,
    /// Log file (appended)
    #[arg(long, default_value_os_t = default_log())]
    log: PathBuf,
    /// Directory for the heartbeat JSON (<state-dir>/dotter-realtime-watcher.json)
    #[arg(long, default_value_os_t = default_state_dir())]
    state_dir: PathBuf,
    /// PID lock file (not taken with --check)
    #[arg(long, default_value = "/tmp/dotter-realtime-watcher.lock")]
    lock: PathBuf,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}
fn default_dotter_config() -> PathBuf {
    home().join("dotfiles/.dotter/global.toml")
}
fn default_log() -> PathBuf {
    home().join(".local/share/dotter-realtime-watcher.log")
}
fn default_state_dir() -> PathBuf {
    home().join(".local/state/watchers")
}
fn default_watch_paths(home: &Path) -> Vec<PathBuf> {
    [".config", ".local/bin", ".zshrc", ".bashrc"].iter().map(|p| home.join(p)).collect()
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let logger = Logger { path: cli.log.clone() };
    if let Some(dir) = cli.log.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let ctx = Ctx { home: home(), dotter_config: cli.dotter_config.clone(), dry_run: cli.dry_run };
    // interval_secs 0 = event-driven; the health check skips staleness for it.
    let mut hb = Heartbeat::new(&cli.state_dir, NAME, env!("CARGO_PKG_VERSION"), 0);

    if let Some(path) = &cli.check {
        let reported = handle_event(path, "Check", &ctx, &logger);
        if reported {
            hb.record_action();
        }
        hb.write().ok();
        std::process::exit(if reported { 0 } else { 1 });
    }

    take_lock(&cli.lock, &logger)?;
    if let Err(e) = hb.write() {
        logger.log(&format!("❌ heartbeat write failed: {e:#}"));
    }
    let paths = if cli.watch.is_empty() { default_watch_paths(&ctx.home) } else { cli.watch.clone() };
    logger.log(&format!("🚀 Starting real-time config watcher ({NAME} {}, heartbeat {})", env!("CARGO_PKG_VERSION"), hb.path().display()));
    logger.log(&format!("👀 Watching: {}", paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")));

    let result = watch_loop(&paths, Duration::from_millis(cli.debounce_ms), &ctx, &logger, &mut hb);
    if let Err(e) = &result {
        logger.log(&format!("❌ Watch failed: {e:#}"));
        hb.set_error(Some(format!("{e:#}")));
        hb.write().ok();
    }
    std::fs::remove_file(&cli.lock).ok();
    logger.log("🛑 Real-time watcher stopped");
    result
}

// ── lock ────────────────────────────────────────────────────────────
/// PID lock; stale iff the recorded PID is dead (a SIGKILL never runs cleanup).
fn take_lock(lock: &Path, logger: &Logger) -> Result<()> {
    if let Ok(s) = std::fs::read_to_string(lock) {
        if let Ok(pid) = s.trim().parse::<u32>() {
            if pid != std::process::id() && pid_alive(pid) {
                logger.log(&format!("🔒 Real-time watcher already running — pid {pid}"));
                std::process::exit(1);
            }
            logger.log(&format!("⚠️  Removing stale lock file — pid {pid} not running"));
        }
    }
    std::fs::write(lock, std::process::id().to_string()).with_context(|| format!("writing {}", lock.display()))
}
fn pid_alive(pid: u32) -> bool {
    Command::new("kill").args(["-0", &pid.to_string()]).output().map(|o| o.status.success()).unwrap_or(false)
}

// ── watch loop ──────────────────────────────────────────────────────
pub struct Ctx {
    pub home: PathBuf,
    pub dotter_config: PathBuf,
    pub dry_run: bool,
}

/// Runs until the watcher channel closes. Events are coalesced per path and
/// handled once the path has been quiet for `debounce` (the oracle's 1 s
/// debounce plus its 1 s "let file operations complete" sleep).
fn watch_loop(paths: &[PathBuf], debounce: Duration, ctx: &Ctx, logger: &Logger, hb: &mut Heartbeat) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<notify::Event>| {
            let _ = tx.send(res);
        },
        Config::default(),
    )
    .context("creating filesystem watcher")?;
    let mut watched = 0;
    for p in paths {
        if !p.exists() {
            logger.log(&format!("⚠️  Skipping missing watch path: {}", p.display()));
            continue;
        }
        let mode = if p.is_dir() { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
        match watcher.watch(p, mode) {
            Ok(()) => watched += 1,
            Err(e) => logger.log(&format!("⚠️  Cannot watch {}: {e}", p.display())),
        }
    }
    if watched == 0 {
        anyhow::bail!("no watchable paths");
    }
    logger.log(&format!("⚡ Monitoring active on {watched} path(s), debounce {} ms", debounce.as_millis()));

    let mut pending: HashMap<PathBuf, (Instant, &'static str)> = HashMap::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(event)) => {
                let label = match event.kind {
                    EventKind::Create(_) => "Create",
                    EventKind::Modify(_) => "Modify",
                    _ => continue, // renames-away, removes, access: ignored like the oracle
                };
                for p in event.paths {
                    pending.insert(p, (Instant::now(), label));
                }
            }
            Ok(Err(e)) => logger.log(&format!("⚠️  Watch error: {e}")),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => anyhow::bail!("watcher channel closed"),
        }
        let now = Instant::now();
        let due: Vec<PathBuf> = pending.iter().filter(|(_, (t, _))| now.duration_since(*t) >= debounce).map(|(p, _)| p.clone()).collect();
        for p in due {
            let (_, label) = pending.remove(&p).expect("just selected");
            if handle_event(&p, label, ctx, logger) {
                hb.record_action();
            }
            hb.set_error(None);
            if let Err(e) = hb.write() {
                logger.log(&format!("❌ heartbeat write failed: {e:#}"));
            }
        }
    }
}

/// One Create/Modify event for `path`. Returns true iff a candidate was reported.
pub fn handle_event(path: &Path, operation: &str, ctx: &Ctx, logger: &Logger) -> bool {
    if !should_monitor_file(path, &ctx.home) {
        return false;
    }
    logger.log(&format!("📝 Config change detected: {} ({operation})", path.display()));
    let toml = std::fs::read_to_string(&ctx.dotter_config).ok();
    match decide(path, &ctx.home, path.is_file(), toml.as_deref()) {
        Decision::Candidate => {
            // Recognition only — never write into ~/dotfiles unattended.
            logger.log(&format!("🆕 Unmanaged config candidate: {} — run: dotter-add {}", path.display(), path.display()));
            let filename = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            if ctx.dry_run {
                logger.log(&format!("(dry-run) would notify: Unmanaged config: {filename} — dotter-add to onboard"));
            } else {
                notify_desktop(&filename);
            }
            true
        }
        Decision::Managed | Decision::Missing => {
            logger.log("ℹ️  File already managed or not suitable for onboarding");
            false
        }
    }
}

// ── pure core ───────────────────────────────────────────────────────
const CONFIG_EXTENSIONS: &[&str] = &["toml", "yml", "yaml", "json", "nu", "sh", "py", "lua", "js", "ts"];
const CONFIG_NAMES: &[&str] = &["config", "settings", "preferences"];
const TEMP_EXTENSIONS: &[&str] = &["tmp", "temp", "lock", "pid", "log", "cache"];
const DOT_TEMP_EXTENSIONS: &[&str] = &["tmp", "swp", "bak"];

/// Port of the oracle's `should_monitor_file`, plus: nothing under
/// `<home>/dotfiles` (the managed side), nothing inside a `.git` directory.
pub fn should_monitor_file(path: &Path, home: &Path) -> bool {
    if path.starts_with(home.join("dotfiles")) {
        return false;
    }
    if path.components().any(|c| c.as_os_str() == ".git") {
        return false;
    }
    let filename = match path.file_name().and_then(|f| f.to_str()) {
        Some(f) => f,
        None => return false,
    };
    // `path parse` extension: text after the last dot, but only if the name
    // has a dot after its first character (".zshrc" has no extension).
    let extension = filename[1..].rsplit_once('.').map(|(_, e)| e).unwrap_or("");

    if TEMP_EXTENSIONS.contains(&extension) {
        return false;
    }
    if filename.starts_with('.') && DOT_TEMP_EXTENSIONS.contains(&extension) {
        return false;
    }
    if filename == ".DS_Store" {
        return false;
    }
    if CONFIG_EXTENSIONS.contains(&extension) {
        return true;
    }
    if CONFIG_NAMES.contains(&filename) {
        return true;
    }
    // Executable-ish files in .local/bin (the oracle only checked existence).
    if path.to_string_lossy().contains(".local/bin/") && path.is_file() {
        return true;
    }
    false
}

/// `<home>` prefix replaced by `~`, once, as the oracle's `str replace` did.
pub fn tilde_path(path: &Path, home: &Path) -> String {
    let p = path.to_string_lossy();
    let h = home.to_string_lossy();
    if h.is_empty() || h == "/" {
        return p.into_owned();
    }
    p.replacen(h.as_ref(), "~", 1)
}

#[derive(Debug, PartialEq)]
pub enum Decision {
    /// exists and is not referenced in global.toml
    Candidate,
    /// the `~`-relative path appears in global.toml
    Managed,
    /// no longer on disk (or not a regular file)
    Missing,
}

/// Port of the oracle's `is_new_unmanaged_config`: a plain substring check of
/// the `~`-relative path against the raw toml text; no toml → unmanaged.
pub fn decide(path: &Path, home: &Path, exists: bool, dotter_toml: Option<&str>) -> Decision {
    if !exists {
        return Decision::Missing;
    }
    match dotter_toml {
        Some(text) if text.contains(&tilde_path(path, home)) => Decision::Managed,
        _ => Decision::Candidate,
    }
}

// ── desktop notification (best effort) ──────────────────────────────
fn notify_desktop(filename: &str) {
    let body = format!("Unmanaged config: {filename} — dotter-add to onboard");
    let title = "Dotter watcher";
    let _ = if cfg!(target_os = "macos") {
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        Command::new("osascript").args(["-e", &format!("display notification \"{}\" with title \"{}\"", esc(&body), esc(title))]).output()
    } else {
        Command::new("notify-send").args([title, &body]).output()
    };
}

// ── logger (oracle format: `YYYY-mm-dd HH:MM:SS - message`) ─────────
pub struct Logger {
    path: PathBuf,
}
impl Logger {
    fn log(&self, msg: &str) {
        let line = format!("{} - {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
        println!("{line}");
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{line}");
        }
    }
}

// ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/Users/w")
    }
    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn filter_accepts_config_extensions_and_names() {
        let h = home();
        for f in ["/Users/w/.config/helix/config.toml", "/Users/w/.config/x/settings.yml", "/Users/w/.config/y/a.yaml", "/Users/w/.config/z/a.json", "/Users/w/.config/nushell/env.nu", "/Users/w/.config/s.sh", "/Users/w/.config/s.py", "/Users/w/.config/nvim/init.lua", "/Users/w/.config/a.js", "/Users/w/.config/a.ts"] {
            assert!(should_monitor_file(&p(f), &h), "{f}");
        }
        for f in ["/Users/w/.config/ghostty/config", "/Users/w/.config/x/settings", "/Users/w/.config/x/preferences"] {
            assert!(should_monitor_file(&p(f), &h), "{f}");
        }
    }

    #[test]
    fn filter_skips_temp_lock_log_cache_ds_store_git_and_dotfiles() {
        let h = home();
        for f in [
            "/Users/w/.config/a.tmp",
            "/Users/w/.config/a.temp",
            "/Users/w/.config/a.lock",
            "/Users/w/.config/a.pid",
            "/Users/w/.config/a.log",
            "/Users/w/.config/a.cache",
            "/Users/w/.config/.config.toml.swp",
            "/Users/w/.config/.x.bak",
            "/Users/w/.config/.DS_Store",
            "/Users/w/.config/a.txt",
            "/Users/w/.config/nested/dir/binary",
            "/Users/w/.zshrc",
            "/Users/w/.config/repo/.git/config",
            "/Users/w/dotfiles/.config/helix/config.toml",
            "/Users/w/dotfiles/scripts/x.nu",
        ] {
            assert!(!should_monitor_file(&p(f), &h), "{f}");
        }
    }

    #[test]
    fn filter_accepts_existing_files_in_local_bin_only() {
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let tool = bin.join("my-tool");
        assert!(!should_monitor_file(&tool, d.path()), "missing file");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        assert!(should_monitor_file(&tool, d.path()));
        assert!(!should_monitor_file(&bin, d.path()), "the directory itself");
        assert!(!should_monitor_file(&bin.join("x.log"), d.path()));
    }

    #[test]
    fn tilde_path_replaces_home_once() {
        assert_eq!(tilde_path(&p("/Users/w/.config/x.toml"), &home()), "~/.config/x.toml");
        assert_eq!(tilde_path(&p("/Users/w/a/Users/w/b"), &home()), "~/a/Users/w/b");
        assert_eq!(tilde_path(&p("/tmp/x/y.toml"), &home()), "/tmp/x/y.toml");
        assert_eq!(tilde_path(&p("/a/b"), &p("/")), "/a/b");
    }

    #[test]
    fn decide_uses_plain_substring_match_against_raw_toml() {
        let toml = "[shared.files]\n\"config/helix/config.toml\" = \"~/.config/helix/config.toml\"\n\"scripts/tool\" = { target = \"~/.local/bin/tool\", type = \"symbolic\" }\n";
        let h = home();
        assert_eq!(decide(&p("/Users/w/.config/helix/config.toml"), &h, true, Some(toml)), Decision::Managed);
        assert_eq!(decide(&p("/Users/w/.local/bin/tool"), &h, true, Some(toml)), Decision::Managed);
        assert_eq!(decide(&p("/Users/w/.config/helix/languages.toml"), &h, true, Some(toml)), Decision::Candidate);
        assert_eq!(decide(&p("/Users/w/.config/helix/languages.toml"), &h, true, None), Decision::Candidate);
        assert_eq!(decide(&p("/Users/w/.config/helix/languages.toml"), &h, false, Some(toml)), Decision::Missing);
        // substring semantics, as the oracle: a longer managed path covers a prefix of itself
        assert_eq!(decide(&p("/Users/w/.config/helix/config"), &h, true, Some(toml)), Decision::Managed);
    }

    #[test]
    fn handle_event_logs_candidate_and_managed_lines() {
        let d = tempfile::tempdir().unwrap();
        let cfg = d.path().join(".config/app");
        std::fs::create_dir_all(&cfg).unwrap();
        let managed = cfg.join("managed.toml");
        let new = cfg.join("new.toml");
        std::fs::write(&managed, "").unwrap();
        std::fs::write(&new, "").unwrap();
        let toml_path = d.path().join("global.toml");
        std::fs::write(&toml_path, "\"x\" = \"~/.config/app/managed.toml\"\n").unwrap();
        let log = d.path().join("log");
        let logger = Logger { path: log.clone() };
        let ctx = Ctx { home: d.path().to_path_buf(), dotter_config: toml_path, dry_run: true };

        assert!(!handle_event(&managed, "Modify", &ctx, &logger));
        assert!(handle_event(&new, "Create", &ctx, &logger));
        assert!(!handle_event(&cfg.join("ignored.txt"), "Create", &ctx, &logger));
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains(&format!("📝 Config change detected: {} (Modify)", managed.display())), "{text}");
        assert!(text.contains("ℹ️  File already managed or not suitable for onboarding"), "{text}");
        assert!(text.contains(&format!("🆕 Unmanaged config candidate: {} — run: dotter-add {}", new.display(), new.display())), "{text}");
        assert!(text.contains("(dry-run) would notify: Unmanaged config: new.toml"), "{text}");
        assert!(!text.contains("ignored.txt"), "{text}");
    }

    #[test]
    fn stale_lock_is_replaced() {
        let d = tempfile::tempdir().unwrap();
        let logger = Logger { path: d.path().join("log") };
        let lock = d.path().join("lock");
        std::fs::write(&lock, "999999999").unwrap();
        take_lock(&lock, &logger).unwrap();
        assert_eq!(std::fs::read_to_string(&lock).unwrap(), std::process::id().to_string());
        assert!(std::fs::read_to_string(d.path().join("log")).unwrap().contains("⚠️  Removing stale lock file — pid 999999999 not running"));
    }

}
