//! `zotero-watcher bridge` — port of `scripts/zotero-bridge-renu`.
//!
//! Every `--interval` seconds, take the oldest `--batch-size` `*.pdf` files
//! in the source directory (Dropbox ZoteroImport) and move each into the
//! destination (Documents ZoteroImport):
//! - destination file absent → move, notify;
//! - present with the same size → the source copy is a duplicate, remove it;
//! - present with a different size → move as `<stem>_<YYYYmmdd_HHMMSS>.pdf`;
//! - move refused ("Operation not permitted" etc.) → copy, verify size,
//!   remove source; on a size mismatch keep both.
//! `--once` is the oracle's `main sync-now`: one pass, no batch cap.
//!
//! The oracle's `log_transfer_event` was a no-op (its save line was commented
//! out); this port has no analytics log either. Difference: `.PDF` matches.

use crate::common::{file_name, is_pdf, move_file, notify, Heartbeat, Logger, MoveError, Moved};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct BridgeCfg {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub interval: Duration,
    pub batch_size: usize,
    pub notify: bool,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Transferred(String),
    RenamedTransfer(String),
    DuplicateRemoved,
    CopiedInstead,
    CopyVerifyFailed,
    Failed(String),
}
impl Outcome {
    fn is_action(&self) -> bool {
        matches!(self, Outcome::Transferred(_) | Outcome::RenamedTransfer(_) | Outcome::CopiedInstead)
    }
}

pub fn run(cfg: &BridgeCfg, logger: &Logger, hb: &mut Heartbeat) -> Result<()> {
    logger.log(&format!("🌉 Zotero Import Bridge — zotero-watcher {}", env!("CARGO_PKG_VERSION")));
    logger.log(&format!("📂 Source: {}", cfg.source.display()));
    logger.log(&format!("📁 Destination: {}", cfg.destination.display()));
    logger.log(&format!("⏱️  Check interval: {}s", cfg.interval.as_secs()));
    logger.log(&format!("📦 Batch size: {} files", cfg.batch_size));
    ensure_directory_exists(&cfg.source, "source", logger)?;
    ensure_directory_exists(&cfg.destination, "destination", logger)?;
    logger.log("🔄 Starting import bridge... Press Ctrl+C to stop");
    hb.write();
    loop {
        let found = sweep(cfg, Some(cfg.batch_size), logger, hb)?;
        if !found.is_empty() {
            logger.log(&format!("📄 Found {} PDFs to process", found.len()));
            for (name, o) in &found {
                if let Outcome::Failed(e) = o {
                    hb.error(&format!("{name}: {e}"));
                }
            }
        }
        std::thread::sleep(cfg.interval);
    }
}

/// The oracle's `sync-now`: one pass over every PDF, then exit.
pub fn sync_now(cfg: &BridgeCfg, logger: &Logger, hb: &mut Heartbeat) -> Result<()> {
    logger.log("🔄 Running one-time sync...");
    ensure_directory_exists(&cfg.source, "source", logger)?;
    ensure_directory_exists(&cfg.destination, "destination", logger)?;
    hb.write();
    let pdfs = source_pdfs(&cfg.source, None)?;
    if pdfs.is_empty() {
        logger.log("📂 No PDFs found to transfer");
        hb.cycle();
        hb.write();
        return Ok(());
    }
    logger.log(&format!("📄 Found {} PDFs to transfer", pdfs.len()));
    let found = sweep(cfg, None, logger, hb)?;
    logger.log("✅ Sync complete!");
    if found.iter().any(|(_, o)| matches!(o, Outcome::Failed(_))) {
        std::process::exit(1);
    }
    Ok(())
}

fn ensure_directory_exists(dir: &Path, kind: &str, logger: &Logger) -> Result<()> {
    if dir.exists() {
        return Ok(());
    }
    match std::fs::create_dir_all(dir) {
        Ok(()) => {
            logger.log(&format!("✅ Created {kind} directory: {}", dir.display()));
            Ok(())
        }
        Err(e) => {
            logger.log(&format!("❌ Cannot create {kind} directory: {e}"));
            std::process::exit(1);
        }
    }
}

/// `*.pdf` regular files in `dir`, oldest modification first, optionally capped.
pub fn source_pdfs(dir: &Path, cap: Option<usize>) -> Result<Vec<PathBuf>> {
    let mut v: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_pdf(p))
        .map(|p| {
            let m = std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            (m, p)
        })
        .collect();
    v.sort();
    Ok(v.into_iter().map(|(_, p)| p).take(cap.unwrap_or(usize::MAX)).collect())
}

/// One pass: transfer up to `cap` PDFs; heartbeat cycled once and written.
pub fn sweep(cfg: &BridgeCfg, cap: Option<usize>, logger: &Logger, hb: &mut Heartbeat) -> Result<Vec<(String, Outcome)>> {
    let pdfs = source_pdfs(&cfg.source, cap)?;
    let mut out = Vec::with_capacity(pdfs.len());
    hb.cycle();
    for p in pdfs {
        let o = process_pdf_transfer(&p, &cfg.destination, cfg.notify, logger);
        if o.is_action() {
            hb.action();
        }
        out.push((file_name(&p), o));
        hb.write();
    }
    if out.is_empty() {
        hb.write();
    }
    Ok(out)
}

pub fn process_pdf_transfer(source_file: &Path, dest_dir: &Path, do_notify: bool, logger: &Logger) -> Outcome {
    let filename = file_name(source_file);
    let dest_file = dest_dir.join(&filename);
    logger.log(&format!("🔄 Processing: {filename}"));

    if dest_file.exists() {
        logger.log(&format!("⚠️  File already exists in destination: {filename}"));
        let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
        if size(source_file).is_some() && size(source_file) == size(&dest_file) {
            logger.log(&format!("🗑️  Removing duplicate from source: {filename}"));
            return match std::fs::remove_file(source_file) {
                Ok(()) => Outcome::DuplicateRemoved,
                Err(e) => {
                    logger.log(&format!("❌ Failed to transfer {filename}: {e}"));
                    Outcome::Failed(e.to_string())
                }
            };
        }
        let new_name = timestamped_name(&filename, &chrono::Local::now().format("%Y%m%d_%H%M%S").to_string());
        let new_dest = dest_dir.join(&new_name);
        return match move_file(source_file, &new_dest) {
            Ok(_) => {
                logger.log(&format!("✅ Transferred with new name: {new_name}"));
                Outcome::RenamedTransfer(new_name)
            }
            Err(e) => {
                logger.log(&format!("❌ Failed to transfer {filename}: {e}"));
                Outcome::Failed(e.to_string())
            }
        };
    }

    match move_file(source_file, &dest_file) {
        Ok(Moved::Renamed) => {
            logger.log(&format!("✅ Transferred: {filename}"));
            if do_notify {
                notify("Zotero Bridge", &format!("PDF ready for import: {filename}"));
            }
            Outcome::Transferred(filename)
        }
        Ok(Moved::Copied(rename_err)) => {
            logger.log(&format!("❌ Failed to transfer {filename}: {rename_err}"));
            logger.log(&format!("✅ Copied instead (permissions issue): {filename}"));
            logger.log("🗑️  Removed source after successful copy");
            Outcome::CopiedInstead
        }
        Err(MoveError::VerifyFailed(rename_err)) => {
            logger.log(&format!("❌ Failed to transfer {filename}: {rename_err}"));
            logger.log(&format!("✅ Copied instead (permissions issue): {filename}"));
            logger.log("⚠️  Copy verification failed - keeping both files");
            Outcome::CopyVerifyFailed
        }
        Err(MoveError::CopyFailed(rename_err, copy_err)) => {
            logger.log(&format!("❌ Failed to transfer {filename}: {rename_err}"));
            logger.log(&format!("❌ Copy also failed: {copy_err}"));
            Outcome::Failed(format!("{rename_err}; {copy_err}"))
        }
    }
}

/// `paper.pdf` + `20260902_101500` → `paper_20260902_101500.pdf`
/// (the oracle's `str replace '.pdf'`: first occurrence; no `.pdf` → suffix appended).
pub fn timestamped_name(filename: &str, ts: &str) -> String {
    match filename.find(".pdf") {
        Some(i) => format!("{}_{ts}.pdf{}", &filename[..i], &filename[i + 4..]),
        None => format!("{filename}_{ts}.pdf"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, BridgeCfg, Logger, Heartbeat) {
        let d = tempfile::tempdir().unwrap();
        let cfg = BridgeCfg { source: d.path().join("src"), destination: d.path().join("dst"), interval: Duration::from_secs(1), batch_size: 5, notify: false };
        std::fs::create_dir_all(&cfg.source).unwrap();
        std::fs::create_dir_all(&cfg.destination).unwrap();
        let logger = Logger::new(d.path().join("log"));
        let hb = Heartbeat::new(&d.path().join("state"), "bridge", 1);
        (d, cfg, logger, hb)
    }

    #[test]
    fn round_trip_move_duplicate_and_rename_rules() {
        let (d, cfg, logger, mut hb) = fixture();
        std::fs::write(cfg.source.join("new.pdf"), b"fresh").unwrap();
        std::fs::write(cfg.source.join("dup.pdf"), b"same!").unwrap();
        std::fs::write(cfg.destination.join("dup.pdf"), b"same!").unwrap();
        std::fs::write(cfg.source.join("clash.pdf"), b"longer content").unwrap();
        std::fs::write(cfg.destination.join("clash.pdf"), b"short").unwrap();
        std::fs::write(cfg.source.join("notes.txt"), b"ignored").unwrap();

        let out = sweep(&cfg, Some(cfg.batch_size), &logger, &mut hb).unwrap();
        let get = |n: &str| out.iter().find(|(f, _)| f == n).map(|(_, o)| o).unwrap();
        assert_eq!(out.len(), 3, "{out:?}");
        assert_eq!(get("new.pdf"), &Outcome::Transferred("new.pdf".into()));
        assert_eq!(get("dup.pdf"), &Outcome::DuplicateRemoved);
        match get("clash.pdf") {
            Outcome::RenamedTransfer(n) => {
                assert!(n.starts_with("clash_") && n.ends_with(".pdf") && n.len() == "clash_20260902_101500.pdf".len(), "{n}");
                assert_eq!(std::fs::read(cfg.destination.join(n)).unwrap(), b"longer content");
            }
            o => panic!("{o:?}"),
        }
        assert_eq!(std::fs::read(cfg.destination.join("new.pdf")).unwrap(), b"fresh");
        assert_eq!(std::fs::read(cfg.destination.join("dup.pdf")).unwrap(), b"same!");
        assert_eq!(std::fs::read(cfg.destination.join("clash.pdf")).unwrap(), b"short", "existing file untouched");
        assert!(!cfg.source.join("new.pdf").exists() && !cfg.source.join("dup.pdf").exists() && !cfg.source.join("clash.pdf").exists());
        assert!(cfg.source.join("notes.txt").exists());
        assert_eq!(hb.actions, 2, "duplicate removal is not an action");

        let log = std::fs::read_to_string(d.path().join("log")).unwrap();
        for phrase in ["🔄 Processing: new.pdf", "✅ Transferred: new.pdf", "⚠️  File already exists in destination: dup.pdf", "🗑️  Removing duplicate from source: dup.pdf", "✅ Transferred with new name: clash_"] {
            assert!(log.contains(phrase), "missing {phrase:?} in {log}");
        }
        let hb_json = std::fs::read_to_string(d.path().join("state/zotero-watcher-bridge.json")).unwrap();
        assert!(hb_json.contains("\"watcher\":\"zotero-watcher-bridge\"") && hb_json.contains("\"actions\":2"), "{hb_json}");

        assert!(sweep(&cfg, Some(5), &logger, &mut hb).unwrap().is_empty());
    }

    #[test]
    fn batch_cap_takes_oldest_first() {
        let (_d, cfg, logger, mut hb) = fixture();
        for (i, n) in ["c.pdf", "a.pdf", "b.pdf"].iter().enumerate() {
            let p = cfg.source.join(n);
            std::fs::write(&p, n).unwrap();
            let t = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + i as u64 * 60);
            std::fs::File::open(&p).unwrap().set_modified(t).unwrap();
        }
        let out = sweep(&cfg, Some(2), &logger, &mut hb).unwrap();
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["c.pdf", "a.pdf"]);
        assert!(cfg.source.join("b.pdf").exists());
        assert_eq!(sweep(&cfg, None, &logger, &mut hb).unwrap().len(), 1);
    }

    #[test]
    fn timestamp_rename_matches_oracle() {
        assert_eq!(timestamped_name("paper.pdf", "20260902_101500"), "paper_20260902_101500.pdf");
        assert_eq!(timestamped_name("a.pdf.pdf", "T"), "a_T.pdf.pdf");
        assert_eq!(timestamped_name("weird.PDF", "T"), "weird.PDF_T.pdf");
    }
}
