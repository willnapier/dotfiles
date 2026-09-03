//! `zotero-watcher pdf` — port of `scripts/zotero-pdf-watcher-renu`.
//!
//! The oracle watches `~/Documents/ZoteroImport` for `*.pdf`, sleeps the
//! debounce, moves the file to `~/Documents/ProcessedPDFs`, prints
//! "📚 Ready for Zotero import" and notifies. It never invokes
//! `zotero-import-now` (that is a clipboard helper for a manual Zotero step);
//! `--import-cmd` is an opt-in hook that runs `<cmd> <processed path>` after
//! the move.
//!
//! Differences from the oracle:
//! - The watch directory is canonicalised first (on the Mac it is a symlink
//!   into Dropbox; FSEvents reports the real path).
//! - Real debounce: a burst of writes to one file yields one import, timed
//!   from the last write. The oracle slept per event and re-processed.
//! - `.PDF` matches too.

use crate::common::{display_name, is_pdf, move_file, notify, Heartbeat, Logger, Moved};
use anyhow::{Context, Result};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

pub struct PdfCfg {
    pub watch_dir: PathBuf,
    pub output_dir: PathBuf,
    pub debounce: Duration,
    pub import_cmd: Option<PathBuf>,
    pub notify: bool,
}

pub fn run(mut cfg: PdfCfg, logger: &Logger, hb: &mut Heartbeat) -> Result<()> {
    logger.log(&format!("🔍 Zotero PDF Watcher (zotero-watcher {})", env!("CARGO_PKG_VERSION")));
    if !cfg.watch_dir.exists() {
        logger.log(&format!("❌ Watch directory does not exist: {}", cfg.watch_dir.display()));
        std::fs::create_dir_all(&cfg.watch_dir).with_context(|| format!("creating {}", cfg.watch_dir.display()))?;
        logger.log(&format!("✅ Created watch directory: {}", cfg.watch_dir.display()));
    }
    if !cfg.output_dir.exists() {
        std::fs::create_dir_all(&cfg.output_dir).with_context(|| format!("creating {}", cfg.output_dir.display()))?;
        logger.log(&format!("✅ Created output directory: {}", cfg.output_dir.display()));
    }
    cfg.watch_dir = std::fs::canonicalize(&cfg.watch_dir).with_context(|| format!("resolving {}", cfg.watch_dir.display()))?;
    cfg.output_dir = std::fs::canonicalize(&cfg.output_dir).with_context(|| format!("resolving {}", cfg.output_dir.display()))?;
    logger.log(&format!("📁 Watching: {}", cfg.watch_dir.display()));
    logger.log(&format!("📤 Output: {}", cfg.output_dir.display()));
    logger.log(&format!("⏱️  Debounce: {}ms", cfg.debounce.as_millis()));

    let (tx, rx) = channel();
    let mut watcher = RecommendedWatcher::new(
        move |res: std::result::Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        Config::default().with_poll_interval(Duration::from_secs(2)),
    )?;
    watcher.watch(&cfg.watch_dir, RecursiveMode::NonRecursive)?;
    logger.log("👀 Watching for PDF files... (Press Ctrl+C to stop)");
    hb.write();
    run_events(rx, &cfg, logger, hb)
}

/// Event loop. Each Create/Modify of an existing `*.pdf` (re)arms that
/// path's deadline; a path is processed once its deadline has passed.
/// Returns when the sender is gone and nothing is pending (tests).
pub fn run_events(rx: Receiver<Event>, cfg: &PdfCfg, logger: &Logger, hb: &mut Heartbeat) -> Result<()> {
    let mut pending: HashMap<PathBuf, Instant> = HashMap::new();
    loop {
        let wait = pending.values().min().map(|d| d.saturating_duration_since(Instant::now())).unwrap_or(Duration::from_secs(3600));
        match rx.recv_timeout(wait) {
            Ok(ev) => {
                if matches!(ev.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                    for p in ev.paths {
                        if !is_pdf(&p) || !p.is_file() {
                            continue;
                        }
                        if !pending.contains_key(&p) {
                            logger.log(&format!("📄 New PDF detected: {}", display_name(&p)));
                        }
                        pending.insert(p, Instant::now() + cfg.debounce);
                    }
                    hb.cycle();
                    hb.write();
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if pending.is_empty() {
                    return Ok(());
                }
                std::thread::sleep(wait);
            }
        }
        let now = Instant::now();
        let mut due: Vec<PathBuf> = pending.iter().filter(|(_, d)| **d <= now).map(|(p, _)| p.clone()).collect();
        due.sort();
        for p in due {
            pending.remove(&p);
            if !p.is_file() {
                continue;
            }
            hb.cycle();
            match process_pdf(&p, cfg, logger) {
                Ok(()) => hb.action(),
                Err(e) => {
                    let msg = format!("❌ Error processing {}: {e:#}", display_name(&p));
                    logger.log(&msg);
                    hb.error(&msg);
                }
            }
            hb.write();
        }
    }
}

/// Move one PDF into the output directory, run the import hook, notify.
pub fn process_pdf(path: &Path, cfg: &PdfCfg, logger: &Logger) -> Result<()> {
    // Join the OsStr the OS gave us, not the NFC display name: the file must
    // land under the same bytes it arrived with.
    let output_path = cfg.output_dir.join(path.file_name().context("no file name")?);
    let filename = display_name(path);
    logger.log(&format!("🔄 Processing: {filename}"));
    match move_file(path, &output_path)? {
        Moved::Renamed => {}
        Moved::Copied(why) => logger.log(&format!("ℹ️ Copied rather than moved ({why})")),
    }
    logger.log(&format!("✅ Moved to processed: {filename}"));
    logger.log(&format!("📚 Ready for Zotero import: {}", output_path.display()));

    if let Some(cmd) = &cfg.import_cmd {
        let o = Command::new(cmd).arg(&output_path).output().with_context(|| format!("running {}", cmd.display()))?;
        if o.status.success() {
            logger.log(&format!("📥 Import command finished: {}", cmd.display()));
        } else {
            anyhow::bail!("{} exited {}: {}", cmd.display(), o.status, String::from_utf8_lossy(&o.stderr).trim());
        }
    }

    if cfg.notify {
        if let Some(method) = notify("Zotero PDF Watcher", &format!("New PDF ready: {filename}")) {
            logger.log(&format!("🔔 Notification sent via {method}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, ModifyKind};
    use std::os::unix::fs::PermissionsExt;

    struct Fixture {
        _d: tempfile::TempDir,
        cfg: PdfCfg,
        argv_log: PathBuf,
        logger: Logger,
        hb: Heartbeat,
    }

    /// Tempdir with watch/, out/, a fake import command that appends its argv
    /// to argv.log, and notifications off.
    fn fixture(debounce: Duration) -> Fixture {
        let d = tempfile::tempdir().unwrap();
        let watch = d.path().join("watch");
        let out = d.path().join("out");
        std::fs::create_dir_all(&watch).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        let argv_log = d.path().join("argv.log");
        let script = d.path().join("fake-import");
        std::fs::write(&script, format!("#!/bin/sh\necho \"$@\" >> '{}'\n", argv_log.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cfg = PdfCfg { watch_dir: watch, output_dir: out, debounce, import_cmd: Some(script), notify: false };
        let logger = Logger::new(d.path().join("log"));
        let hb = Heartbeat::new(&d.path().join("state"), "pdf", 0);
        Fixture { _d: d, cfg, argv_log, logger, hb }
    }

    fn create(p: &Path) -> Event {
        Event::new(EventKind::Create(CreateKind::File)).add_path(p.to_path_buf())
    }
    fn modify(p: &Path) -> Event {
        Event::new(EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Content))).add_path(p.to_path_buf())
    }

    #[test]
    fn imports_a_pdf_and_records_argv() {
        let mut f = fixture(Duration::from_millis(50));
        let pdf = f.cfg.watch_dir.join("paper.pdf");
        std::fs::write(&pdf, b"%PDF-1.4").unwrap();
        let (tx, rx) = channel();
        tx.send(create(&pdf)).unwrap();
        drop(tx);
        run_events(rx, &f.cfg, &f.logger, &mut f.hb).unwrap();

        let moved = f.cfg.output_dir.join("paper.pdf");
        assert!(moved.exists() && !pdf.exists());
        let argv = std::fs::read_to_string(&f.argv_log).unwrap();
        assert_eq!(argv.trim(), moved.display().to_string());
        assert_eq!(f.hb.actions, 1);
        let log = std::fs::read_to_string(f._d.path().join("log")).unwrap();
        for phrase in ["📄 New PDF detected: paper.pdf", "🔄 Processing: paper.pdf", "✅ Moved to processed: paper.pdf", "📚 Ready for Zotero import: "] {
            assert!(log.contains(phrase), "missing {phrase:?} in {log}");
        }
    }

    /// The output path is joined from the OsStr the OS gave us: an NFD name
    /// lands under the same bytes it arrived with, while the log is NFC.
    #[test]
    fn nfd_named_pdf_lands_under_its_own_bytes() {
        use std::os::unix::ffi::OsStrExt;
        let mut f = fixture(Duration::from_millis(20));
        let nfd = "Hanwell Cafe\u{0301}.pdf";
        let nfc = "Hanwell Café.pdf";
        assert_ne!(nfd, nfc);
        let pdf = f.cfg.watch_dir.join(nfd);
        std::fs::write(&pdf, b"%PDF").unwrap();
        let (tx, rx) = channel();
        tx.send(create(&pdf)).unwrap();
        drop(tx);
        run_events(rx, &f.cfg, &f.logger, &mut f.hb).unwrap();

        let listed: Vec<Vec<u8>> = std::fs::read_dir(&f.cfg.output_dir).unwrap().map(|e| e.unwrap().file_name().as_bytes().to_vec()).collect();
        assert_eq!(listed, vec![nfd.as_bytes().to_vec()], "moved under the original bytes, no NFC twin");
        assert!(std::fs::read_dir(&f.cfg.watch_dir).unwrap().next().is_none());
        assert_eq!(f.hb.actions, 1);
        let log = std::fs::read_to_string(f._d.path().join("log")).unwrap();
        for prefix in ["📄 New PDF detected: ", "🔄 Processing: ", "✅ Moved to processed: "] {
            assert!(log.contains(&format!("{prefix}{nfc}")), "missing NFC line {prefix:?} in {log}");
            assert!(!log.contains(&format!("{prefix}{nfd}")), "NFD leaked into a name line {prefix:?}: {log}");
        }
        // the path line is an I/O identity and keeps the OS's bytes
        assert!(log.contains(&format!("📚 Ready for Zotero import: {}", f.cfg.output_dir.join(nfd).display())), "{log}");
    }

    #[test]
    fn non_pdf_is_ignored() {
        let mut f = fixture(Duration::from_millis(20));
        let txt = f.cfg.watch_dir.join("notes.txt");
        let part = f.cfg.watch_dir.join("paper.pdf.part");
        std::fs::write(&txt, b"hi").unwrap();
        std::fs::write(&part, b"hi").unwrap();
        let (tx, rx) = channel();
        tx.send(create(&txt)).unwrap();
        tx.send(modify(&part)).unwrap();
        // a pdf path that no longer exists (rename-out echo) is ignored too
        tx.send(modify(&f.cfg.watch_dir.join("gone.pdf"))).unwrap();
        drop(tx);
        run_events(rx, &f.cfg, &f.logger, &mut f.hb).unwrap();
        assert!(txt.exists() && part.exists());
        assert!(!f.argv_log.exists());
        assert_eq!(f.hb.actions, 0);
    }

    #[test]
    fn burst_of_writes_is_debounced_to_one_import() {
        let mut f = fixture(Duration::from_millis(300));
        let pdf = f.cfg.watch_dir.join("big.pdf");
        std::fs::write(&pdf, b"%PDF").unwrap();
        let (tx, rx) = channel();
        let start = Instant::now();
        tx.send(create(&pdf)).unwrap();
        for _ in 0..5 {
            std::thread::sleep(Duration::from_millis(40));
            tx.send(modify(&pdf)).unwrap();
        }
        let last_write = Instant::now();
        drop(tx);
        run_events(rx, &f.cfg, &f.logger, &mut f.hb).unwrap();
        let done = Instant::now();
        // exactly one import, and not before the debounce measured from the last write
        let argv = std::fs::read_to_string(&f.argv_log).unwrap();
        assert_eq!(argv.lines().count(), 1, "{argv}");
        assert!(done.duration_since(last_write) >= Duration::from_millis(250), "processed too early: {:?} after last write", done - last_write);
        assert!(start.elapsed() < Duration::from_secs(5));
        assert_eq!(f.hb.actions, 1);
        let log = std::fs::read_to_string(f._d.path().join("log")).unwrap();
        assert_eq!(log.matches("📄 New PDF detected").count(), 1, "{log}");
    }

    #[test]
    fn failing_import_cmd_is_reported_and_counted_as_error() {
        let mut f = fixture(Duration::from_millis(20));
        let bad = f._d.path().join("bad-import");
        std::fs::write(&bad, "#!/bin/sh\necho nope >&2\nexit 3\n").unwrap();
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o755)).unwrap();
        f.cfg.import_cmd = Some(bad);
        let pdf = f.cfg.watch_dir.join("p.pdf");
        std::fs::write(&pdf, b"%PDF").unwrap();
        let (tx, rx) = channel();
        tx.send(create(&pdf)).unwrap();
        drop(tx);
        run_events(rx, &f.cfg, &f.logger, &mut f.hb).unwrap();
        assert!(f.cfg.output_dir.join("p.pdf").exists(), "the move itself still happens");
        assert_eq!(f.hb.actions, 0);
        let hb = std::fs::read_to_string(f._d.path().join("state/zotero-watcher-pdf.json")).unwrap();
        assert!(hb.contains("\"last_error\":\"❌ Error processing p.pdf: "), "{hb}");
        assert!(hb.contains("nope"), "{hb}");
    }
}
