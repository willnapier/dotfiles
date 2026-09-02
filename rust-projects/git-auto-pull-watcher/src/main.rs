//! git-auto-pull-watcher — fast-forward local clones from origin/main.
//!
//! Rust port 2026-09-02 of three Nushell scripts: `git-auto-pull-watcher`
//! (Linux: ~/dotfiles with `dotter deploy`, plus ~/Assistants),
//! `git-auto-pull-watcher-macos` (Mac: ~/dotfiles) and
//! `assistants-auto-pull-macos` (Mac: ~/Assistants). One binary, one log,
//! one lock on both platforms. Behaviour changes from the oracles:
//!
//! 1. **`--ff-only`.** The oracles ran a plain `git pull`, which would merge
//!    (or on Mac, `--no-rebase` merge) a diverged branch unattended. A
//!    non-fast-forward is now a logged failure; nothing merges or rebases.
//! 2. **Dirty tree blocks the pull.** Modified tracked files mean someone is
//!    mid-edit; the repo is skipped and the skip is logged once per change of
//!    state, not every tick. Untracked files do not block (a stray file must
//!    not silence the watcher indefinitely; if a pull would overwrite one, git
//!    refuses and that is logged).
//! 3. **Heartbeat.** After every cycle `<state-dir>/git-auto-pull-watcher.json`
//!    is rewritten atomically for monitors.
//! 4. **Each log line once.** The Mac oracle wrote every line twice.
//!
//! Log phrases the monitors grep for are unchanged, now tagged `[label]`:
//! "📥 Remote changes detected: N commits behind", "✅ Successfully pulled
//! changes", "✅ Dotter deploy successful - configs updated",
//! "❌ Git pull failed:", "❌ Dotter deploy failed:", "❌ Git fetch failed:".

use anyhow::{Context, Result};
use chrono::SecondsFormat;
use clap::Parser;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "git-auto-pull-watcher", version, about)]
struct Cli {
    /// Repository to keep fast-forwarded (repeatable). Default: ~/dotfiles and ~/Assistants, whichever exist
    #[arg(long)]
    repo: Vec<PathBuf>,
    /// Repository that gets `dotter deploy` after a successful pull (repeatable). ~/dotfiles always does
    #[arg(long)]
    deploy: Vec<PathBuf>,
    /// Seconds between checks
    #[arg(long, default_value_t = 120)]
    tick: u64,
    /// Run one cycle and exit (exit 1 only if a pull or deploy failed)
    #[arg(long)]
    once: bool,
    /// Report what would happen; pull and deploy nothing
    #[arg(long)]
    dry_run: bool,
    /// Log file (appended)
    #[arg(long, default_value_os_t = default_log())]
    log: PathBuf,
    /// Directory for the heartbeat file git-auto-pull-watcher.json
    #[arg(long, default_value_os_t = default_state_dir())]
    state_dir: PathBuf,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}
fn default_log() -> PathBuf {
    home().join(".local/share/git-auto-pull-watcher.log")
}
fn default_state_dir() -> PathBuf {
    home().join(".local/state/watchers")
}
const LOCK: &str = "/tmp/git-auto-pull-watcher.lock";
const NAME: &str = "git-auto-pull-watcher";

fn main() -> Result<()> {
    let cli = Cli::parse();
    let logger = Logger { path: cli.log.clone() };
    if let Some(dir) = cli.log.parent() {
        std::fs::create_dir_all(dir).ok();
    }

    let repos = resolve_repos(&cli.repo, &cli.deploy);
    if repos.is_empty() {
        anyhow::bail!("no repositories to watch (none of the defaults exist; pass --repo)");
    }
    let mut watcher = Watcher::new(repos, cli.dry_run, cli.tick, cli.state_dir.clone(), logger);

    if !cli.once {
        take_lock(&watcher.logger)?;
        let list: Vec<String> = watcher.repos.iter().map(|r| if r.deploy { format!("{} (deploy)", r.label) } else { r.label.clone() }).collect();
        watcher.logger.log(&format!("🚀 Starting {NAME} {} — repos {}; tick {}s", env!("CARGO_PKG_VERSION"), list.join(", "), cli.tick));
    }
    // Heartbeat exists from startup, not only after the first tick.
    watcher.write_heartbeat();

    loop {
        if !cli.once {
            std::thread::sleep(Duration::from_secs(cli.tick));
        }
        let outcomes = watcher.cycle();
        if cli.once {
            if outcomes.iter().any(Outcome::is_failure) {
                std::process::exit(1);
            }
            return Ok(());
        }
    }
}

// ── repos ───────────────────────────────────────────────────────────
#[derive(Debug, Clone)]
pub struct Repo {
    pub path: PathBuf,
    pub label: String,
    pub deploy: bool,
}
impl Repo {
    pub fn new(path: impl Into<PathBuf>, deploy: bool) -> Self {
        let path: PathBuf = path.into();
        let label = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
        Repo { path, label, deploy }
    }
}

/// `--repo` list, or the defaults that exist; `--deploy` and ~/dotfiles get dotter.
fn resolve_repos(repos: &[PathBuf], deploy: &[PathBuf]) -> Vec<Repo> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let dotfiles = canon(&home().join("dotfiles"));
    let deploy: Vec<PathBuf> = deploy.iter().map(|p| canon(p)).collect();
    let wants_deploy = |p: &Path| {
        let c = canon(p);
        c == dotfiles || deploy.contains(&c)
    };
    if repos.is_empty() {
        return [home().join("dotfiles"), home().join("Assistants")].into_iter().filter(|p| p.is_dir()).map(|p| {
            let d = wants_deploy(&p);
            Repo::new(p, d)
        }).collect();
    }
    repos.iter().map(|p| Repo::new(p.clone(), wants_deploy(p))).collect()
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

// ── one cycle ───────────────────────────────────────────────────────
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// nothing behind origin/main
    UpToDate,
    /// behind, but modified tracked files present; not pulling
    Dirty { behind: u64 },
    DryRun { behind: u64 },
    /// HEAD moved; `deployed` is true when dotter ran and succeeded
    Pulled { behind: u64, deployed: bool },
    FetchFailed(String),
    PullFailed(String),
    /// pull succeeded (HEAD moved) but `dotter deploy` failed
    DeployFailed(String),
}
impl Outcome {
    /// What `--once` exits 1 for: a pull or deploy that failed.
    pub fn is_failure(&self) -> bool {
        matches!(self, Outcome::PullFailed(_) | Outcome::DeployFailed(_))
    }
    fn error(&self) -> Option<String> {
        match self {
            Outcome::FetchFailed(e) | Outcome::PullFailed(e) | Outcome::DeployFailed(e) => Some(e.clone()),
            _ => None,
        }
    }
}

pub struct Watcher {
    pub repos: Vec<Repo>,
    pub dry_run: bool,
    pub state_dir: PathBuf,
    pub logger: Logger,
    /// repos last seen dirty-and-behind, so the skip is logged once per state change
    dirty: HashSet<PathBuf>,
    heartbeat: Heartbeat,
}

impl Watcher {
    pub fn new(repos: Vec<Repo>, dry_run: bool, tick_secs: u64, state_dir: PathBuf, logger: Logger) -> Self {
        Watcher { repos, dry_run, state_dir, logger, dirty: HashSet::new(), heartbeat: Heartbeat::new(tick_secs) }
    }

    /// Stamp `last_cycle = now` and rewrite the heartbeat file; a failure is logged, never fatal.
    pub fn write_heartbeat(&mut self) {
        self.heartbeat.last_cycle = Some(now_rfc3339());
        if let Err(e) = self.heartbeat.write(&self.state_dir) {
            self.logger.log(&format!("❌ heartbeat write failed: {e:#}"));
        }
    }

    /// One pass over every repo, then the heartbeat. Never fails: a repo whose
    /// git could not even run is reported as `FetchFailed`.
    pub fn cycle(&mut self) -> Vec<Outcome> {
        let mut outcomes = Vec::with_capacity(self.repos.len());
        let mut last_error = None;
        for i in 0..self.repos.len() {
            let repo = self.repos[i].clone();
            let outcome = match self.pull_repo(&repo) {
                Ok(o) => o,
                Err(e) => {
                    let msg = format!("{e:#}");
                    self.logger.log(&format!("❌ [{}] Error: {msg}", repo.label));
                    Outcome::FetchFailed(msg)
                }
            };
            if let Some(e) = outcome.error() {
                last_error = Some(format!("[{}] {e}", repo.label));
            }
            if matches!(outcome, Outcome::Pulled { .. } | Outcome::DeployFailed(_)) {
                self.heartbeat.actions += 1;
                self.heartbeat.last_action = Some(now_rfc3339());
            }
            outcomes.push(outcome);
        }
        self.heartbeat.last_error = last_error;
        self.write_heartbeat();
        outcomes
    }

    fn pull_repo(&mut self, repo: &Repo) -> Result<Outcome> {
        let label = &repo.label;
        let dir = &repo.path;

        let fetch = git(dir, &["fetch", "-q", "origin", "main"])?;
        if !fetch.ok {
            let err = fetch.err.trim().to_string();
            self.logger.log(&format!("❌ [{label}] Git fetch failed: {err}"));
            return Ok(Outcome::FetchFailed(err));
        }
        let count = git(dir, &["rev-list", "--count", "HEAD..origin/main"])?;
        if !count.ok {
            anyhow::bail!("git rev-list failed: {}", count.err.trim());
        }
        let behind = parse_count(&count.out).context("unparseable rev-list count")?;
        if behind == 0 {
            return Ok(Outcome::UpToDate);
        }

        let status = git(dir, &["status", "--porcelain", "--untracked-files=no"])?;
        if !status.ok {
            anyhow::bail!("git status failed: {}", status.err.trim());
        }
        if porcelain_is_dirty(&status.out) {
            if self.dirty.insert(dir.clone()) {
                self.logger.log(&format!("⏸ [{label}] uncommitted changes present — not pulling"));
            }
            return Ok(Outcome::Dirty { behind });
        }
        if self.dirty.remove(dir) {
            self.logger.log(&format!("▶ [{label}] working tree clean — pulling resumed"));
        }

        self.logger.log(&format!("📥 [{label}] Remote changes detected: {behind} commits behind"));
        if self.dry_run {
            self.logger.log(&format!("(dry-run) [{label}] would pull {behind} commits{}", if repo.deploy { " and run dotter deploy" } else { "" }));
            return Ok(Outcome::DryRun { behind });
        }

        let before = git(dir, &["rev-parse", "HEAD"])?.out;
        let pull = git(dir, &["pull", "-q", "--ff-only", "origin", "main"])?;
        let after = git(dir, &["rev-parse", "HEAD"])?.out;
        if !pull.ok || before == after {
            let err = if pull.ok { "HEAD did not move".to_string() } else { pull.err.trim().to_string() };
            self.logger.log(&format!("❌ [{label}] Git pull failed: {err}"));
            return Ok(Outcome::PullFailed(err));
        }
        self.logger.log(&format!("✅ [{label}] Successfully pulled changes"));

        if !repo.deploy {
            return Ok(Outcome::Pulled { behind, deployed: false });
        }
        match Command::new("dotter").arg("deploy").current_dir(dir).output() {
            Ok(o) if o.status.success() => {
                self.logger.log(&format!("✅ [{label}] Dotter deploy successful - configs updated"));
                Ok(Outcome::Pulled { behind, deployed: true })
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                self.logger.log(&format!("❌ [{label}] Dotter deploy failed: {err}"));
                Ok(Outcome::DeployFailed(err))
            }
            Err(e) => {
                let err = format!("could not run dotter: {e}");
                self.logger.log(&format!("❌ [{label}] Dotter deploy failed: {err}"));
                Ok(Outcome::DeployFailed(err))
            }
        }
    }
}

// ── pieces (pure where possible, so they are testable) ──────────────
/// `git rev-list --count` output → number; None if git printed something odd.
pub fn parse_count(s: &str) -> Option<u64> {
    s.trim().parse().ok()
}

/// `git status --porcelain` has any entry at all.
pub fn porcelain_is_dirty(s: &str) -> bool {
    s.lines().any(|l| !l.trim().is_empty())
}

struct GitOut {
    ok: bool,
    out: String,
    err: String,
}
fn git(repo: &Path, args: &[&str]) -> Result<GitOut> {
    let o = Command::new("git").args(args).current_dir(repo).output().with_context(|| format!("running git {} in {}", args.join(" "), repo.display()))?;
    Ok(GitOut { ok: o.status.success(), out: String::from_utf8_lossy(&o.stdout).trim().to_string(), err: String::from_utf8_lossy(&o.stderr).into_owned() })
}

// ── heartbeat ───────────────────────────────────────────────────────
/// `<state-dir>/git-auto-pull-watcher.json`, rewritten atomically after every cycle.
struct Heartbeat {
    /// the tick; a health check judges staleness against it (0 would mean event-driven)
    interval_secs: u64,
    started_at: String,
    last_cycle: Option<String>,
    last_action: Option<String>,
    actions: u64,
    /// error from the most recent cycle; null when that cycle was clean
    last_error: Option<String>,
    host: String,
}
impl Heartbeat {
    fn new(interval_secs: u64) -> Self {
        Heartbeat { interval_secs, started_at: now_rfc3339(), last_cycle: None, last_action: None, actions: 0, last_error: None, host: hostname() }
    }
    fn to_json(&self) -> String {
        let opt = |v: &Option<String>| v.as_deref().map(json_str).unwrap_or_else(|| "null".into());
        format!(
            "{{\"watcher\":{},\"version\":{},\"interval_secs\":{},\"started_at\":{},\"last_cycle\":{},\"last_action\":{},\"actions\":{},\"last_error\":{},\"host\":{}}}\n",
            json_str(NAME),
            json_str(env!("CARGO_PKG_VERSION")),
            self.interval_secs,
            json_str(&self.started_at),
            opt(&self.last_cycle),
            opt(&self.last_action),
            self.actions,
            opt(&self.last_error),
            json_str(&self.host),
        )
    }
    fn write(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join(format!("{NAME}.json"));
        let tmp = dir.join(format!(".{NAME}.json.{}", std::process::id()));
        std::fs::write(&tmp, self.to_json()).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("renaming to {}", path.display()))
    }
}

pub fn json_str(s: &str) -> String {
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

fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339_opts(SecondsFormat::Secs, false)
}

fn hostname() -> String {
    Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .and_then(|h| h.trim().split('.').next().map(str::to_string))
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

pub struct Logger {
    pub path: PathBuf,
}
impl Logger {
    fn log(&self, msg: &str) {
        let line = format!("[{}] {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
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

    #[test]
    fn rev_list_count_and_porcelain_parsing() {
        assert_eq!(parse_count("3\n"), Some(3));
        assert_eq!(parse_count("  0  "), Some(0));
        assert_eq!(parse_count(""), None);
        assert_eq!(parse_count("fatal: bad revision"), None);
        assert!(!porcelain_is_dirty(""));
        assert!(!porcelain_is_dirty("\n"));
        assert!(porcelain_is_dirty(" M scripts/a\n"));
        assert!(porcelain_is_dirty("R  old -> new\n"));
    }

    #[test]
    fn json_strings_are_escaped() {
        assert_eq!(json_str("plain"), "\"plain\"");
        assert_eq!(json_str("a \"q\" \\ b\nc"), "\"a \\\"q\\\" \\\\ b\\nc\"");
        let hb = Heartbeat { interval_secs: 120, started_at: "t0".into(), last_cycle: None, last_action: None, actions: 0, last_error: None, host: "h".into() };
        assert_eq!(hb.to_json(), format!("{{\"watcher\":\"git-auto-pull-watcher\",\"version\":\"{}\",\"interval_secs\":120,\"started_at\":\"t0\",\"last_cycle\":null,\"last_action\":null,\"actions\":0,\"last_error\":null,\"host\":\"h\"}}\n", env!("CARGO_PKG_VERSION")));
    }

    /// The file exists from startup, before any cycle, with last_cycle stamped.
    #[test]
    fn heartbeat_is_written_at_startup() {
        let d = tempfile::tempdir().unwrap();
        let mut w = Watcher::new(vec![], false, 120, d.path().join("state"), Logger { path: d.path().join("log") });
        w.write_heartbeat();
        let hb = std::fs::read_to_string(d.path().join("state/git-auto-pull-watcher.json")).unwrap();
        assert!(hb.contains("\"interval_secs\":120,"), "{hb}");
        assert!(!hb.contains("\"last_cycle\":null"), "{hb}");
        assert!(hb.contains("\"last_action\":null,\"actions\":0,\"last_error\":null"), "{hb}");
    }

    fn sh(dir: &Path, args: &[&str]) {
        let o = Command::new("git").args(args).current_dir(dir).output().unwrap();
        assert!(o.status.success(), "git {:?} in {}: {}", args, dir.display(), String::from_utf8_lossy(&o.stderr));
    }
    fn head(dir: &Path) -> String {
        git(dir, &["rev-parse", "HEAD"]).unwrap().out
    }

    /// Bare remote + clones A and B, both with identity set. Returns (remote, a, b).
    fn fixture(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let remote = root.join("remote.git");
        std::fs::create_dir_all(&remote).unwrap();
        sh(&remote, &["init", "--bare", "-q", "-b", "main"]);
        let seed = root.join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        sh(&seed, &["init", "-q", "-b", "main"]);
        sh(&seed, &["config", "user.email", "t@t"]);
        sh(&seed, &["config", "user.name", "t"]);
        std::fs::write(seed.join("file"), "0\n").unwrap();
        sh(&seed, &["add", "-A"]);
        sh(&seed, &["commit", "-q", "-m", "seed"]);
        sh(&seed, &["remote", "add", "origin", remote.to_str().unwrap()]);
        sh(&seed, &["push", "-q", "origin", "main"]);
        let mut clones = Vec::new();
        for name in ["a", "b"] {
            let c = root.join(name);
            sh(root, &["clone", "-q", remote.to_str().unwrap(), c.to_str().unwrap()]);
            sh(&c, &["config", "user.email", "t@t"]);
            sh(&c, &["config", "user.name", "t"]);
            clones.push(c);
        }
        (remote, clones.remove(0), clones.remove(0))
    }
    fn push_commit(dir: &Path, content: &str) {
        std::fs::write(dir.join("file"), content).unwrap();
        sh(dir, &["commit", "-q", "-am", content]);
        sh(dir, &["push", "-q", "origin", "main"]);
    }
    fn watcher(root: &Path, repo: &Path) -> Watcher {
        Watcher::new(vec![Repo::new(repo, false)], false, 120, root.join("state"), Logger { path: root.join("log") })
    }

    /// Real git: B pushes, A's cycle fast-forwards; log and heartbeat record it.
    #[test]
    fn cycle_pulls_what_the_other_clone_pushed() {
        let d = tempfile::tempdir().unwrap();
        let (_remote, a, b) = fixture(d.path());
        let mut w = watcher(d.path(), &a);

        assert_eq!(w.cycle(), vec![Outcome::UpToDate]);
        let hb = std::fs::read_to_string(d.path().join("state/git-auto-pull-watcher.json")).unwrap();
        assert!(hb.contains("\"actions\":0,\"last_error\":null"), "{hb}");

        push_commit(&b, "1\n");
        push_commit(&b, "2\n");
        let before = head(&a);
        assert_eq!(w.cycle(), vec![Outcome::Pulled { behind: 2, deployed: false }]);
        assert_ne!(head(&a), before);
        assert_eq!(head(&a), head(&b));
        assert_eq!(std::fs::read_to_string(a.join("file")).unwrap(), "2\n");

        let log = std::fs::read_to_string(d.path().join("log")).unwrap();
        assert!(log.contains("📥 [a] Remote changes detected: 2 commits behind"), "{log}");
        assert_eq!(log.matches("✅ [a] Successfully pulled changes").count(), 1, "{log}");
        let hb = std::fs::read_to_string(d.path().join("state/git-auto-pull-watcher.json")).unwrap();
        assert!(hb.contains("\"actions\":1,"), "{hb}");
        assert!(!hb.contains("\"last_action\":null"), "{hb}");

        assert_eq!(w.cycle(), vec![Outcome::UpToDate]);
    }

    /// A modified tracked file blocks the pull, logged once across ticks; a
    /// commit unblocks it and the pull goes through.
    #[test]
    fn dirty_tree_blocks_the_pull_and_is_logged_once() {
        let d = tempfile::tempdir().unwrap();
        let (_remote, a, b) = fixture(d.path());
        let mut w = watcher(d.path(), &a);
        push_commit(&b, "1\n");

        std::fs::write(a.join("file"), "local edit\n").unwrap();
        let before = head(&a);
        assert_eq!(w.cycle(), vec![Outcome::Dirty { behind: 1 }]);
        assert_eq!(w.cycle(), vec![Outcome::Dirty { behind: 1 }]);
        assert_eq!(head(&a), before, "HEAD must not move while dirty");
        let log = std::fs::read_to_string(d.path().join("log")).unwrap();
        assert_eq!(log.matches("⏸ [a] uncommitted changes present — not pulling").count(), 1, "{log}");
        assert!(!log.contains("Remote changes detected"), "{log}");

        std::fs::write(a.join("file"), "0\n").unwrap();
        assert_eq!(w.cycle(), vec![Outcome::Pulled { behind: 1, deployed: false }]);
        let log = std::fs::read_to_string(d.path().join("log")).unwrap();
        assert!(log.contains("▶ [a] working tree clean — pulling resumed"), "{log}");
    }

    /// Local commits that diverge from origin are never merged: ff-only fails, logged.
    #[test]
    fn diverged_branch_is_a_logged_pull_failure() {
        let d = tempfile::tempdir().unwrap();
        let (_remote, a, b) = fixture(d.path());
        let mut w = watcher(d.path(), &a);
        push_commit(&b, "remote\n");
        std::fs::write(a.join("other"), "x").unwrap();
        sh(&a, &["add", "-A"]);
        sh(&a, &["commit", "-q", "-m", "local"]);
        let before = head(&a);
        match w.cycle().remove(0) {
            Outcome::PullFailed(_) => {}
            o => panic!("expected PullFailed, got {o:?}"),
        }
        assert_eq!(head(&a), before);
        let log = std::fs::read_to_string(d.path().join("log")).unwrap();
        assert!(log.contains("❌ [a] Git pull failed:"), "{log}");
        let hb = std::fs::read_to_string(d.path().join("state/git-auto-pull-watcher.json")).unwrap();
        assert!(hb.contains("\"last_error\":\"[a] "), "{hb}");
    }

    #[test]
    fn dry_run_pulls_nothing() {
        let d = tempfile::tempdir().unwrap();
        let (_remote, a, b) = fixture(d.path());
        let mut w = watcher(d.path(), &a);
        w.dry_run = true;
        push_commit(&b, "1\n");
        let before = head(&a);
        assert_eq!(w.cycle(), vec![Outcome::DryRun { behind: 1 }]);
        assert_eq!(head(&a), before);
    }
}
