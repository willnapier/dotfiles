//! forge-md-revs — rolling local snapshots of ~/Forge markdown writes
//! (Helix autosave, Esc-write, etc.).
//!
//! Rust port 2026-09-02 of the Nushell script `scripts/forge-md-revs`.
//! Machine-local: ~/.local/share/forge-md-revs/ — not Syncthing, not Clinical.
//! Keep: every distinct write from the last 10 minutes, plus the last 20
//! snapshots per file. Debounce is short (100 ms) on purpose: a 2 s debounce
//! would collapse "good save then delete save" into only the deletion.
//!
//! Store layout is byte-compatible with the script's:
//!   `<store>/<path relative to Forge, .md name included>/<YYYYmmddTHHMMSS>.md`
//! (with `-<nanos>` appended when that second is already taken), so
//! existing snapshots stay listable and restorable.
//!
//! The store is keyed by the NFC spelling of that relative path (forge-names
//! rule): the watcher hands over OS-spelled paths (NFD on the Mac) and the
//! CLI is typed NFC, and both must land in one directory. A store directory
//! left under the raw spelling by an earlier version is renamed to NFC the
//! first time that note is touched. `restore` never joins the typed path
//! back: it is resolved against the Forge tree and the existing file is
//! written. Case is kept as-is — the store is host-local and the script's
//! layout was case-preserving.
//!
//! Differences from the script: dedup compares the file's bytes with the
//! newest snapshot's bytes instead of comparing sha256 digests (same
//! decision, no hash dependency); `restore` takes the index as a positional
//! argument; a heartbeat JSON is written to `~/.local/state/watchers/` after
//! every handled event.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime};

const KEEP_COUNT: usize = 20;
const KEEP_AGE: Duration = Duration::from_secs(10 * 60);
const DEBOUNCE: Duration = Duration::from_millis(100);
const LOCK: &str = "/tmp/forge-md-revs.lock";
const NAME: &str = "forge-md-revs";

#[derive(Parser, Debug)]
#[command(name = NAME, version, about = "Local rolling snapshots of ~/Forge *.md writes")]
struct Cli {
    /// Forge root to watch (paths given to list/restore must live under it)
    #[arg(long, global = true, default_value_os_t = default_forge())]
    forge: PathBuf,
    /// Snapshot store root
    #[arg(long, global = true, default_value_os_t = default_store())]
    store: PathBuf,
    /// Directory for the heartbeat file `forge-md-revs.json`
    #[arg(long, global = true, default_value_os_t = default_state_dir())]
    state_dir: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Daemon (systemd/launchd): snapshot every distinct .md write under Forge
    Watch,
    /// Notes that have snapshots, or the snapshots of one note (0 = newest)
    List {
        /// Forge markdown path; omit to list every note with snapshots
        path: Option<PathBuf>,
    },
    /// Copy snapshot INDEX (0 = newest) back over the note
    Restore {
        /// Forge markdown path
        path: PathBuf,
        /// Snapshot index from `list <path>`
        #[arg(default_value_t = 0)]
        index: usize,
    },
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}
fn default_forge() -> PathBuf {
    home().join("Forge")
}
fn default_store() -> PathBuf {
    home().join(".local/share/forge-md-revs")
}
fn default_state_dir() -> PathBuf {
    home().join(".local/state/watchers")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let store = Store { forge: cli.forge.clone(), root: cli.store.clone() };
    let logger = Logger { path: None };
    match cli.cmd {
        Cmd::Watch => {
            if !store.forge.exists() {
                println!("Forge not found: {}", store.forge.display());
                std::process::exit(1);
            }
            take_lock(&logger)?;
            let mut hb = Heartbeat::new(cli.state_dir, NAME);
            let never = AtomicBool::new(false);
            run_watch(&store, &logger, &mut hb, &never)
        }
        Cmd::List { path: None } => {
            for line in store.list_all_lines()? {
                println!("{line}");
            }
            Ok(())
        }
        Cmd::List { path: Some(p) } => {
            for line in store.list_one_lines(&p)? {
                println!("{line}");
            }
            Ok(())
        }
        Cmd::Restore { path, index } => {
            for line in store.restore_one(&path, index)? {
                println!("{line}");
            }
            Ok(())
        }
    }
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

// ── store (pure core) ───────────────────────────────────────────────
pub struct Store {
    pub forge: PathBuf,
    pub root: PathBuf,
}

/// One snapshot file, as `list` shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Rev {
    pub index: usize,
    pub modified: SystemTime,
    pub bytes: u64,
    pub file: PathBuf,
}

/// The script's skip rule, verbatim: lowercase the path and refuse
/// `/.stversions/`, `/.syncthing` and `/.git/`.
pub fn skip_path(path: &Path) -> bool {
    let p = path.to_string_lossy().to_lowercase();
    p.contains("/.stversions/") || p.contains("/.syncthing") || p.contains("/.git/")
}

fn is_md(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("md")
}

impl Store {
    /// `<store>/<NFC path relative to forge>` — the note's own name (with
    /// `.md`) becomes a directory holding its snapshots. The key is the NFC
    /// name, whatever spelling the watcher or the CLI supplied. A directory
    /// still under the raw (NFD) spelling is migrated by rename on first
    /// touch, so it is never twinned or lost.
    pub fn rev_dir_for(&self, path: &Path) -> Result<PathBuf> {
        let raw = self.relative_to_forge(path)?;
        let key = PathBuf::from(forge_names::nfc(&raw.to_string_lossy()));
        let dir = self.root.join(&key);
        if key != raw && !dir.exists() {
            let legacy = self.root.join(&raw);
            if legacy.is_dir() {
                if let Some(parent) = dir.parent() {
                    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
                }
                std::fs::rename(&legacy, &dir).with_context(|| format!("migrating {} to NFC {}", legacy.display(), dir.display()))?;
            }
        }
        Ok(dir)
    }

    /// The on-disk file for a CLI-typed Forge path: each component is looked
    /// up in its directory by NFC comparison (forge-names `find_in_dir`) and
    /// the entry's own path is used, so a note spelled NFD on disk is
    /// overwritten, not twinned. A component with no entry (a note deleted
    /// since its snapshot, or a new directory) is joined as typed — there is
    /// nothing existing to collide with.
    fn resolve_in_forge(&self, path: &Path) -> Result<PathBuf> {
        let rel = self.relative_to_forge(path)?;
        let mut cur = self.forge.clone();
        for comp in rel.components() {
            let name = comp.as_os_str().to_string_lossy();
            cur = forge_names::find_in_dir(&cur, &name).unwrap_or_else(|| cur.join(comp));
        }
        Ok(cur)
    }

    /// Relative path under Forge. Tries the configured root, then its
    /// canonical form (FSEvents reports `/private/var/...` for a
    /// `/var/...` root), then the canonical form of the path itself.
    fn relative_to_forge(&self, path: &Path) -> Result<PathBuf> {
        if let Ok(rel) = path.strip_prefix(&self.forge) {
            return Ok(rel.to_path_buf());
        }
        let canon_forge = self.forge.canonicalize().unwrap_or_else(|_| self.forge.clone());
        if let Ok(rel) = path.strip_prefix(&canon_forge) {
            return Ok(rel.to_path_buf());
        }
        if let Ok(canon) = path.canonicalize() {
            if let Ok(rel) = canon.strip_prefix(&canon_forge) {
                return Ok(rel.to_path_buf());
            }
        }
        bail!("{} is not under Forge root {}", path.display(), self.forge.display())
    }

    /// Snapshot files in `revdir`, newest first (by mtime), indexed from 0.
    fn revs_in(&self, revdir: &Path) -> Result<Vec<Rev>> {
        let mut out = Vec::new();
        if !revdir.is_dir() {
            return Ok(out);
        }
        for entry in std::fs::read_dir(revdir).with_context(|| format!("reading {}", revdir.display()))? {
            let entry = entry?;
            let meta = entry.metadata()?;
            if !meta.is_file() {
                continue;
            }
            out.push(Rev { index: 0, modified: meta.modified()?, bytes: meta.len(), file: entry.path() });
        }
        // newest first; the timestamp filename breaks mtime ties
        out.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| b.file.cmp(&a.file)));
        for (i, r) in out.iter_mut().enumerate() {
            r.index = i;
        }
        Ok(out)
    }

    /// Copy `path` into its rev dir if it is a Forge `.md` whose content
    /// differs from the newest snapshot. Returns the snapshot written, if any.
    pub fn snapshot(&self, path: &Path) -> Result<Option<PathBuf>> {
        if !path.exists() || skip_path(path) || !is_md(path) {
            return Ok(None);
        }
        let revdir = self.rev_dir_for(path)?;
        std::fs::create_dir_all(&revdir).with_context(|| format!("creating {}", revdir.display()))?;

        let content = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        if let Some(latest) = self.revs_in(&revdir)?.first() {
            if std::fs::read(&latest.file).map(|old| old == content).unwrap_or(false) {
                return Ok(None);
            }
        }

        let now = chrono::Local::now();
        let ts = now.format("%Y%m%dT%H%M%S").to_string();
        let mut dest = revdir.join(format!("{ts}.md"));
        if dest.exists() {
            dest = revdir.join(format!("{ts}-{}.md", now.format("%f")));
        }
        std::fs::write(&dest, &content).with_context(|| format!("writing {}", dest.display()))?;
        self.prune(&revdir)?;
        Ok(Some(dest))
    }

    /// Keep the newest KEEP_COUNT snapshots plus anything younger than KEEP_AGE.
    fn prune(&self, revdir: &Path) -> Result<()> {
        let cutoff = SystemTime::now() - KEEP_AGE;
        for r in self.revs_in(revdir)? {
            let keep = r.index < KEEP_COUNT || r.modified > cutoff;
            if !keep {
                std::fs::remove_file(&r.file).with_context(|| format!("pruning {}", r.file.display()))?;
            }
        }
        Ok(())
    }

    /// Snapshots of one note, newest first. Empty if it has none.
    pub fn list_one(&self, path: &Path) -> Result<Vec<Rev>> {
        let revdir = self.rev_dir_for(path)?;
        self.revs_in(&revdir)
    }

    fn list_one_lines(&self, path: &Path) -> Result<Vec<String>> {
        let rows = self.list_one(path)?;
        if rows.is_empty() {
            return Ok(vec![format!("No snapshots for {}", path.display())]);
        }
        let mut out = vec![format!("{:>5}  {:<19}  {:>8}  file", "index", "modified", "bytes")];
        for r in rows {
            out.push(format!("{:>5}  {:<19}  {:>8}  {}", r.index, fmt_time(r.modified), r.bytes, r.file.display()));
        }
        Ok(out)
    }

    /// Every note with snapshots: (note relative to store, count, newest mtime, newest bytes), newest first.
    pub fn list_all(&self) -> Result<Vec<(PathBuf, usize, SystemTime, u64)>> {
        let mut out = Vec::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    stack.push(entry.path());
                }
            }
            if let Some(newest) = self.revs_in(&dir)?.first() {
                let note = dir.strip_prefix(&self.root).unwrap_or(&dir).to_path_buf();
                out.push((note, self.revs_in(&dir)?.len(), newest.modified, newest.bytes));
            }
        }
        out.sort_by(|a, b| b.2.cmp(&a.2));
        Ok(out)
    }

    fn list_all_lines(&self) -> Result<Vec<String>> {
        if !self.root.exists() {
            return Ok(vec!["No snapshots yet (watcher has not seen a Forge .md write).".into()]);
        }
        let rows = self.list_all()?;
        let mut out = vec![format!("{:>9}  {:<19}  {:>12}  note", "snapshots", "newest", "newest_bytes")];
        for (note, n, newest, bytes) in rows {
            out.push(format!("{n:>9}  {:<19}  {bytes:>12}  {}", fmt_time(newest), note.display()));
        }
        Ok(out)
    }

    /// Copy snapshot `index` back over `path`. Returns the lines to print.
    pub fn restore_one(&self, path: &Path, index: usize) -> Result<Vec<String>> {
        let rows = self.list_one(path)?;
        if rows.is_empty() {
            return Ok(vec![format!("No snapshots for {}", path.display())]);
        }
        let Some(pick) = rows.iter().find(|r| r.index == index) else {
            return Ok(vec![format!("No snapshot at index {index}. Newest is index 0.")]);
        };
        let content = std::fs::read(&pick.file).with_context(|| format!("reading {}", pick.file.display()))?;
        let target = self.resolve_in_forge(path)?;
        std::fs::write(&target, content).with_context(|| format!("writing {}", target.display()))?;
        Ok(vec![
            format!("Restored {} -> {}", pick.file.display(), target.display()),
            "If Helix has this buffer open, run :reload (gR) — do not keep typing on the stale buffer.".to_string(),
        ])
    }
}

fn fmt_time(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M:%S").to_string()
}

// ── watch ───────────────────────────────────────────────────────────
/// Nushell's `watch` reported Write/Modify/Create; notify's equivalents.
fn wanted(kind: &EventKind) -> bool {
    matches!(kind, EventKind::Create(_) | EventKind::Modify(notify::event::ModifyKind::Data(_)) | EventKind::Modify(notify::event::ModifyKind::Any) | EventKind::Modify(notify::event::ModifyKind::Name(_)))
}

/// Recursive watch of `<forge>/**/*.md`, 100 ms trailing debounce per path,
/// one `snapshot` per settled path. Returns when `stop` is set (tests) —
/// the daemon never sets it.
pub fn run_watch(store: &Store, logger: &Logger, hb: &mut Heartbeat, stop: &AtomicBool) -> Result<()> {
    std::fs::create_dir_all(&store.root).with_context(|| format!("creating {}", store.root.display()))?;
    logger.log(&format!("forge-md-revs watching {}/**/*.md -> {}", store.forge.display(), store.root.display()));

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
    watcher.watch(&store.forge, RecursiveMode::Recursive).with_context(|| format!("watching {}", store.forge.display()))?;
    // startup heartbeat: last_cycle = now, no action yet
    hb.cycle();
    if let Err(e) = hb.write() {
        logger.log(&format!("❌ heartbeat write failed: {e:#}"));
    }

    let mut pending: HashMap<PathBuf, Instant> = HashMap::new();
    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(ev) if wanted(&ev.kind) => {
                for p in ev.paths.into_iter().filter(|p| is_md(p) && !skip_path(p)) {
                    pending.insert(p, Instant::now());
                }
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => bail!("watcher channel closed"),
        }
        let settled: Vec<PathBuf> = pending.iter().filter(|(_, t)| t.elapsed() >= DEBOUNCE).map(|(p, _)| p.clone()).collect();
        for p in settled {
            pending.remove(&p);
            hb.cycle();
            match store.snapshot(&p) {
                Ok(Some(dest)) => {
                    logger.log(&format!("snapshot {} -> {}", p.display(), dest.display()));
                    hb.action();
                }
                Ok(None) => {}
                Err(e) => {
                    logger.log(&format!("❌ snapshot failed for {}: {e:#}", p.display()));
                    hb.error(format!("{e:#}"));
                }
            }
            if let Err(e) = hb.write() {
                logger.log(&format!("❌ heartbeat write failed: {e:#}"));
            }
        }
    }
    Ok(())
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
            "{{\"watcher\":{},\"version\":{},\"interval_secs\":0,\"started_at\":\"{}\",\"last_cycle\":{},\"last_action\":{},\"actions\":{},\"last_error\":{},\"host\":{}}}\n",
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
/// Prints, and appends to `path` when one is set. The daemon logs to
/// stdout only — its units capture that into journal/launchd files.
pub struct Logger {
    pub path: Option<PathBuf>,
}
impl Logger {
    fn log(&self, msg: &str) {
        let line = format!("[{}] {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
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
    use std::sync::Arc;

    fn setup() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let forge = d.path().join("Forge");
        std::fs::create_dir_all(forge.join("NapierianLogs/DayPages")).unwrap();
        let store = Store { forge, root: d.path().join(".local/share/forge-md-revs") };
        (d, store)
    }

    #[test]
    fn skip_path_rules_match_the_script() {
        assert!(skip_path(Path::new("/Users/w/Forge/.stversions/x.md")));
        assert!(skip_path(Path::new("/Users/w/Forge/.STVERSIONS/x.md")));
        assert!(skip_path(Path::new("/Users/w/Forge/.syncthing.note.md.tmp")));
        assert!(skip_path(Path::new("/Users/w/Forge/.git/COMMIT_EDITMSG.md")));
        assert!(!skip_path(Path::new("/Users/w/Forge/NapierianLogs/DayPages/2026-09-02.md")));
        assert!(!skip_path(Path::new("/Users/w/Forge/git-notes/x.md")));
        // non-md and skipped paths never produce a snapshot
        let (_d, s) = setup();
        let txt = s.forge.join("a.txt");
        std::fs::write(&txt, "x").unwrap();
        assert_eq!(s.snapshot(&txt).unwrap(), None);
        std::fs::create_dir_all(s.forge.join(".git")).unwrap();
        let g = s.forge.join(".git/x.md");
        std::fs::write(&g, "x").unwrap();
        assert_eq!(s.snapshot(&g).unwrap(), None);
        assert!(!s.root.exists());
    }

    /// The script: `store-root | path join ($path | path relative-to forge)`,
    /// then `<revdir>/<%Y%m%dT%H%M%S>.md`.
    #[test]
    fn store_layout_matches_the_script() {
        let s = Store { forge: PathBuf::from("/Users/w/Forge"), root: PathBuf::from("/Users/w/.local/share/forge-md-revs") };
        let rd = s.rev_dir_for(Path::new("/Users/w/Forge/NapierianLogs/DayPages/2026-09-02.md")).unwrap();
        assert_eq!(rd, PathBuf::from("/Users/w/.local/share/forge-md-revs/NapierianLogs/DayPages/2026-09-02.md"));
        assert!(s.rev_dir_for(Path::new("/elsewhere/x.md")).is_err());

        let (_d, s) = setup();
        let note = s.forge.join("NapierianLogs/DayPages/2026-09-02.md");
        std::fs::write(&note, "one").unwrap();
        let dest = s.snapshot(&note).unwrap().unwrap();
        assert_eq!(dest.parent().unwrap(), s.root.join("NapierianLogs/DayPages/2026-09-02.md"));
        let name = dest.file_name().unwrap().to_str().unwrap();
        assert_eq!(name.len(), "20260902T101010.md".len(), "{name}");
        assert_eq!(&name[8..9], "T");
        assert!(name.ends_with(".md"));
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "one");
    }

    #[test]
    fn same_content_twice_yields_one_snapshot() {
        let (_d, s) = setup();
        let note = s.forge.join("n.md");
        std::fs::write(&note, "same").unwrap();
        assert!(s.snapshot(&note).unwrap().is_some());
        assert_eq!(s.snapshot(&note).unwrap(), None);
        assert_eq!(s.list_one(&note).unwrap().len(), 1);
        std::fs::write(&note, "changed").unwrap();
        let second = s.snapshot(&note).unwrap().unwrap();
        // same second → the nanos suffix keeps the name unique
        let revs = s.list_one(&note).unwrap();
        assert_eq!(revs.len(), 2);
        assert!(revs.iter().any(|r| r.file == second));
    }

    #[test]
    fn list_orders_newest_first_and_indexes_from_zero() {
        let (_d, s) = setup();
        let note = s.forge.join("n.md");
        let revdir = s.rev_dir_for(&note).unwrap();
        std::fs::create_dir_all(&revdir).unwrap();
        // hand-made snapshots with distinct mtimes, as a long-running store has
        let f = |name: &str, body: &str, secs_ago: u64| {
            let p = revdir.join(name);
            std::fs::write(&p, body).unwrap();
            let t = SystemTime::now() - Duration::from_secs(secs_ago);
            std::fs::File::open(&p).unwrap().set_modified(t).unwrap();
            p
        };
        let old = f("20260901T090000.md", "old", 300);
        let mid = f("20260901T090500.md", "mid", 200);
        let new = f("20260901T091000.md", "new", 100);
        let revs = s.list_one(&note).unwrap();
        assert_eq!(revs.iter().map(|r| r.file.clone()).collect::<Vec<_>>(), vec![new.clone(), mid, old]);
        assert_eq!(revs.iter().map(|r| r.index).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(revs[0].bytes, 3);
        let all = s.list_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].0, PathBuf::from("n.md"));
        assert_eq!(all[0].1, 3);
        assert_eq!(all[0].2, std::fs::metadata(&new).unwrap().modified().unwrap());
        assert_eq!(s.list_one(&s.forge.join("none.md")).unwrap(), vec![]);
    }

    #[test]
    fn restore_round_trip_prints_the_two_lines() {
        let (_d, s) = setup();
        let note = s.forge.join("n.md");
        std::fs::write(&note, "first").unwrap();
        let snap = s.snapshot(&note).unwrap().unwrap();
        std::fs::write(&note, "second, to be discarded").unwrap();
        let lines = s.restore_one(&note, 0).unwrap();
        assert_eq!(lines[0], format!("Restored {} -> {}", snap.display(), note.display()));
        assert_eq!(lines[1], "If Helix has this buffer open, run :reload (gR) — do not keep typing on the stale buffer.");
        assert_eq!(std::fs::read_to_string(&note).unwrap(), "first");
        assert_eq!(s.restore_one(&note, 7).unwrap(), vec!["No snapshot at index 7. Newest is index 0."]);
        let other = s.forge.join("other.md");
        assert_eq!(s.restore_one(&other, 0).unwrap(), vec![format!("No snapshots for {}", other.display())]);
    }

    #[test]
    fn prune_keeps_twenty_or_young() {
        let (_d, s) = setup();
        let note = s.forge.join("n.md");
        let revdir = s.rev_dir_for(&note).unwrap();
        std::fs::create_dir_all(&revdir).unwrap();
        for i in 0..25 {
            let p = revdir.join(format!("20260101T00{i:04}.md"));
            std::fs::write(&p, i.to_string()).unwrap();
            let t = SystemTime::now() - Duration::from_secs(3600 + (25 - i) as u64);
            std::fs::File::open(&p).unwrap().set_modified(t).unwrap();
        }
        s.prune(&revdir).unwrap();
        assert_eq!(s.list_one(&note).unwrap().len(), KEEP_COUNT);
        // a young one beyond the count survives
        for i in 0..5 {
            std::fs::write(revdir.join(format!("20270101T00000{i}.md")), "young").unwrap();
        }
        s.prune(&revdir).unwrap();
        assert_eq!(s.list_one(&note).unwrap().len(), KEEP_COUNT);
        let young = s.list_one(&note).unwrap().iter().filter(|r| r.file.to_string_lossy().contains("2027")).count();
        assert_eq!(young, 5);
    }

    const NFD: &str = "Zoe\u{0308} Harcombe";
    const NFC: &str = "Zoë Harcombe";

    /// The watcher records the OS spelling (NFD on the Mac); the CLI is typed
    /// NFC. One store directory, keyed NFC, serves both.
    #[test]
    fn watcher_nfd_path_and_cli_nfc_path_share_one_store_dir() {
        let (_d, s) = setup();
        let subdir = s.forge.join("Zoe\u{0308}");
        std::fs::create_dir_all(&subdir).unwrap();
        let watched = subdir.join(format!("{NFD}.md"));
        std::fs::write(&watched, "one").unwrap();
        let dest = s.snapshot(&watched).unwrap().unwrap();
        assert_eq!(dest.parent().unwrap(), s.root.join("Zoë").join(format!("{NFC}.md")), "store dir is NFC bytes");

        let typed = s.forge.join("Zoë").join(format!("{NFC}.md"));
        assert_eq!(s.list_one(&typed).unwrap().len(), 1);
        assert_eq!(s.list_one(&typed).unwrap(), s.list_one(&watched).unwrap());
        let all = s.list_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].0, PathBuf::from("Zoë").join(format!("{NFC}.md")));
        assert_eq!(std::fs::read_dir(&s.root).unwrap().count(), 1, "one entry under the store, not twins");
    }

    /// A store directory written under the raw NFD spelling by an earlier
    /// version is found and renamed to NFC on first touch.
    #[test]
    fn legacy_nfd_store_dir_is_migrated_on_first_touch() {
        let (_d, s) = setup();
        let legacy = s.root.join(format!("{NFD}.md"));
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("20260101T000000.md"), "old").unwrap();
        let typed = s.forge.join(format!("{NFC}.md"));
        let revs = s.list_one(&typed).unwrap();
        assert_eq!(revs.len(), 1);
        assert_eq!(std::fs::read_to_string(&revs[0].file).unwrap(), "old");
        assert_eq!(std::fs::read_dir(&s.root).unwrap().count(), 1);
        let entry = std::fs::read_dir(&s.root).unwrap().next().unwrap().unwrap().path();
        assert_eq!(forge_names::file_name(&entry), format!("{NFC}.md"));
        // the watcher's spelling reaches the same, migrated directory
        assert_eq!(s.list_one(&s.forge.join(format!("{NFD}.md"))).unwrap().len(), 1);
    }

    /// `restore` with an NFC-typed path writes over the NFD-named file that
    /// exists, and recreates a deleted note as typed.
    #[test]
    fn restore_writes_to_the_existing_file_whatever_its_spelling() {
        let (_d, s) = setup();
        let subdir = s.forge.join("Zoe\u{0308}");
        std::fs::create_dir_all(&subdir).unwrap();
        let on_disk = subdir.join(format!("{NFD}.md"));
        std::fs::write(&on_disk, "first").unwrap();
        s.snapshot(&on_disk).unwrap().unwrap();
        std::fs::write(&on_disk, "second, to be discarded").unwrap();

        let typed = s.forge.join("Zoë").join(format!("{NFC}.md"));
        let lines = s.restore_one(&typed, 0).unwrap();
        assert!(lines[0].ends_with(&format!(" -> {}", on_disk.display())), "{}", lines[0]);
        assert_eq!(std::fs::read_to_string(&on_disk).unwrap(), "first");
        assert_eq!(std::fs::read_dir(&subdir).unwrap().count(), 1, "no NFC twin beside the NFD file");

        std::fs::remove_file(&on_disk).unwrap();
        s.restore_one(&typed, 0).unwrap();
        assert_eq!(std::fs::read_dir(&subdir).unwrap().count(), 1);
        let back = std::fs::read_dir(&subdir).unwrap().next().unwrap().unwrap().path();
        assert_eq!(std::fs::read_to_string(&back).unwrap(), "first");
    }

    #[test]
    fn heartbeat_json_shape() {
        let d = tempfile::tempdir().unwrap();
        let mut hb = Heartbeat::new(d.path().join("state"), NAME);
        hb.cycle();
        hb.error("bad \"thing\"".into());
        hb.write().unwrap();
        let j = std::fs::read_to_string(hb.path()).unwrap();
        assert!(j.starts_with("{\"watcher\":\"forge-md-revs\",\"version\":\""), "{j}");
        assert!(j.contains("\"interval_secs\":0,\"started_at\":\""), "{j}");
        assert!(j.contains("\"last_action\":null,\"actions\":0,\"last_error\":\"bad \\\"thing\\\"\",\"host\":\""), "{j}");
        hb.action();
        hb.write().unwrap();
        let j = std::fs::read_to_string(hb.path()).unwrap();
        assert!(j.contains("\"actions\":1,\"last_error\":null"), "{j}");
        assert!(std::fs::read_dir(d.path().join("state")).unwrap().count() == 1, "no tmp left behind");
    }

    /// Real watcher on a tempdir: a write turns into a snapshot.
    #[test]
    fn watcher_snapshots_a_write() {
        let (d, s) = setup();
        let stop = Arc::new(AtomicBool::new(false));
        let state = d.path().join("state");
        let store = Arc::new(Store { forge: s.forge.clone(), root: s.root.clone() });
        let handle = {
            let (store, stop, state) = (store.clone(), stop.clone(), state.clone());
            std::thread::spawn(move || {
                let mut hb = Heartbeat::new(state, NAME);
                run_watch(&store, &Logger { path: None }, &mut hb, &stop)
            })
        };
        std::thread::sleep(Duration::from_millis(500)); // watcher registration
        let note = s.forge.join("NapierianLogs/DayPages/2026-09-02.md");
        std::fs::write(&note, "hello").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut revs = vec![];
        while Instant::now() < deadline {
            revs = store.list_one(&note).unwrap();
            if !revs.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(revs.len(), 1, "no snapshot within timeout");
        assert_eq!(std::fs::read_to_string(&revs[0].file).unwrap(), "hello");
        let hb = std::fs::read_to_string(state.join("forge-md-revs.json")).unwrap();
        assert!(hb.contains("\"actions\":1"), "{hb}");
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap().unwrap();
    }
}
