//! logkeep — cap a tool's own log at N bytes, keeping one predecessor.
//!
//! Audit row D2-24 (SYSTEM-REVIEW-2026-08-01-DOMAIN2-SILENT-FAILURE.md): no
//! log rotation on either machine — a 597 MB stderr log on the Mac, 218 MB of
//! duplicated watcher logs and a 52 MB `gmpull.log` on nimbini. This crate is
//! the shared answer, a path dependency like `forge-names`:
//!
//! ```toml
//! logkeep = { path = "../logkeep" }
//! ```
//!
//! Two strategies, chosen by who holds the file open:
//!
//! * [`cap`] — **rename**. For a log the tool itself opens by path each run
//!   (timer-run oneshots, or a watcher that reopens per line). When the file
//!   exceeds the cap it is renamed to `<log>.1`, replacing any earlier
//!   predecessor, and the next append starts a fresh file. Nothing is copied.
//! * [`cap_in_place`] — **copy then truncate**. For a log a supervisor holds
//!   open for the tool's whole life (`StandardOutPath` / `StandardOutput=append:`).
//!   Renaming such a file frees nothing — the supervisor keeps writing to the
//!   renamed inode until the unit restarts — so the content is copied to
//!   `<log>.1` and the original truncated to zero. An `O_APPEND` writer
//!   continues at the new end of file; the tail of the copy and the head of
//!   the new file may overlap by whatever was written between copy and
//!   truncate (same trade-off as logrotate's `copytruncate`).
//!
//! Both are idempotent, never fail on a missing log, and keep exactly one
//! predecessor. Callers treat the result as advisory: a log that could not be
//! capped is worth a log line, never a reason to skip the tool's real work.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// One mebibyte, so call sites read `logkeep::cap(&log, 5 * logkeep::MB)`.
pub const MB: u64 = 1024 * 1024;

/// What `cap` / `cap_in_place` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The log does not exist yet; nothing to do.
    Missing,
    /// The log is at or under the cap; untouched. Carries its size in bytes.
    Kept(u64),
    /// The log exceeded the cap and was rolled to the predecessor. Carries the
    /// size in bytes it had before rolling.
    Rolled(u64),
}

impl Outcome {
    pub fn rolled(&self) -> bool {
        matches!(self, Outcome::Rolled(_))
    }
}

/// The predecessor path: `<log>.1` (the extension is appended, not replaced,
/// so `foo.log` keeps `foo.log.1` and readers grepping `*.log` still find it).
pub fn predecessor(log: &Path) -> PathBuf {
    let mut s = log.as_os_str().to_owned();
    s.push(".1");
    PathBuf::from(s)
}

fn size_of(log: &Path) -> io::Result<Option<u64>> {
    match fs::metadata(log) {
        Ok(m) if m.is_file() => Ok(Some(m.len())),
        Ok(_) => Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} is not a regular file", log.display()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Rename strategy. If `log` is larger than `max_bytes`, move it to
/// [`predecessor`] (replacing any existing predecessor) so the caller's next
/// open creates a fresh file. Use when the caller opens the log by path.
pub fn cap(log: &Path, max_bytes: u64) -> io::Result<Outcome> {
    let Some(size) = size_of(log)? else { return Ok(Outcome::Missing) };
    if size <= max_bytes {
        return Ok(Outcome::Kept(size));
    }
    fs::rename(log, predecessor(log))?;
    Ok(Outcome::Rolled(size))
}

/// Copy-then-truncate strategy. If `log` is larger than `max_bytes`, copy it
/// to [`predecessor`] (replacing any existing predecessor) and truncate the
/// original to zero length in place, so a writer that already holds it open
/// in append mode carries on into the emptied file. Use when a supervisor
/// (launchd `StandardOutPath`, systemd `StandardOutput=append:`) owns the
/// handle for the tool's lifetime.
pub fn cap_in_place(log: &Path, max_bytes: u64) -> io::Result<Outcome> {
    let Some(size) = size_of(log)? else { return Ok(Outcome::Missing) };
    if size <= max_bytes {
        return Ok(Outcome::Kept(size));
    }
    fs::copy(log, predecessor(log))?;
    fs::OpenOptions::new().write(true).truncate(true).open(log)?;
    Ok(Outcome::Rolled(size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn log_in(dir: &Path, bytes: usize) -> PathBuf {
        let p = dir.join("tool.log");
        fs::write(&p, vec![b'x'; bytes]).unwrap();
        p
    }

    #[test]
    fn predecessor_appends_dot_one() {
        assert_eq!(predecessor(Path::new("/var/log/tool.log")), PathBuf::from("/var/log/tool.log.1"));
        assert_eq!(predecessor(Path::new("gmpull")), PathBuf::from("gmpull.1"));
    }

    #[test]
    fn missing_log_is_not_an_error() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("never.log");
        assert_eq!(cap(&p, 10).unwrap(), Outcome::Missing);
        assert_eq!(cap_in_place(&p, 10).unwrap(), Outcome::Missing);
        assert!(!predecessor(&p).exists());
    }

    #[test]
    fn under_cap_is_kept_untouched_green_control() {
        let d = tempfile::tempdir().unwrap();
        let p = log_in(d.path(), 100);
        assert_eq!(cap(&p, 100).unwrap(), Outcome::Kept(100), "at the cap counts as under");
        assert_eq!(cap_in_place(&p, 1000).unwrap(), Outcome::Kept(100));
        assert_eq!(fs::metadata(&p).unwrap().len(), 100);
        assert!(!predecessor(&p).exists());
    }

    #[test]
    fn over_cap_is_rolled_red_control() {
        let d = tempfile::tempdir().unwrap();
        let p = log_in(d.path(), 101);
        let r = cap(&p, 100).unwrap();
        assert_eq!(r, Outcome::Rolled(101));
        assert!(r.rolled());
        assert!(!p.exists(), "rename strategy leaves no live file");
        assert_eq!(fs::metadata(predecessor(&p)).unwrap().len(), 101);
    }

    #[test]
    fn exactly_one_predecessor_is_kept() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("tool.log");
        fs::write(&p, b"first-generation").unwrap();
        assert!(cap(&p, 1).unwrap().rolled());
        fs::write(&p, b"second-generation!!").unwrap();
        assert!(cap(&p, 1).unwrap().rolled());
        assert_eq!(fs::read(predecessor(&p)).unwrap(), b"second-generation!!");
        assert!(!predecessor(&predecessor(&p)).exists(), "no .1.1 chain");
    }

    #[test]
    fn in_place_keeps_an_open_append_writer_working() {
        let d = tempfile::tempdir().unwrap();
        let p = log_in(d.path(), 50);
        // The "supervisor": an append handle opened before the cap, as launchd
        // or systemd would hold it.
        let mut supervisor = fs::OpenOptions::new().append(true).open(&p).unwrap();
        let r = cap_in_place(&p, 10).unwrap();
        assert_eq!(r, Outcome::Rolled(50));
        assert_eq!(fs::metadata(&p).unwrap().len(), 0, "truncated in place");
        assert_eq!(fs::metadata(predecessor(&p)).unwrap().len(), 50, "content copied");
        supervisor.write_all(b"after").unwrap();
        supervisor.flush().unwrap();
        let mut s = String::new();
        fs::File::open(&p).unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "after", "the held handle writes into the emptied file, not past a 50-byte hole");
    }

    #[test]
    fn a_directory_at_the_log_path_is_an_error_not_a_pass() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("dir.log");
        fs::create_dir(&p).unwrap();
        assert!(cap(&p, 10).is_err());
        assert!(cap_in_place(&p, 10).is_err());
    }
}
