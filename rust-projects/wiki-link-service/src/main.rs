//! wiki-link-service — `backlinks` / `resolve-mark` watchers, the
//! `start`/`status`/`stop` supervisor that replaces `link-service`, and the
//! read-only `audit` and explicitly applied `reconcile`.
//!
//! `start` runs both watchers in ONE foreground process (two threads) so a
//! supervisor (launchd / systemd) owns exactly one PID. The PID lock lives
//! where `link-service` kept its PID file
//! (`~/scripts/wiki-link-management/logs/link-service.pid`) and is stale iff
//! the recorded PID is dead (modelled on `git-auto-push-watcher`).

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::Command;
use wiki_link_service::heartbeat::{default_state_dir, Heartbeat};
use wiki_link_service::logger::Logger;
use wiki_link_service::watch::{self, Which};
use wiki_link_service::wiki::{self, Ctx};

#[derive(Parser, Debug)]
#[command(name = "wiki-link-service", version, about = "Wiki-link watchers: ## Backlinks maintenance and ?[[missing-target]] marking (Rust port of wiki-backlinks, wiki-resolve-mark and link-service)")]
struct Cli {
    /// Directory to scan; repeatable. The FIRST is the one watched for events.
    /// Default: ~/Forge (watched) plus ~/Admin and ~/Archives when they exist.
    #[arg(long = "root", value_name = "DIR", global = true)]
    roots: Vec<PathBuf>,
    /// Directory for the PID lock and backlinks.out.log / resolve.out.log
    #[arg(long, value_name = "DIR", global = true, default_value_os_t = default_log_dir())]
    log_dir: PathBuf,
    /// Directory for the heartbeat files wiki-link-service-<sub>.json
    #[arg(long, value_name = "DIR", global = true, default_value_os_t = default_state_dir())]
    state_dir: PathBuf,
    /// Debounce window for file events, in milliseconds
    #[arg(long, value_name = "INT", global = true, default_value_t = 2000)]
    debounce_ms: u64,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug, Clone)]
enum Cmd {
    /// Run the wiki-backlinks watcher (maintains ## Backlinks sections)
    Backlinks,
    /// Run the wiki-resolve-mark watcher (marks ?[[target]] when the target is missing, unmarks when it exists)
    ResolveMark,
    /// Run both watchers in this one foreground process (for launchd / systemd)
    Start,
    /// Report whether the PID lock is held; show recent log lines and heartbeats
    Status,
    /// Send SIGTERM to the process holding the PID lock and remove the lock
    Stop,
    /// Read-only: report which ## Backlinks sections and ?[[ markers the current rules would change under --root (never writes)
    Audit,
    /// Plan the watchers' fixed point (audit narrowed to the first, watched root); pass --apply to write it atomically. Refuses --apply while the service holds its PID lock
    Reconcile {
        /// Apply the plan. Without this flag reconcile is a read-only dry run.
        #[arg(long)]
        apply: bool,
    },
    /// Print the path of the note a link name refers to (NFC-insensitive, case-insensitive, `Dir/Name` and a trailing `.md` accepted); exit 1 when no note exists. Scripts must call this before creating a note, so a typed NFC name never gets a twin beside an NFD-named file
    Resolve {
        /// The link name as typed, e.g. `Zoë Harcombe` or `NapierianLogs/Scenarios/Deep`
        name: String,
        /// Print every match (root order, then path) instead of the first
        #[arg(long)]
        all: bool,
    },
    /// Print the `[[link]]` text for a note path — the stem, NFC — for pickers that paste links into notes
    LinkFor {
        /// Path to a note (any spelling the OS gave you)
        path: PathBuf,
    },
    /// Print the paths of the notes that link to a note (name, Dir/Name, or path; either spelling)
    LinksTo {
        name: String,
    },
    /// Rename a note and rewrite every [[link]] to it in either spelling, rebuild sections, re-evaluate ?[[ markers — what the watchers do on a rename. Refuses if a note of the new name exists anywhere
    Rename {
        /// Existing note: name, Dir/Name, or path
        old: String,
        /// New bare note name (NFC is applied)
        new: String,
    },
    /// Create <dir>/<name>.md with the standard frontmatter — unless a note of that name exists anywhere under the roots, in which case print its path and exit 2 (no twin)
    New {
        name: String,
        /// Directory to create in (default: the first root)
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
    },
    /// Move <root>/Reception/<name>.md (either spelling) to <root>/<name>.md; refuses if the destination exists
    Promote {
        name: String,
    },
}

fn default_log_dir() -> PathBuf {
    wiki::home().join("scripts/wiki-link-management/logs")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd.clone() {
        Cmd::Resolve { name, all } => {
            let roots = resolve_roots(&cli)?;
            let found = forge_names::find_note(&roots, &name);
            if found.is_empty() {
                std::process::exit(1);
            }
            for p in if all { &found[..] } else { &found[..1] } {
                println!("{}", p.display());
            }
            return Ok(());
        }
        Cmd::LinkFor { path } => {
            println!("[[{}]]", forge_names::note_name(&path));
            return Ok(());
        }
        Cmd::LinksTo { name } => {
            for p in wiki_link_service::ops::backlinks(&resolve_roots(&cli)?, &name)? {
                println!("{}", p.display());
            }
            return Ok(());
        }
        Cmd::Rename { old, new } => {
            let ctx = Ctx::new(resolve_roots(&cli)?, Logger { file: None, tag: None, quiet: false });
            let (from, to) = wiki_link_service::ops::rename(&ctx, &old, &new)?;
            println!("✓ Renamed: {} → {}", from.display(), to.display());
            return Ok(());
        }
        Cmd::New { name, dir } => {
            let n = wiki_link_service::ops::new_note(&resolve_roots(&cli)?, &name, dir.as_deref())?;
            println!("{}", n.path.display());
            if !n.created {
                std::process::exit(2);
            }
            return Ok(());
        }
        Cmd::Promote { name } => {
            let (from, to) = wiki_link_service::ops::promote(&resolve_roots(&cli)?, &name)?;
            println!("✓ Promoted: {} → {}", from.display(), to.display());
            return Ok(());
        }
        Cmd::Audit => {
            let roots = resolve_roots(&cli)?;
            print!("{}", wiki_link_service::audit::audit(&roots).render());
            return Ok(());
        }
        Cmd::Reconcile { apply } => {
            let roots = resolve_roots(&cli)?;
            if apply {
                if let Some((label, pid)) = running_lock(&cli) {
                    println!("❌ {label} is running (pid {pid}) — stop it before --apply (wiki-link-service stop, or the launchd/systemd unit), then restart it afterwards");
                    std::process::exit(1);
                }
            }
            let report = wiki_link_service::reconcile::reconcile(&roots, apply)?;
            print!("{}", report.render());
            return Ok(());
        }
        Cmd::Status => return status(&cli),
        Cmd::Stop => return stop(&cli),
        _ => {}
    }
    std::fs::create_dir_all(&cli.log_dir).with_context(|| format!("creating {}", cli.log_dir.display()))?;
    match cli.cmd {
        Cmd::Backlinks => run_one(&cli, Which::Backlinks),
        Cmd::ResolveMark => run_one(&cli, Which::ResolveMark),
        Cmd::Start => start(&cli),
        Cmd::Status | Cmd::Stop | Cmd::Audit | Cmd::Reconcile { .. } | Cmd::Resolve { .. } | Cmd::LinkFor { .. } | Cmd::LinksTo { .. } | Cmd::Rename { .. } | Cmd::New { .. } | Cmd::Promote { .. } => unreachable!(),
    }
}

// ── roots ───────────────────────────────────────────────────────────
fn resolve_roots(cli: &Cli) -> Result<Vec<PathBuf>> {
    if !cli.roots.is_empty() {
        return Ok(cli.roots.clone());
    }
    let forge = wiki::home().join("Forge");
    if !forge.exists() {
        println!("❌ Forge directory not found");
        bail!("{} does not exist", forge.display());
    }
    Ok(wiki::default_roots())
}

fn ctx_for(cli: &Cli, which: Which, tag: bool) -> Result<Ctx> {
    Ok(Ctx::new(resolve_roots(cli)?, Logger { file: Some(cli.log_dir.join(which.log_file())), tag: tag.then(|| which.sub().to_string()), quiet: false }))
}

// ── lock ────────────────────────────────────────────────────────────
fn service_lock(cli: &Cli) -> PathBuf {
    cli.log_dir.join("link-service.pid")
}
fn single_lock(cli: &Cli, which: Which) -> PathBuf {
    cli.log_dir.join(format!("wiki-link-service-{}.pid", which.sub()))
}

fn pid_alive(pid: u32) -> bool {
    Command::new("kill").args(["-0", &pid.to_string()]).output().map(|o| o.status.success()).unwrap_or(false)
}

fn read_pid(lock: &Path) -> Option<u32> {
    std::fs::read_to_string(lock).ok()?.trim().parse().ok()
}

/// PID lock; stale iff the recorded PID is dead.
fn take_lock(lock: &Path, logger: &Logger) -> Result<()> {
    if let Some(pid) = read_pid(lock) {
        if pid != std::process::id() && pid_alive(pid) {
            logger.log(&format!("❌ already running — pid {pid} holds {}", lock.display()));
            std::process::exit(1);
        }
        logger.log(&format!("Removing stale lock file — pid {pid} not running"));
    }
    std::fs::write(lock, std::process::id().to_string()).with_context(|| format!("writing {}", lock.display()))
}

// ── commands ────────────────────────────────────────────────────────
fn run_one(cli: &Cli, which: Which) -> Result<()> {
    let ctx = ctx_for(cli, which, false)?;
    take_lock(&single_lock(cli, which), &ctx.logger)?;
    let mut hb = Heartbeat::new(&cli.state_dir, which.sub());
    watch::run(which, &ctx, cli.debounce_ms, &mut hb)
}

fn start(cli: &Cli) -> Result<()> {
    let logger = Logger { file: Some(cli.log_dir.join("link-service.log")), tag: None, quiet: false };
    take_lock(&service_lock(cli), &logger)?;
    logger.log(&format!("🚀 Starting wiki link management service {} (pid {})", env!("CARGO_PKG_VERSION"), std::process::id()));
    logger.log("   Architecture: Two watchers in one process");
    logger.log("   - backlinks: Maintains ## Backlinks sections");
    logger.log("   - resolve-mark: Marks/unmarks ?[[ for missing targets");

    let mut handles = Vec::new();
    for which in [Which::Backlinks, Which::ResolveMark] {
        let ctx = ctx_for(cli, which, true)?;
        let (debounce, state_dir) = (cli.debounce_ms, cli.state_dir.clone());
        let h = std::thread::Builder::new().name(which.sub().to_string()).spawn(move || -> Result<()> {
            let mut hb = Heartbeat::new(&state_dir, which.sub());
            watch::run(which, &ctx, debounce, &mut hb)
        })?;
        handles.push((which, h));
    }
    logger.log(&format!("✅ Wiki link management service started (2 watchers) — logs: {}/", cli.log_dir.display()));

    // Either watcher ending is fatal: exit non-zero so the supervisor restarts us.
    let mut failed = false;
    for (which, h) in handles {
        match h.join() {
            Ok(Ok(())) => logger.log(&format!("{} watcher ended", which.sub())),
            Ok(Err(e)) => logger.log(&format!("❌ {} watcher failed: {e:#}", which.sub())),
            Err(_) => logger.log(&format!("❌ {} watcher panicked", which.sub())),
        }
        failed = true;
    }
    let _ = std::fs::remove_file(service_lock(cli));
    if failed {
        std::process::exit(1);
    }
    Ok(())
}

fn locks(cli: &Cli) -> [(&'static str, PathBuf); 3] {
    [("link-service (start)", service_lock(cli)), ("backlinks", single_lock(cli, Which::Backlinks)), ("resolve-mark", single_lock(cli, Which::ResolveMark))]
}

/// The first lock held by a live process, if any.
fn running_lock(cli: &Cli) -> Option<(&'static str, u32)> {
    locks(cli).into_iter().find_map(|(label, lock)| read_pid(&lock).filter(|&pid| pid_alive(pid)).map(|pid| (label, pid)))
}

fn status(cli: &Cli) -> Result<()> {
    let mut running = false;
    for (label, lock) in locks(cli) {
        match read_pid(&lock) {
            Some(pid) if pid_alive(pid) => {
                println!("✅ {label}: running — pid {pid}");
                running = true;
            }
            Some(pid) => println!("⚠️  {label}: stale lock — pid {pid} not running ({})", lock.display()),
            None => {}
        }
    }
    if !running {
        println!("❌ Wiki link management service not running");
        println!("💡 Run: wiki-link-service start");
    }
    for (title, file) in [("📝 Recent backlinks activity:", Which::Backlinks.log_file()), ("🔍 Recent resolve-mark activity:", Which::ResolveMark.log_file())] {
        if let Ok(s) = std::fs::read_to_string(cli.log_dir.join(file)) {
            println!("\n{title}");
            let lines: Vec<&str> = s.lines().collect();
            for line in lines.iter().rev().take(3).rev() {
                println!("   {line}");
            }
        }
    }
    for sub in ["backlinks", "resolve-mark"] {
        let p = cli.state_dir.join(format!("wiki-link-service-{sub}.json"));
        if let Ok(s) = std::fs::read_to_string(&p) {
            println!("\n💓 {}: {}", p.display(), s.trim_end());
        }
    }
    if running {
        Ok(())
    } else {
        std::process::exit(1)
    }
}

fn stop(cli: &Cli) -> Result<()> {
    let mut any = false;
    for (label, lock) in locks(cli) {
        if let Some(pid) = read_pid(&lock) {
            if pid_alive(pid) {
                let ok = Command::new("kill").arg(pid.to_string()).status().map(|s| s.success()).unwrap_or(false);
                println!("{} {label}: pid {pid}", if ok { "🛑 Stopped" } else { "❌ Failed to signal" });
                any = true;
            } else {
                println!("⚠️  {label}: stale lock — pid {pid} not running");
            }
            let _ = std::fs::remove_file(&lock);
        }
    }
    if !any {
        println!("⚠️  Link management service not running");
    }
    Ok(())
}
