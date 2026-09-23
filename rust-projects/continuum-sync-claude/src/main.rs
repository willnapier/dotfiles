//! continuum-sync-claude — import recently modified Claude Code sessions into
//! Continuum. Rust port (2026-09-23) of the bash script of the same name; run
//! every 5 minutes by `com.williamnapier.continuum-sync-claude` (Mac) and
//! `continuum-sync-claude.timer` (nimbini); the ExecStart path is unchanged.
//!
//! Behaviour kept: every `*.jsonl` under `$CLAUDE_PROJECTS` (default
//! `~/.claude/projects`) modified within the last 30 minutes, with any path
//! component starting `agent-` excluded, is offered to
//! `$CONTINUUM_BIN import -a claude-code -s <file>`; a session whose id has a
//! marker in `$CONTINUUM_CLAUDE_CLINICAL_MARKERS` (registered by `cc-clinical`
//! before Claude starts) is logged as "Skipping protected cc-clinical session"
//! and never passed on. Log lines, env overrides and the 0700 directories are
//! as before; `~/.local/share/continuum/sync.log` is the tool's own log.
//!
//! Semantic changes versus the script:
//! - An import that exits non-zero is counted: the run exits 1 and the
//!   heartbeat's `last_error` names the failed session ids. The script's
//!   `set -o pipefail` inside `fd | while read` never propagated one.
//! - No `fd` dependency (std::fs walk), so the "Refusing Claude sync: fd is
//!   unavailable" path is gone.
//! - The log is capped at 5 MB with one predecessor (`logkeep`, D2-24).
//! - Every run records `~/.local/state/watchers/continuum-sync-claude.json`
//!   (interval_secs 300) for system-health-check Check 9.
//!
//! Exit status: 0 every candidate imported (or nothing to do); 1 at least one
//! import failed or the projects directory could not be read.

mod exec;
mod outcome;

use clap::Parser;
use exec::Exec;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const NAME: &str = "continuum-sync-claude";
const INTERVAL_SECS: u64 = 300;
const WINDOW: Duration = Duration::from_secs(30 * 60);

#[derive(Parser)]
#[command(name = NAME, version, about = "Import Claude Code sessions modified in the last 30 minutes into Continuum")]
struct Cli {}

struct Config {
    continuum_bin: PathBuf,
    projects: PathBuf,
    log: PathBuf,
    markers: PathBuf,
}

impl Config {
    fn from_env(home: &Path) -> Config {
        let env_path = |k: &str, default: PathBuf| std::env::var_os(k).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty()).unwrap_or(default);
        Config {
            continuum_bin: env_path("CONTINUUM_BIN", home.join(".local/bin/continuum")),
            projects: env_path("CLAUDE_PROJECTS", home.join(".claude/projects")),
            log: env_path("CONTINUUM_SYNC_LOG", home.join(".local/share/continuum/sync.log")),
            markers: env_path("CONTINUUM_CLAUDE_CLINICAL_MARKERS", home.join(".local/share/continuum/claude-clinical-sessions")),
        }
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Candidate {
    id: String,
    path: PathBuf,
}

fn is_agent_component(p: &Path) -> bool {
    p.components().any(|c| c.as_os_str().to_string_lossy().starts_with("agent-"))
}

/// Recursively collect `*.jsonl` files under `root` modified after `since`,
/// skipping anything with an `agent-…` path component (relative to `root`).
/// Sorted for a deterministic log.
fn candidates(root: &Path, since: SystemTime) -> std::io::Result<Vec<Candidate>> {
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir)?.flatten() {
            let p = e.path();
            let rel = p.strip_prefix(root).unwrap_or(&p);
            if is_agent_component(rel) {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                stack.push(p);
                continue;
            }
            if !meta.is_file() || p.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(mtime) = meta.modified() else { continue };
            if mtime < since {
                continue;
            }
            let Some(id) = p.file_stem().and_then(|s| s.to_str()).map(String::from) else { continue };
            out.push(Candidate { id, path: p });
        }
    }
    out.sort();
    Ok(out)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Summary {
    imported: Vec<String>,
    skipped_protected: Vec<String>,
    failed: Vec<String>,
}

struct Log {
    path: PathBuf,
}

impl Log {
    fn line(&self, msg: &str) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{} {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
        }
    }
    fn raw(&self, text: &str) {
        use std::io::Write;
        if text.is_empty() {
            return;
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = f.write_all(text.as_bytes());
            if !text.ends_with('\n') {
                let _ = f.write_all(b"\n");
            }
        }
    }
}

/// One pass. `now` is injected so the 30-minute window is testable.
fn run(exec: &dyn Exec, cfg: &Config, log: &Log, now: SystemTime) -> Result<Summary, String> {
    let since = now.checked_sub(WINDOW).unwrap_or(SystemTime::UNIX_EPOCH);
    let cands = candidates(&cfg.projects, since).map_err(|e| format!("cannot read {}: {e}", cfg.projects.display()))?;
    let mut s = Summary::default();
    let bin = cfg.continuum_bin.to_string_lossy().into_owned();
    for c in cands {
        if cfg.markers.join(&c.id).exists() {
            log.line(&format!("Skipping protected cc-clinical session: {}", c.id));
            s.skipped_protected.push(c.id);
            continue;
        }
        log.line(&format!("Syncing: {}", c.id));
        let path = c.path.to_string_lossy().into_owned();
        let r = exec.run(&bin, &["import", "-a", "claude-code", "-s", &path]);
        log.raw(&r.stdout);
        log.raw(&r.stderr);
        if r.ok() {
            s.imported.push(c.id);
        } else {
            log.line(&format!("Import FAILED (exit {}): {}", r.exit_code, c.id));
            s.failed.push(c.id);
        }
    }
    Ok(s)
}

fn create_private_dir(p: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(p)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME is set"))
}

fn main() {
    let _ = Cli::parse();
    let home = home();
    let started_at = outcome::now_rfc3339();
    let cfg = Config::from_env(&home);
    if let Some(d) = cfg.log.parent() {
        let _ = create_private_dir(d);
    }
    let _ = create_private_dir(&cfg.markers);
    let log = Log { path: cfg.log.clone() };
    match logkeep::cap(&cfg.log, 5 * logkeep::MB) {
        Ok(o) if o.rolled() => log.line(&format!("rolled the previous log to {}", logkeep::predecessor(&cfg.log).display())),
        Ok(_) => {}
        Err(e) => log.line(&format!("could not cap {}: {e}", cfg.log.display())),
    }
    let (code, last_action, actions, last_error) = match run(&exec::Real, &cfg, &log, SystemTime::now()) {
        Ok(s) if s.failed.is_empty() => {
            let action = if s.imported.is_empty() { None } else { Some(format!("imported {}", s.imported.len())) };
            (0, action, s.imported.len() as u64, None)
        }
        Ok(s) => {
            let why = format!("import failed: {}", s.failed.join(", "));
            log.line(&why);
            (1, Some(format!("imported {}", s.imported.len())), s.imported.len() as u64, Some(why))
        }
        Err(why) => {
            log.line(&why);
            (1, None, 0, Some(why))
        }
    };
    let o = outcome::Outcome { name: NAME, interval_secs: INTERVAL_SECS, started_at, last_action, actions, last_error };
    if let Err(e) = outcome::record(&outcome::state_dir(&home), &o) {
        log.line(&format!("could not record outcome: {e}"));
    }
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;
    use exec::{CmdResult, Fake};

    fn touch(p: &Path, age: Duration, now: SystemTime) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "{}\n").unwrap();
        let f = std::fs::File::options().write(true).open(p).unwrap();
        f.set_modified(now - age).unwrap();
    }

    /// projects/: fresh.jsonl (5 min), protected.jsonl (5 min, marker set),
    /// old.jsonl (2 h), agent-abc.jsonl (fresh), agent-x/inner.jsonl (fresh),
    /// notes.txt (fresh, wrong extension).
    fn fixture(root: &Path, now: SystemTime) -> Config {
        let projects = root.join("projects/-Users-x");
        touch(&projects.join("fresh.jsonl"), Duration::from_secs(300), now);
        touch(&projects.join("protected.jsonl"), Duration::from_secs(300), now);
        touch(&projects.join("old.jsonl"), Duration::from_secs(7200), now);
        touch(&projects.join("agent-abc.jsonl"), Duration::from_secs(10), now);
        touch(&projects.join("agent-x/inner.jsonl"), Duration::from_secs(10), now);
        touch(&projects.join("notes.txt"), Duration::from_secs(10), now);
        let markers = root.join("markers");
        std::fs::create_dir_all(&markers).unwrap();
        std::fs::write(markers.join("protected"), "").unwrap();
        Config { continuum_bin: PathBuf::from("continuum"), projects: root.join("projects"), log: root.join("sync.log"), markers }
    }

    fn import_key(cfg: &Config, id: &str) -> String {
        Fake::key("continuum", &["import", "-a", "claude-code", "-s", &cfg.projects.join("-Users-x").join(format!("{id}.jsonl")).to_string_lossy()])
    }

    #[test]
    fn candidates_are_fresh_jsonl_only_and_never_agent_paths() {
        let d = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let cfg = fixture(d.path(), now);
        let ids: Vec<String> = candidates(&cfg.projects, now - WINDOW).unwrap().into_iter().map(|c| c.id).collect();
        assert_eq!(ids, ["fresh", "protected"]);
    }

    #[test]
    fn green_unprotected_fresh_sessions_are_imported_exit_zero() {
        let d = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let cfg = fixture(d.path(), now);
        let mut ex = Fake::default();
        ex.responses.insert(import_key(&cfg, "fresh"), CmdResult::success("imported 12 messages"));
        let log = Log { path: cfg.log.clone() };
        let s = run(&ex, &cfg, &log, now).unwrap();
        assert_eq!(s, Summary { imported: vec!["fresh".into()], skipped_protected: vec!["protected".into()], failed: vec![] });
        let calls = ex.calls.borrow();
        assert_eq!(calls.len(), 1, "exactly one import: {calls:?}");
        assert!(!calls.iter().any(|c| c.contains("protected")), "protected id must never reach the importer");
        let text = std::fs::read_to_string(&cfg.log).unwrap();
        assert!(text.contains("Syncing: fresh"));
        assert!(text.contains("Skipping protected cc-clinical session: protected"));
        assert!(text.contains("imported 12 messages"));
    }

    #[test]
    fn red_import_failure_is_reported_not_swallowed() {
        let d = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let cfg = fixture(d.path(), now);
        let mut ex = Fake::default();
        ex.responses.insert(import_key(&cfg, "fresh"), CmdResult::failure(1, "db locked"));
        let log = Log { path: cfg.log.clone() };
        let s = run(&ex, &cfg, &log, now).unwrap();
        assert_eq!(s.failed, ["fresh"]);
        assert!(s.imported.is_empty());
        let text = std::fs::read_to_string(&cfg.log).unwrap();
        assert!(text.contains("db locked"));
        assert!(text.contains("Import FAILED (exit 1): fresh"));
    }

    #[test]
    fn missing_projects_dir_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        let cfg = Config { continuum_bin: "continuum".into(), projects: d.path().join("nope"), log: d.path().join("l"), markers: d.path().join("m") };
        let log = Log { path: cfg.log.clone() };
        assert!(run(&Fake::default(), &cfg, &log, SystemTime::now()).is_err());
    }

    #[test]
    fn heartbeat_records_failure_ids() {
        let d = tempfile::tempdir().unwrap();
        let o = outcome::Outcome { name: NAME, interval_secs: INTERVAL_SECS, started_at: outcome::now_rfc3339(), last_action: None, actions: 0, last_error: Some("import failed: fresh".into()) };
        let p = outcome::record(&d.path().join("w"), &o).unwrap();
        let hb: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert_eq!(hb["last_error"], "import failed: fresh");
        assert_eq!(hb["interval_secs"], 300);
    }
}
