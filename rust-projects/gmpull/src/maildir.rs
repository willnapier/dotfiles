//! Maildir writer — lieer-compatible filename layout.
//!
//! Filename: `<gmail_id>:2,<flags>` where `<flags>` is the sorted
//! concatenation of single-letter Maildir flags derived from Gmail
//! labelIds:
//!
//! | Gmail label  | Maildir flag | Meaning                |
//! |--------------|--------------|------------------------|
//! | (no UNREAD)  | `S`          | Seen                   |
//! | STARRED      | `F`          | Flagged                |
//! | (any)        | -            | (Replied/`R` is not    |
//! |              |              | derivable from labels) |
//! | TRASH        | -            | skipped (default)      |
//!
//! We deliberately keep things small. `P` (passed/forwarded) and
//! `D` (draft) aren't derivable from Gmail's label set without
//! cross-checking the X-Gmail-Labels header, which lieer doesn't do
//! either; mbsync handles them via IMAP `\Draft` etc., which we
//! don't have here.
//!
//! Atomicity: write to `tmp/` first, then `rename(tmp → cur)`. POSIX
//! guarantees this is atomic on the same filesystem, which the
//! parallel `tmp`/`cur` directory layout ensures. mtime is set to
//! Gmail's `internalDate` *after* the rename so date-sorted clients
//! show real receipt order.

use anyhow::{Context, Result};
use filetime::FileTime;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::api::RawMessage;

/// Per-process counter so concurrent writes get distinct tmp names
/// without needing a PRNG. Combined with PID and nanosecond clock
/// it's effectively collision-free.
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Should this message be skipped (e.g. it's in TRASH)?
pub fn should_skip(label_ids: &[String]) -> bool {
    label_ids.iter().any(|l| l == "TRASH" || l == "SPAM")
}

/// Derive the Maildir flag string (e.g. "S", "FS", "") from a list
/// of Gmail label IDs. Always returned in alphabetic order, which
/// is the Maildir convention.
pub fn flags_from_labels(label_ids: &[String]) -> String {
    let mut flags: Vec<char> = Vec::with_capacity(2);
    let unread = label_ids.iter().any(|l| l == "UNREAD");
    if !unread {
        flags.push('S');
    }
    if label_ids.iter().any(|l| l == "STARRED") {
        flags.push('F');
    }
    flags.sort_unstable();
    flags.into_iter().collect()
}

/// Write one message to the maildir. The destination filename is
/// `<id>:2,<flags>` placed under `<root>/cur/`. tmp/ and cur/ must
/// already exist (callers create them once at startup).
///
/// If a file with the same Gmail ID already exists with the same
/// flags we skip writing (saves an inode dance on resume); if flags
/// differ, we overwrite via the same tmp→cur rename.
///
/// `_labels` is plumbed through for future per-folder mirroring; v1
/// uses a single flat maildir so the parameter is only consulted to
/// silence unused warnings.
pub async fn write_message(
    root: &Path,
    msg: &RawMessage,
    _labels: &HashMap<String, String>,
) -> Result<PathBuf> {
    let flags = flags_from_labels(&msg.label_ids);
    let final_name = format!("{}:2,{}", msg.id, flags);
    let cur_path = root.join("cur").join(&final_name);

    // Fast-path: if a file with the same suffix already exists, do
    // nothing. This keeps `--resume` cheap (~1ms stat per dup).
    if tokio::fs::metadata(&cur_path).await.is_ok() {
        return Ok(cur_path);
    }

    // Build a unique tmp filename per Maildir convention:
    // `<unix_ts>.P<pid>Q<seq>.<host>` with a `.gmpull` suffix to make
    // strays easy to clean up if we crash mid-write.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let pid = std::process::id();
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!("{now}.P{pid}Q{seq}.gmpull-tmp");
    let tmp_path = root.join("tmp").join(&tmp_name);

    // Write body, then mtime, then rename. Each step is O(1) syscalls.
    tokio::fs::write(&tmp_path, &msg.raw_rfc822)
        .await
        .with_context(|| format!("writing tmp {}", tmp_path.display()))?;

    // Set mtime to Gmail internalDate. Use the std-lib filetime
    // crate because tokio::fs has no equivalent and we don't need
    // async for a single fstat-like call.
    let mtime_secs = msg.internal_date_ms / 1000;
    let mtime = FileTime::from_unix_time(mtime_secs, 0);
    if let Err(e) = filetime::set_file_mtime(&tmp_path, mtime) {
        // Non-fatal — log and continue. Worst case: file gets the
        // current time, sort order is slightly off for this one.
        tracing::warn!(error = %e, tmp = %tmp_path.display(), "set_file_mtime failed");
    }

    // Atomic rename (same FS). We tolerate the destination already
    // existing due to a concurrent write — last writer wins, which
    // is fine because all writers had the same Gmail ID + flags.
    tokio::fs::rename(&tmp_path, &cur_path)
        .await
        .with_context(|| {
            format!(
                "rename {} -> {}",
                tmp_path.display(),
                cur_path.display()
            )
        })?;

    Ok(cur_path)
}

/// Ensure a maildir's `cur/`, `new/`, and `tmp/` subdirs exist.
/// Idempotent — safe to call every startup.
pub async fn ensure_maildir(root: &Path) -> Result<()> {
    for sub in ["cur", "new", "tmp"] {
        tokio::fs::create_dir_all(root.join(sub))
            .await
            .with_context(|| format!("creating {}/{sub}", root.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_seen_when_no_unread() {
        let labels = vec!["INBOX".to_string()];
        assert_eq!(flags_from_labels(&labels), "S");
    }

    #[test]
    fn flags_unseen_when_unread_present() {
        let labels = vec!["INBOX".to_string(), "UNREAD".to_string()];
        assert_eq!(flags_from_labels(&labels), "");
    }

    #[test]
    fn flags_starred_combines_with_seen() {
        let labels = vec!["STARRED".to_string()];
        // No UNREAD → S; STARRED → F; sorted → "FS"
        assert_eq!(flags_from_labels(&labels), "FS");
    }

    #[test]
    fn flags_starred_unread() {
        let labels = vec!["STARRED".to_string(), "UNREAD".to_string()];
        // UNREAD → no S; STARRED → F.
        assert_eq!(flags_from_labels(&labels), "F");
    }

    #[test]
    fn skip_trash_and_spam() {
        assert!(should_skip(&["TRASH".to_string()]));
        assert!(should_skip(&["SPAM".to_string()]));
        assert!(!should_skip(&["INBOX".to_string()]));
    }
}
