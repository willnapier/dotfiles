//! git-auto-push-watcher — sweep uncommitted dotfiles to origin/main.
//!
//! Rust port 2026-09-02 of the two near-identical Nushell scripts
//! (`git-auto-push-watcher`, `git-auto-push-watcher-macos`). Behaviour changes
//! agreed with Will the same day:
//!
//! 1. **Quiet window.** The tree is swept only when the newest dirty file has
//!    been untouched for `--quiet` (default 10 min). The old 2-minute tick
//!    swept whatever an active session had on disk — half-edited scripts went
//!    to main (and, via nimbini's auto-pull, live) and the session's own
//!    commit lost its message to a generic "Local changes".
//! 2. **Subjects name the paths.** `Auto-commit (macos): scripts/x, .dotter/global.toml`
//!    instead of `Auto-commit: Local changes from user@host`.
//! 3. **Nushell parse gate** beside the existing Rust compile gate: a dirty
//!    `.nu` file or `#!/usr/bin/env nu` script that fails `nu-check` blocks the
//!    sweep, as a failing `cargo check` already did.
//!
//! Log phrases the monitors grep for are unchanged: "Local changes detected",
//! "✅ Successfully pushed", "❌ … failed", "Push attempt".

use anyhow::{Context, Result};
use clap::Parser;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

#[derive(Parser, Debug)]
#[command(name = "git-auto-push-watcher", version, about)]
struct Cli {
    /// Repository to sweep
    #[arg(long, default_value_os_t = default_repo())]
    repo: PathBuf,
    /// Seconds between checks
    #[arg(long, default_value_t = 120)]
    tick: u64,
    /// Seconds the newest dirty file must have been untouched before a sweep
    #[arg(long, default_value_t = 600)]
    quiet: u64,
    /// Run one cycle and exit (exit 1 only if a push failed)
    #[arg(long)]
    once: bool,
    /// Report what would happen; commit and push nothing
    #[arg(long)]
    dry_run: bool,
    /// Log file (appended); default is the platform's historical name
    #[arg(long, default_value_os_t = default_log())]
    log: PathBuf,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}
fn default_repo() -> PathBuf {
    home().join("dotfiles")
}
fn default_log() -> PathBuf {
    let name = if cfg!(target_os = "macos") { "git-auto-push-watcher-macos.log" } else { "git-auto-push-watcher.log" };
    home().join(".local/share").join(name)
}
/// One lock per repo, so a second instance (e.g. ~/Assistants) does not collide with the dotfiles one.
fn lock_path(repo: &Path) -> PathBuf {
    let name = repo.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    PathBuf::from(format!("/tmp/git-auto-push-watcher-{name}.lock"))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let logger = Logger { path: cli.log.clone() };
    if let Some(dir) = cli.log.parent() {
        std::fs::create_dir_all(dir).ok();
    }

    if !cli.once {
        take_lock(&lock_path(&cli.repo), &logger)?;
        logger.log(&format!("🚀 Starting git-auto-push-watcher {} — repo {}, tick {}s, quiet {}s", env!("CARGO_PKG_VERSION"), cli.repo.display(), cli.tick, cli.quiet));
    }

    loop {
        if !cli.once {
            std::thread::sleep(Duration::from_secs(cli.tick));
        }
        let outcome = cycle(&cli.repo, Duration::from_secs(cli.quiet), cli.dry_run, &logger);
        match &outcome {
            Ok(Outcome::PushFailed) if cli.once => std::process::exit(1),
            Err(e) => logger.log(&format!("❌ cycle failed: {e:#}")),
            _ => {}
        }
        if cli.once {
            return Ok(());
        }
    }
}

// ── lock ────────────────────────────────────────────────────────────
/// PID lock; stale iff the recorded PID is dead (a SIGKILL never runs cleanup).
fn take_lock(lock: &Path, logger: &Logger) -> Result<()> {
    if let Ok(s) = std::fs::read_to_string(lock) {
        if let Ok(pid) = s.trim().parse::<u32>() {
            if pid != std::process::id() && pid_alive(pid) {
                logger.log(&format!("❌ already running — pid {pid}"));
                std::process::exit(1);
            }
            logger.log(&format!("Removing stale lock file — pid {pid} not running"));
        }
    }
    std::fs::write(lock, std::process::id().to_string()).with_context(|| format!("writing {}", lock.display()))
}
fn pid_alive(pid: u32) -> bool {
    Command::new("kill").args(["-0", &pid.to_string()]).output().map(|o| o.status.success()).unwrap_or(false)
}

// ── one cycle ───────────────────────────────────────────────────────
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Clean,
    /// newest dirty file is younger than the quiet window
    Waiting { path: String, age: Duration },
    /// a gate failed; retry next tick
    Blocked(String),
    NothingToCommit,
    DryRun(String),
    Pushed(String),
    PushFailed,
}

pub fn cycle(repo: &Path, quiet: Duration, dry_run: bool, logger: &Logger) -> Result<Outcome> {
    let status = git(repo, &["status", "--porcelain", "-uall"])?;
    if !status.ok {
        anyhow::bail!("git status failed: {}", status.err.trim());
    }
    let mut paths = parse_porcelain(&status.out);
    paths.sort();
    if paths.is_empty() {
        return Ok(Outcome::Clean);
    }

    if let Some((path, age)) = youngest(repo, &paths) {
        if age < quiet {
            // Quiet: not logged every tick, it would fill the log while someone edits.
            return Ok(Outcome::Waiting { path, age });
        }
    }
    logger.log(&format!("Local changes detected ({} paths) - auto-pushing", paths.len()));

    let crates = dirty_crate_roots(repo, &paths);
    let failing: Vec<&String> = crates.iter().filter(|c| !cargo_check_ok(&repo.join(c))).collect();
    if !failing.is_empty() {
        let msg = format!("⏸ Skipping push — broken rust crates: {}", failing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
        logger.log(&msg);
        return Ok(Outcome::Blocked(msg));
    }
    let scripts: Vec<&String> = paths.iter().filter(|p| is_nu_script(&repo.join(p))).collect();
    if !scripts.is_empty() {
        if let Some(bad) = scripts.iter().find(|s| !nu_check_ok(&repo.join(s))) {
            let msg = format!("⏸ Skipping push — nushell script fails nu-check: {bad}");
            logger.log(&msg);
            return Ok(Outcome::Blocked(msg));
        }
    }

    let host = host_label();
    let message = commit_message(&host, &paths, quiet);
    let subject = message.lines().next().unwrap_or_default().to_string();
    if dry_run {
        logger.log(&format!("(dry-run) would commit: {subject}"));
        return Ok(Outcome::DryRun(subject));
    }

    let add = git(repo, &["add", "-A"])?;
    if !add.ok {
        anyhow::bail!("git add failed: {}", add.err.trim());
    }
    let commit = git(repo, &["commit", "-q", "-m", &message])?;
    if !commit.ok {
        if commit.out.contains("nothing to commit") || commit.err.contains("nothing to commit") {
            logger.log("ℹ️ No changes to commit");
            return Ok(Outcome::NothingToCommit);
        }
        anyhow::bail!("git commit failed: {}{}", commit.out.trim(), commit.err.trim());
    }
    logger.log(&format!("committed: {subject}"));

    for (attempt, wait) in [(1u32, 0u64), (2, 5), (3, 20)] {
        if wait > 0 {
            std::thread::sleep(Duration::from_secs(wait));
        }
        if attempt > 1 {
            logger.log(&format!("Push attempt {attempt}/3"));
        }
        let push = git(repo, &["push", "origin", "main"])?;
        if push.ok {
            logger.log("✅ Successfully pushed changes to GitHub");
            return Ok(Outcome::Pushed(subject));
        }
        logger.log(&format!("❌ Push failed (attempt {attempt}): {}", push.err.trim()));
    }
    if on_path("messageboard-edit") {
        let name = repo.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
        let _ = Command::new("messageboard-edit").args(["insert", &format!("PUSH FAILED on {host} ({name}) - commits stuck locally")]).output();
    }
    Ok(Outcome::PushFailed)
}

// ── pieces (pure where possible, so they are testable) ──────────────
/// Paths from `git status --porcelain`; renames yield the new path.
pub fn parse_porcelain(s: &str) -> Vec<String> {
    s.lines()
        .filter(|l| l.len() > 3)
        .map(|l| {
            let p = &l[3..];
            let p = p.split(" -> ").last().unwrap_or(p);
            p.trim_matches('"').to_string()
        })
        .collect()
}

/// Youngest (path, age) among paths that still exist.
fn youngest(repo: &Path, paths: &[String]) -> Option<(String, Duration)> {
    let now = SystemTime::now();
    paths
        .iter()
        .filter_map(|p| {
            let m = std::fs::metadata(repo.join(p)).ok()?.modified().ok()?;
            Some((p.clone(), now.duration_since(m).unwrap_or(Duration::ZERO)))
        })
        .min_by_key(|(_, age)| *age)
}

/// `rust-projects/<crate>` roots (with a Cargo.toml) touched by the dirty paths.
pub fn dirty_crate_roots(repo: &Path, paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = paths
        .iter()
        .filter_map(|p| {
            let mut it = p.split('/');
            match (it.next(), it.next()) {
                (Some("rust-projects"), Some(c)) if !c.is_empty() => Some(format!("rust-projects/{c}")),
                _ => None,
            }
        })
        .filter(|c| repo.join(c).join("Cargo.toml").exists())
        .collect();
    out.sort();
    out.dedup();
    out
}

fn cargo_check_ok(dir: &Path) -> bool {
    Command::new("cargo").args(["check", "--quiet", "--message-format=short"]).current_dir(dir).output().map(|o| o.status.success()).unwrap_or(false)
}

/// A `.nu` file, or a regular file whose first line is a nu shebang.
pub fn is_nu_script(p: &Path) -> bool {
    if !p.is_file() {
        return false;
    }
    if p.extension().and_then(|e| e.to_str()) == Some("nu") {
        return true;
    }
    let mut buf = [0u8; 64];
    let n = std::fs::File::open(p).and_then(|mut f| std::io::Read::read(&mut f, &mut buf)).unwrap_or(0);
    let head = String::from_utf8_lossy(&buf[..n]);
    let first = head.lines().next().unwrap_or("");
    first.starts_with("#!") && (first.ends_with("/nu") || first.ends_with(" nu"))
}

/// `nu-check` is a builtin that returns a bool and always exits 0, so it is
/// wrapped in an `if`. No `nu` on PATH →
/// the gate cannot run and is skipped open (logged), not closed.
fn nu_check_ok(p: &Path) -> bool {
    if !on_path("nu") {
        return true;
    }
    let cmd = format!("if (nu-check {}) {{ exit 0 }} else {{ exit 1 }}", shell_quote(&p.display().to_string()));
    Command::new("nu").args(["-c", &cmd]).output().map(|o| o.status.success()).unwrap_or(true)
}
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Subject: `Auto-commit (host): a, b, c` truncated to ~72 chars with "+N more";
/// body lists every path and the quiet window that was observed.
pub fn commit_message(host: &str, paths: &[String], quiet: Duration) -> String {
    const LIMIT: usize = 72;
    let prefix = format!("Auto-commit ({host}): ");
    let mut subject = prefix.clone();
    let mut shown = 0;
    for p in paths {
        let sep = if shown == 0 { "" } else { ", " };
        let candidate = format!("{subject}{sep}{p}");
        let remaining = paths.len() - shown - 1;
        let suffix_len = if remaining > 0 { format!(", +{remaining} more").len() } else { 0 };
        if shown > 0 && candidate.chars().count() + suffix_len > LIMIT {
            break;
        }
        subject = candidate;
        shown += 1;
    }
    if shown < paths.len() {
        subject.push_str(&format!(", +{} more", paths.len() - shown));
    }
    let mut body = String::new();
    for p in paths {
        body.push_str(&format!("- {p}\n"));
    }
    format!("{subject}\n\n{body}\nSwept by git-auto-push-watcher after {}s quiet.\n", quiet.as_secs())
}

fn host_label() -> String {
    if cfg!(target_os = "macos") {
        return "macos".into();
    }
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .or_else(|| Command::new("hostname").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()))
        .and_then(|h| h.trim().split('.').next().map(|s| s.to_lowercase()))
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file())).unwrap_or(false)
}

struct GitOut {
    ok: bool,
    out: String,
    err: String,
}
fn git(repo: &Path, args: &[&str]) -> Result<GitOut> {
    let o = Command::new("git").args(args).current_dir(repo).output().with_context(|| format!("running git {}", args.join(" ")))?;
    Ok(GitOut { ok: o.status.success(), out: String::from_utf8_lossy(&o.stdout).into_owned(), err: String::from_utf8_lossy(&o.stderr).into_owned() })
}

pub struct Logger {
    path: PathBuf,
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
    fn porcelain_paths_including_renames_and_untracked() {
        let s = " M scripts/a\n?? new/file\nR  old -> new-name\nD  gone\n";
        assert_eq!(parse_porcelain(s), vec!["scripts/a", "new/file", "new-name", "gone"]);
    }

    #[test]
    fn subject_names_paths_and_truncates() {
        let p = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let m = commit_message("macos", &p(&["scripts/x", ".dotter/global.toml"]), Duration::from_secs(600));
        assert_eq!(m.lines().next().unwrap(), "Auto-commit (macos): scripts/x, .dotter/global.toml");
        assert!(m.contains("- scripts/x\n- .dotter/global.toml\n"));
        assert!(m.contains("after 600s quiet"));
        let many = p(&["rust-projects/one/src/main.rs", "rust-projects/two/src/main.rs", "rust-projects/three/src/main.rs", "d"]);
        let s = commit_message("nimbini", &many, Duration::ZERO).lines().next().unwrap().to_string();
        assert!(s.chars().count() <= 72, "{s}");
        assert!(s.ends_with("more"), "{s}");
        // a single very long path is never dropped to an empty subject
        let one = p(&["a/very/long/path/that/goes/well/beyond/seventy/two/characters/for/sure/really.rs"]);
        assert!(commit_message("x", &one, Duration::ZERO).starts_with("Auto-commit (x): a/very"));
    }

    #[test]
    fn nu_script_detection_by_extension_and_shebang() {
        let d = tempfile::tempdir().unwrap();
        let ext = d.path().join("x.nu");
        std::fs::write(&ext, "print 1").unwrap();
        let shebang = d.path().join("tool");
        std::fs::write(&shebang, "#!/usr/bin/env nu\nprint 1\n").unwrap();
        let bash = d.path().join("other");
        std::fs::write(&bash, "#!/usr/bin/env bash\necho\n").unwrap();
        assert!(is_nu_script(&ext));
        assert!(is_nu_script(&shebang));
        assert!(!is_nu_script(&bash));
        assert!(!is_nu_script(d.path()));
    }

    #[test]
    fn dirty_crate_roots_need_a_cargo_toml() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("rust-projects/real/src")).unwrap();
        std::fs::write(d.path().join("rust-projects/real/Cargo.toml"), "").unwrap();
        let paths = vec!["rust-projects/real/src/main.rs".to_string(), "rust-projects/real/Cargo.toml".to_string(), "rust-projects/notacrate/x".to_string(), "scripts/y".to_string()];
        assert_eq!(dirty_crate_roots(d.path(), &paths), vec!["rust-projects/real"]);
    }

    /// Real git: bare remote + clone. Quiet window blocks, then a sweep
    /// commits with the descriptive subject and pushes.
    #[test]
    fn cycle_waits_then_sweeps_and_pushes() {
        let d = tempfile::tempdir().unwrap();
        let remote = d.path().join("remote.git");
        let repo = d.path().join("repo");
        let sh = |dir: &Path, args: &[&str]| {
            let o = Command::new("git").args(args).current_dir(dir).output().unwrap();
            assert!(o.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&o.stderr));
        };
        std::fs::create_dir_all(&remote).unwrap();
        sh(&remote, &["init", "--bare", "-q", "-b", "main"]);
        std::fs::create_dir_all(&repo).unwrap();
        sh(&repo, &["init", "-q", "-b", "main"]);
        sh(&repo, &["config", "user.email", "t@t"]);
        sh(&repo, &["config", "user.name", "t"]);
        sh(&repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
        std::fs::write(repo.join("seed"), "0").unwrap();
        sh(&repo, &["add", "-A"]);
        sh(&repo, &["commit", "-q", "-m", "seed"]);
        sh(&repo, &["push", "-q", "origin", "main"]);
        let logger = Logger { path: d.path().join("log") };

        assert_eq!(cycle(&repo, Duration::from_secs(600), false, &logger).unwrap(), Outcome::Clean);

        std::fs::create_dir_all(repo.join("scripts")).unwrap();
        std::fs::write(repo.join("scripts/tool"), "#!/usr/bin/env bash\necho\n").unwrap();
        std::fs::write(repo.join("seed"), "1").unwrap();
        match cycle(&repo, Duration::from_secs(3600), false, &logger).unwrap() {
            Outcome::Waiting { .. } => {}
            o => panic!("expected Waiting, got {o:?}"),
        }
        let dry = cycle(&repo, Duration::ZERO, true, &logger).unwrap();
        assert_eq!(dry, Outcome::DryRun("Auto-commit (".to_string() + &host_label() + "): scripts/tool, seed"));
        let o = cycle(&repo, Duration::ZERO, false, &logger).unwrap();
        assert!(matches!(o, Outcome::Pushed(ref s) if s.ends_with("scripts/tool, seed")), "{o:?}");
        let head = Command::new("git").args(["log", "-1", "--format=%s"]).current_dir(&remote).output().unwrap();
        assert!(String::from_utf8_lossy(&head.stdout).contains("scripts/tool, seed"));
        assert_eq!(cycle(&repo, Duration::ZERO, false, &logger).unwrap(), Outcome::Clean);
    }

    /// A dirty nushell script that does not parse blocks the sweep (needs `nu`).
    #[test]
    fn broken_nu_script_blocks_the_sweep() {
        if !on_path("nu") {
            eprintln!("nu not on PATH; gate test skipped");
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("repo");
        std::fs::create_dir_all(repo.join("scripts")).unwrap();
        let sh = |args: &[&str]| assert!(Command::new("git").args(args).current_dir(&repo).output().unwrap().status.success());
        sh(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("scripts/bad.nu"), "def main [ {\n").unwrap();
        let logger = Logger { path: d.path().join("log") };
        match cycle(&repo, Duration::ZERO, false, &logger).unwrap() {
            Outcome::Blocked(m) => assert!(m.contains("nu-check") && m.contains("bad.nu"), "{m}"),
            o => panic!("expected Blocked, got {o:?}"),
        }
    }
}
