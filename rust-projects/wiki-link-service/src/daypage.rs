//! DayPages are edited in Helix. Queue import transforms stdin, never the page
//! on disk; acknowledgement only removes instructions already present on disk.
//! Failed/conflicting saves therefore leave the queue available for retry.
use crate::{
    resolve,
    wiki::{self, Index},
};
use anyhow::{bail, Context, Result};
use fs2::FileExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

pub fn is_daypage(path: &Path) -> bool {
    let Some(parent) = path.parent() else { return false };
    parent.file_name().is_some_and(|n| n == "DayPages")
        && parent.parent().and_then(Path::file_name).is_some_and(|n| n == "NapierianLogs")
        && path.extension().is_some_and(|e| e == "md")
}

fn valid_date(date: &str) -> Result<()> {
    let parsed = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")?;
    if parsed.format("%Y-%m-%d").to_string() != date {
        bail!("expected YYYY-MM-DD");
    }
    Ok(())
}

fn private_dir(path: &Path) -> Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(path)?;
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

pub struct Store {
    pub pages: PathBuf,
    pub pending: PathBuf,
    pub recovery: PathBuf,
}

impl Store {
    pub fn local() -> Self {
        let home = wiki::home();
        Self {
            pages: home.join("Forge/NapierianLogs/DayPages"),
            pending: home.join(".local/share/daypage-pending"),
            recovery: home.join(".local/share/daypage-recovery"),
        }
    }

    fn lock(&self) -> Result<File> {
        private_dir(&self.pending)?;
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.pending.join(".queue.lock"))?;
        for _ in 0..100 {
            match f.try_lock_exclusive() {
                Ok(()) => return Ok(f),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(e.into()),
            }
        }
        bail!("DayPage queue busy; retry shortly")
    }

    fn read_pending(&self, date: &str) -> Result<String> {
        match fs::read_to_string(self.pending.join(format!("{date}.md"))) {
            Ok(s) => Ok(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(e.into()),
        }
    }

    // The lock serialises every current-date producer and acknowledgement.
    // Rename avoids losing queued bytes if interrupted during a rewrite.
    fn replace_pending(&self, date: &str, text: &str) -> Result<()> {
        let tmp = self.pending.join(format!(".{date}.{}.tmp", std::process::id()));
        // A predecessor from a crashed process is not overwritten.
        write_private(&tmp, text.as_bytes())?;
        fs::rename(&tmp, self.pending.join(format!("{date}.md")))?;
        File::open(&self.pending)?.sync_all()?;
        Ok(())
    }

    pub fn queue(&self, date: &str, entry: &str) -> Result<()> {
        valid_date(date)?;
        let entry = entry.trim();
        if entry.is_empty() || entry.contains(['\n', '\r']) {
            bail!("queue one nonempty line at a time");
        }
        let _lock = self.lock()?;
        let pending = self.read_pending(date)?;
        let disk = match fs::read_to_string(self.pages.join(format!("{date}.md"))) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        if applied(&disk, entry) || pending.lines().any(|l| l.trim() == entry) {
            return Ok(());
        }
        let updated = format!(
            "{}{entry}\n",
            if pending.is_empty() || pending.ends_with('\n') {
                pending
            } else {
                format!("{pending}\n")
            }
        );
        self.replace_pending(date, &updated)
    }

    /// Acknowledge only instructions demonstrably persisted. Never write a DayPage.
    pub fn acknowledge(&self) -> Result<usize> {
        if !self.pending.exists() {
            return Ok(0);
        }
        let _lock = self.lock()?;
        let mut remaining = 0;
        for entry in fs::read_dir(&self.pending)? {
            let path = entry?.path();
            if path.extension().is_none_or(|e| e != "md") {
                continue;
            }
            let date = path.file_stem().and_then(|s| s.to_str()).context("invalid queue filename")?;
            valid_date(date)?;
            let queued = fs::read_to_string(&path)?;
            let disk = match fs::read_to_string(self.pages.join(format!("{date}.md"))) {
                Ok(s) => s,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    remaining += queued.lines().count();
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let kept = unpersisted(&queued, &disk);
            remaining += kept.lines().count();
            if kept != queued {
                self.replace_pending(date, &kept)?;
            }
        }
        Ok(remaining)
    }

    /// Snapshot both versions before modifying the editor buffer. No disk merge
    /// is guessed: Helix's ordinary :write remains the authority on conflicts.
    pub fn import(&self, roots: &[PathBuf], path: &Path, buffer: &str) -> Result<String> {
        // Helix's buffer_name is a display path and may start with ~/ even
        // when opened with an absolute path. Shell-quoted ~ is not expanded.
        let expanded = path.strip_prefix("~").ok().map(|rest| wiki::home().join(rest));
        let path = fs::canonicalize(expanded.as_deref().unwrap_or(path)).context("open an existing DayPage first")?;
        let pages = fs::canonicalize(&self.pages)?;
        if path.parent() != Some(pages.as_path()) || !is_daypage(&path) {
            bail!("Space+U requires a DayPage");
        }
        let date = path.file_stem().and_then(|s| s.to_str()).context("invalid DayPage name")?;
        valid_date(date)?;
        let _lock = self.lock()?;
        let disk = fs::read_to_string(&path)?;
        let queued = self.read_pending(date)?;
        private_dir(&self.recovery)?;
        let recovery = self
            .recovery
            .join(format!("{}-{}", chrono::Utc::now().format("%Y%m%dT%H%M%S%.9f"), std::process::id()));
        // Exclusive directory creation: a collision fails instead of reusing a snapshot.
        fs::DirBuilder::new().mode(0o700).create(&recovery)?;
        write_private(&recovery.join("buffer.md"), buffer.as_bytes())?;
        write_private(&recovery.join("disk.md"), disk.as_bytes())?;
        write_private(&recovery.join("queue.txt"), queued.as_bytes())?;
        write_private(&recovery.join("source.txt"), path.to_string_lossy().as_bytes())?;

        // Apply ALL queued items to the buffer, including ones already on disk:
        // a stale buffer must not silently omit an item we just acknowledged.
        let mut merged = apply_entries(buffer, &queued);
        let roots: Vec<_> = roots.iter().map(|root| wiki::canon(root)).collect();
        let index = Index::build(&roots);
        let i = index.position(&path).context("DayPage absent from wiki index")?;
        if merged.len() >= 10 {
            merged = wiki::with_section(&merged, &index.backlink_names(i));
        }
        if merged.len() <= wiki::LARGE_FILE_BYTES as usize && wiki::outgoing_names(&merged).len() <= wiki::MAX_LINKS {
            merged = resolve::transform(&merged, &index).content;
        }
        write_private(&recovery.join("imported.md"), merged.as_bytes())?;
        File::open(&recovery)?.sync_all()?;
        // Only the old disk contents count as persisted, never the output we
        // are about to send to Helix (which the user may undo or fail to save).
        let kept = unpersisted(&queued, &disk);
        if kept != queued {
            self.replace_pending(date, &kept)?;
        }
        Ok(merged)
    }
}

fn done_line(line: &str, id: &str, checked: bool) -> bool {
    let prefix = if checked { "- [x] " } else { "- [ ] " };
    line.trim_start().strip_prefix(prefix).is_some_and(|rest| {
        rest.strip_prefix(id)
            .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(char::is_whitespace))
    })
}

fn applied(text: &str, entry: &str) -> bool {
    if let Some(id) = entry.strip_prefix("DONE:") {
        !id.is_empty() && text.lines().any(|l| done_line(l, id, true)) && !text.lines().any(|l| done_line(l, id, false))
    } else {
        // Resolve-marker maintenance changes presentation, not entry identity.
        let key = entry.trim().replace("?[[", "[[");
        text.lines().any(|l| l.trim().replace("?[[", "[[") == key)
    }
}

fn unpersisted(queue: &str, disk: &str) -> String {
    queue
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !applied(disk, l))
        .map(|l| format!("{l}\n"))
        .collect()
}

pub fn apply_entries(buffer: &str, queue: &str) -> String {
    let mut content = buffer.to_owned();
    for entry in queue.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(id) = entry.strip_prefix("DONE:") {
            if !id.is_empty() {
                content = content
                    .split_inclusive('\n')
                    .map(|line| {
                        if done_line(line, id, false) {
                            line.replacen("- [ ] ", "- [x] ", 1)
                        } else {
                            line.to_owned()
                        }
                    })
                    .collect();
            }
        } else if !applied(&content, entry) {
            let at = content
                .split_inclusive('\n')
                .scan(0, |pos, line| {
                    let start = *pos;
                    *pos += line.len();
                    Some((start, line))
                })
                .find(|(_, line)| line.trim_end() == "## Backlinks")
                .map(|(i, _)| i);
            match at {
                Some(i) => content.insert_str(i, &format!("{entry}\n\n")),
                None => content = format!("{}\n\n{entry}\n", content.trim_end()),
            }
        }
    }
    content
}
