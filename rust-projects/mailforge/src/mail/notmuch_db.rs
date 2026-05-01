//! Thin subprocess wrapper over the `notmuch` CLI.
//!
//! Mirrors the `mailcurator/src/notmuch.rs` pattern: shell out, no
//! libnotmuch dev-headers dependency at build time. Call pattern is
//! batch-oriented (one HTTP request → one notmuch query → one render),
//! so subprocess overhead is negligible vs query-execution time.
//!
//! ## Why CLI not bindings
//!
//! - **No build-time C deps**: cargo build works on a fresh machine without
//!   `apt install libnotmuch-dev` or homebrew library_file_path coupling.
//!   Meli has to do gymnastics with `library_file_path` in its config to
//!   find libnotmuch; we sidestep that.
//! - **Notmuch CLI is stable**: the `--format=json` and `--output=*`
//!   contracts are versioned. mailcurator has been stable on 0.39+ for
//!   months.
//! - **Subprocess overhead is negligible**: a single `notmuch search`
//!   takes 50-200ms on the 217k-message DB; fork+exec is < 5ms. The user
//!   waits for Xapian, not for `fork(2)`.
//!
//! ## What this module exposes
//!
//! Listed roughly in the order the implementation agent should fill them:
//!
//! 1. [`mailbox_query`] — translate (account, mailbox) to a notmuch query
//!    string. The mapping mirrors `~/.config/meli/config.toml`.
//! 2. [`search`] — run a query, parse JSON, return `Vec<Envelope>`.
//! 3. [`count`] — `notmuch count <query>` — used for paginator total.
//! 4. [`show`] — fetch a single message (headers + body parts) by id.
//! 5. [`tag`] — apply tag changes to a query.
//!
//! All functions are synchronous (`std::process::Command`). They're called
//! from async axum handlers via `tokio::task::spawn_blocking` if blocking
//! becomes an issue; for now the call durations (50-200ms) are short
//! enough that running them on the tokio runtime threads is acceptable.

use anyhow::{Context, Result};
use mail_parser::MessageParser;
use serde::{Deserialize, Serialize};
use std::process::{Command, Stdio};

use crate::mail::accounts::Account;

/// One row in a mailbox listing. Fields mirror notmuch's
/// `--output=summary --format=json` schema.
///
/// See `notmuch search --format=json --output=summary "..."` for the
/// upstream contract. The shape of one row:
///
/// ```json
/// {
///   "thread": "0000000000031458",
///   "timestamp": 1777537591,
///   "date_relative": "16 mins. ago",
///   "matched": 1,
///   "total": 1,
///   "authors": "Avigilon Alta",
///   "subject": "...",
///   "query": ["id:...", null],
///   "tags": ["inbox", "unread"]
/// }
/// ```
///
/// `query[0]` carries an `id:<message-id>` term that (for single-message
/// threads) uniquely identifies the message. For multi-message threads we
/// follow the thread-id link and let `show_thread` enumerate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub thread: String,
    pub timestamp: i64,
    pub date_relative: String,
    pub matched: u32,
    pub total: u32,
    pub authors: String,
    pub subject: String,
    /// `["id:<msg-id>", null]` for single-thread queries; the id: term
    /// is the most reliable per-message handle.
    pub query: [Option<String>; 2],
    pub tags: Vec<String>,
    /// Set by the listing render path AFTER the notmuch JSON
    /// deserialization, by parsing the message file's `List-Unsubscribe`
    /// header. `#[serde(default)]` so notmuch's JSON (which doesn't carry
    /// this field) deserializes cleanly with `has_unsubscribe = false`.
    /// Drives the per-row hover-reveal unsubscribe icon.
    #[serde(default)]
    pub has_unsubscribe: bool,
}

impl Envelope {
    /// Extract the bare message id (without "id:" prefix) from `query[0]`,
    /// when the thread has a single matched message. Returns None for
    /// multi-message threads.
    ///
    /// Format: `query[0]` is `"id:<message-id>"`; we strip the prefix.
    /// notmuch IDs include `@`, which means downstream URL handlers need
    /// to URL-encode them (or accept them as path components, which axum
    /// handles via `Path<String>`).
    pub fn message_id(&self) -> Option<&str> {
        if self.matched != 1 {
            return None;
        }
        self.query[0].as_deref()?.strip_prefix("id:")
    }
}

/// A fully-fetched message: headers + decoded body parts. Returned by
/// [`show`]. The handler is responsible for choosing how to render
/// (text/plain inline; text/html via the existing `/v/<uuid>` viewer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub subject: Option<String>,
    pub from: Option<String>,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub date: Option<String>,
    pub tags: Vec<String>,
    /// File path on disk (under `~/Mail/.../cur/`). Useful for reading
    /// raw bytes when piping into the mailforge viewer.
    pub filename: Option<String>,
    /// Plain-text body parts (joined). None if the message has no
    /// text/plain alternative.
    pub text_plain: Option<String>,
    /// Raw HTML body, if any. Renderable via mailforge's existing pipeline.
    pub text_html: Option<String>,
}

// ------------------------------------------------------------------
// Query translation
// ------------------------------------------------------------------

/// Translate (account, mailbox) into a notmuch query string.
///
/// Mirrors the queries in `~/.config/meli/config.toml`. The mapping is
/// account-aware: `personal/inbox` excludes `tag:cohs` (workspace inbox
/// excludes COHS messages); `cohs/inbox` requires `tag:cohs`.
///
/// Returns None for unknown mailbox names.
///
/// ## Mailbox vocabulary
///
/// Personal account:
/// - `inbox`     → `tag:inbox and not tag:archive and not tag:spam and not tag:trash and not tag:cohs and not tag:promotions and date:6M..`
/// - `promotions`→ `tag:inbox and tag:promotions and not tag:archive and not tag:spam and not tag:trash and not tag:cohs and date:6M..`
/// - `unread`    → `tag:unread and not tag:cohs`
/// - `sent`      → `tag:sent and not tag:cohs`
/// - `archive`   → `not tag:inbox and not tag:trash and not tag:spam and not tag:sent and not tag:cohs`
/// - `all-mail`  → `date:30d.. and not tag:cohs`
///
/// COHS account:
/// - `inbox`     → `tag:cohs and tag:inbox and not tag:archive and not tag:spam and not tag:trash`
/// - `unread`    → `tag:cohs and tag:unread and not tag:trash and not tag:spam`
/// - `sent`      → `tag:cohs and tag:sent`
/// - `drafts`    → `tag:cohs and tag:drafts`
/// - `archive`   → `tag:cohs and tag:archive`
/// - `trash`     → `tag:cohs and tag:trash`
/// - `spam`      → `tag:cohs and tag:spam`
///
/// Implementation hint: a static `match (account.slug, mailbox)` block is
/// fine. The total count of valid (slug, mailbox) pairs is ~15.
pub fn mailbox_query(account: &Account, mailbox: &str) -> Option<String> {
    // Inbox queries hide `tag:sent` clutter (Gmail labels self-addressed
    // mail as both Inbox and Sent) EXCEPT when `from:` is the account's
    // own identity — that keeps useful self-sent items visible (PracticeForge
    // OTP login codes, GitHub notifications addressed back to your own
    // account, etc.) while still excluding any other sent-tagged stragglers.
    let self_from = account.identity;
    let q: String = match (account.slug, mailbox) {
        // ---------------- Personal (Gmail / no cohs tag) ----------------
        ("personal", "inbox") => format!(
            "tag:inbox and (not tag:sent or from:{self_from}) \
             and not tag:archive and not tag:spam and not tag:trash \
             and not tag:cohs and not tag:promotions and date:6M.."
        ),
        ("personal", "promotions") => {
            "tag:inbox and tag:promotions and not tag:archive and not tag:spam \
             and not tag:trash and not tag:cohs and date:6M..".to_string()
        }
        ("personal", "unread") => "tag:unread and not tag:cohs".to_string(),
        ("personal", "sent") => "tag:sent and not tag:cohs".to_string(),
        ("personal", "archive") => {
            "not tag:inbox and not tag:trash and not tag:spam and not tag:sent and not tag:cohs".to_string()
        }
        ("personal", "all-mail") => "date:30d.. and not tag:cohs".to_string(),

        // ---------------- COHS (M365, gated by tag:cohs) ----------------
        ("cohs", "inbox") => format!(
            "tag:cohs and tag:inbox and (not tag:sent or from:{self_from}) \
             and not tag:archive and not tag:spam and not tag:trash"
        ),
        ("cohs", "unread") => "tag:cohs and tag:unread and not tag:trash and not tag:spam".to_string(),
        ("cohs", "sent") => "tag:cohs and tag:sent".to_string(),
        ("cohs", "drafts") => "tag:cohs and tag:drafts".to_string(),
        ("cohs", "archive") => "tag:cohs and tag:archive".to_string(),
        ("cohs", "trash") => "tag:cohs and tag:trash".to_string(),
        ("cohs", "spam") => "tag:cohs and tag:spam".to_string(),

        _ => return None,
    };
    Some(q)
}

// ------------------------------------------------------------------
// Search / list
// ------------------------------------------------------------------

/// Run a notmuch search and parse the JSON output into envelopes.
///
/// `query` is the raw notmuch query (already account-prefixed if needed —
/// callers use [`mailbox_query`] for mailbox listings, raw user input
/// for cross-mailbox search).
///
/// `offset` and `limit` map to notmuch's `--offset` and `--limit` flags.
/// Use `0`-indexed offset; pass `limit = 50` (or whatever the page size is).
///
/// Sort order: notmuch defaults to newest-first (`--sort=newest-first`).
/// Pass `--sort=oldest-first` if archived browsing wants chronological
/// order; not supported by this signature yet (add when a handler needs it).
///
/// Implementation hint:
/// ```ignore
/// let output = Command::new("notmuch")
///     .args(["search", "--format=json", "--output=summary",
///            &format!("--offset={offset}"), &format!("--limit={limit}"),
///            query])
///     .output()?;
/// let envs: Vec<Envelope> = serde_json::from_slice(&output.stdout)?;
/// ```
pub fn search(query: &str, offset: usize, limit: usize) -> Result<Vec<Envelope>> {
    let output = Command::new("notmuch")
        .args([
            "search",
            "--format=json",
            "--output=summary",
            &format!("--offset={offset}"),
            &format!("--limit={limit}"),
            query,
        ])
        .stderr(Stdio::null())
        .output()
        .with_context(|| format!("spawning `notmuch search ... {query}`"))?;
    if !output.status.success() {
        anyhow::bail!(
            "notmuch search failed (exit {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    parse_search_json(&output.stdout)
        .with_context(|| format!("parsing `notmuch search` JSON for query: {query}"))
}

/// Parse notmuch search JSON output into a `Vec<Envelope>`.
///
/// Split out so the parsing layer can be unit-tested without touching
/// the subprocess.
fn parse_search_json(bytes: &[u8]) -> Result<Vec<Envelope>> {
    let envs: Vec<Envelope> = serde_json::from_slice(bytes)
        .context("deserializing notmuch search JSON output")?;
    Ok(envs)
}

/// Total count of messages matching the query. Used by paginator.
///
/// `notmuch count <query>` returns a single integer on stdout. mailcurator
/// has the canonical implementation; copy from there.
pub fn count(query: &str) -> Result<u64> {
    let output = Command::new("notmuch")
        .args(["count", query])
        .stderr(Stdio::null())
        .output()
        .with_context(|| format!("spawning `notmuch count {query}`"))?;
    if !output.status.success() {
        anyhow::bail!(
            "notmuch count failed (exit {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let s = String::from_utf8_lossy(&output.stdout);
    let n = s
        .trim()
        .parse::<u64>()
        .with_context(|| format!("parsing notmuch count output: {s:?}"))?;
    Ok(n)
}

// ------------------------------------------------------------------
// Show one message / one thread
// ------------------------------------------------------------------

/// Fetch one message by ID. Returns the full envelope plus decoded
/// body parts.
///
/// `id` is the bare message id (no `id:` prefix). The query becomes
/// `id:<id>`. Notmuch guarantees uniqueness on Message-ID at index time.
///
/// Implementation:
/// 1. `notmuch search --output=files id:<id>` to get the on-disk path.
/// 2. Read the file via `std::fs::read`.
/// 3. Parse with `mail-parser::MessageParser` (already a dep).
/// 4. Pull headers and body parts into [`Message`].
///
/// Fallback for missing files (e.g. mbsync just deleted the file but
/// notmuch hasn't run): return Err with a clear message; handler can
/// 404 or render a stub.
pub fn show(id: &str) -> Result<Message> {
    let query = format!("id:{id}");

    // 1. Find the on-disk path(s). A single Message-ID can have multiple
    //    backing files (Gmail's All Mail + label folders); we pick the
    //    first that exists.
    let path = first_existing_file(&query)
        .with_context(|| format!("locating files for {query}"))?
        .ok_or_else(|| anyhow::anyhow!("no on-disk file found for {query}"))?;

    // 2. Read raw bytes.
    let bytes = std::fs::read(&path)
        .with_context(|| format!("reading message file {}", path.display()))?;

    // 3. Parse with mail-parser.
    let parsed = MessageParser::default()
        .parse(&bytes[..])
        .ok_or_else(|| anyhow::anyhow!("mail-parser failed to parse {}", path.display()))?;

    // 4. Pull tags via a separate `notmuch search --output=tags` call.
    //    Cheap (single Xapian lookup); avoids re-parsing the message-level
    //    JSON which would also require parsing all the body parts twice.
    let tags = fetch_tags(&query).unwrap_or_default();

    // 5. Build Message.
    let subject = parsed.subject().map(|s| s.to_string());
    let from = parsed
        .from()
        .and_then(|a| a.first())
        .and_then(|a| a.address().map(|s| s.to_string()));
    let to = parsed
        .to()
        .map(|addr| {
            addr.iter()
                .filter_map(|a| a.address().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let cc = parsed
        .cc()
        .map(|addr| {
            addr.iter()
                .filter_map(|a| a.address().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let date = parsed.date().map(|d| d.to_rfc3339());
    let text_plain = if parsed.text_body_count() > 0 {
        parsed.body_text(0).map(|c| c.into_owned())
    } else {
        None
    };
    let text_html = if parsed.html_body_count() > 0 {
        parsed.body_html(0).map(|c| c.into_owned())
    } else {
        None
    };

    Ok(Message {
        id: id.to_string(),
        subject,
        from,
        to,
        cc,
        date,
        tags,
        filename: Some(path.to_string_lossy().into_owned()),
        text_plain,
        text_html,
    })
}

/// Fetch all messages in a thread, in chronological order.
///
/// `thread_id` is the bare thread id (no `thread:` prefix). The query is
/// `thread:<id>`.
pub fn show_thread(thread_id: &str) -> Result<Vec<Message>> {
    let query = format!("thread:{thread_id}");

    // Get the per-message id list, oldest-first for chronological order.
    let output = Command::new("notmuch")
        .args([
            "search",
            "--format=json",
            "--output=messages",
            "--sort=oldest-first",
            &query,
        ])
        .stderr(Stdio::null())
        .output()
        .with_context(|| format!("spawning `notmuch search --output=messages {query}`"))?;
    if !output.status.success() {
        anyhow::bail!(
            "notmuch search (thread) failed (exit {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let ids: Vec<String> = serde_json::from_slice(&output.stdout)
        .context("parsing notmuch --output=messages JSON")?;

    let mut messages = Vec::with_capacity(ids.len());
    for raw in ids {
        // notmuch returns "id:<x>" entries; strip the prefix.
        let bare = raw.strip_prefix("id:").unwrap_or(&raw);
        match show(bare) {
            Ok(m) => messages.push(m),
            Err(e) => {
                // Log and continue; one missing file shouldn't drop the
                // whole thread render.
                tracing::warn!(
                    "show_thread: skipping {bare} in thread:{thread_id}: {e:#}"
                );
            }
        }
    }
    Ok(messages)
}

/// Fetch the on-disk file path for the first matching message file.
/// Returns None if notmuch finds no files. Returns Err only on subprocess
/// failure (not on no-results).
fn first_existing_file(query: &str) -> Result<Option<std::path::PathBuf>> {
    let output = Command::new("notmuch")
        .args(["search", "--output=files", query])
        .stderr(Stdio::null())
        .output()
        .with_context(|| format!("spawning `notmuch search --output=files {query}`"))?;
    if !output.status.success() {
        anyhow::bail!(
            "notmuch search --output=files failed (exit {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let p = std::path::PathBuf::from(line);
        if p.exists() {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

/// Fetch the tag list for a single-message query.
fn fetch_tags(query: &str) -> Result<Vec<String>> {
    let output = Command::new("notmuch")
        .args(["search", "--format=json", "--output=tags", query])
        .stderr(Stdio::null())
        .output()
        .with_context(|| format!("spawning `notmuch search --output=tags {query}`"))?;
    if !output.status.success() {
        anyhow::bail!(
            "notmuch search --output=tags failed (exit {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let tags: Vec<String> = serde_json::from_slice(&output.stdout)
        .context("parsing notmuch --output=tags JSON")?;
    Ok(tags)
}

/// Read the raw RFC822 bytes for a single message id. Used by handlers
/// that hand off to the existing `pipe::run` HTML-render pipeline.
///
/// Returns the full message including all MIME parts, exactly as on disk.
pub fn raw_bytes(id: &str) -> Result<Vec<u8>> {
    let query = format!("id:{id}");
    let path = first_existing_file(&query)
        .with_context(|| format!("locating files for {query}"))?
        .ok_or_else(|| anyhow::anyhow!("no on-disk file found for {query}"))?;
    std::fs::read(&path).with_context(|| format!("reading {}", path.display()))
}

// ------------------------------------------------------------------
// Tag operations
// ------------------------------------------------------------------

/// Apply tag changes to messages matching the query.
///
/// `add` and `remove` are bare tag names (no `+` / `-` prefix; this
/// function adds them). Empty slices are valid no-ops.
///
/// Convention: when the caller wants to operate on specific message ids,
/// they construct the query as `id:<a> or id:<b> or ...`. notmuch supports
/// this natively. For trash workflows the caller uses
/// `query = "id:<the-id>"`; for bulk operations from the listing the
/// caller folds `or` between the selected ids.
///
/// Implementation: copy from `mailcurator::notmuch::apply_tag_changes`.
pub fn apply_tag_changes(query: &str, add: &[&str], remove: &[&str]) -> Result<()> {
    if add.is_empty() && remove.is_empty() {
        return Ok(());
    }
    let mut changes: Vec<String> = Vec::with_capacity(add.len() + remove.len());
    for t in add {
        changes.push(format!("+{t}"));
    }
    for t in remove {
        changes.push(format!("-{t}"));
    }
    let mut cmd = Command::new("notmuch");
    cmd.arg("tag");
    for ch in &changes {
        cmd.arg(ch);
    }
    cmd.arg("--");
    cmd.arg(query);
    cmd.stderr(Stdio::null());
    let status = cmd.status().with_context(|| {
        format!("spawning `notmuch tag {changes:?} -- {query}`")
    })?;
    if !status.success() {
        anyhow::bail!("notmuch tag failed (exit {:?})", status.code());
    }
    Ok(())
}

/// Convenience: add a single tag to messages matching `query`.
pub fn add_tag(query: &str, tag: &str) -> Result<()> {
    apply_tag_changes(query, &[tag], &[])
}

/// Convenience: remove a single tag from messages matching `query`.
pub fn remove_tag(query: &str, tag: &str) -> Result<()> {
    apply_tag_changes(query, &[], &[tag])
}

/// Replace tags atomically: for each (add, remove) pair this is a single
/// `notmuch tag` invocation. Useful for "move to trash" = add trash + remove inbox.
pub fn set_tag(query: &str, add: &[&str], remove: &[&str]) -> Result<()> {
    apply_tag_changes(query, add, remove)
}

// ------------------------------------------------------------------
// Helpers (already implemented because they're trivial and shared)
// ------------------------------------------------------------------

/// Build a notmuch query that matches any of the given message ids.
/// Returns `id:a or id:b or id:c`. Empty input returns the empty string,
/// which is a notmuch-illegal query — callers should guard.
pub fn ids_to_query(ids: &[String]) -> String {
    ids.iter()
        .map(|id| format!("id:{id}"))
        .collect::<Vec<_>>()
        .join(" or ")
}

/// URL-encode a bare message id for use as a path component.
/// Notmuch ids contain `@` and sometimes `<`/`>`; axum's path extractor
/// accepts `@` but `<`/`>` need encoding.
pub fn encode_id(id: &str) -> String {
    // url crate is already a dep.
    url::form_urlencoded::byte_serialize(id.as_bytes()).collect()
}

// ------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::accounts::{find, ACCOUNTS};

    /// Sanity: every static account has at least one valid mailbox.
    #[test]
    fn every_account_has_inbox_query() {
        for acc in ACCOUNTS {
            let q = mailbox_query(acc, "inbox");
            assert!(
                q.is_some(),
                "account {} has no inbox query mapping",
                acc.slug
            );
            let q = q.unwrap();
            assert!(
                q.contains("tag:inbox") || q.contains("tag:cohs"),
                "inbox query for {} doesn't reference inbox or cohs tag: {q}",
                acc.slug
            );
        }
    }

    #[test]
    fn personal_inbox_excludes_cohs_and_promotions() {
        let acc = find("personal").expect("personal account exists");
        let q = mailbox_query(acc, "inbox").expect("inbox mapping exists");
        assert!(q.contains("not tag:cohs"), "got: {q}");
        assert!(q.contains("not tag:promotions"), "got: {q}");
        assert!(q.contains("date:6M.."), "got: {q}");
    }

    #[test]
    fn cohs_inbox_requires_cohs_tag() {
        let acc = find("cohs").expect("cohs account exists");
        let q = mailbox_query(acc, "inbox").expect("inbox mapping exists");
        assert!(q.starts_with("tag:cohs"), "got: {q}");
        assert!(q.contains("tag:inbox"), "got: {q}");
    }

    #[test]
    fn unknown_mailbox_returns_none() {
        let acc = find("personal").expect("personal account exists");
        assert!(mailbox_query(acc, "no-such-mailbox").is_none());
        assert!(mailbox_query(acc, "drafts").is_none()); // personal has no drafts
    }

    #[test]
    fn personal_inbox_hides_sent_unless_from_self() {
        let acc = find("personal").expect("personal account exists");
        let q = mailbox_query(acc, "inbox").expect("inbox mapping exists");
        // The clause must contain both halves: hide sent items in general,
        // but exempt those whose from-address is this account's identity.
        assert!(q.contains("not tag:sent"), "got: {q}");
        assert!(
            q.contains(&format!("from:{}", acc.identity)),
            "expected from:{} clause, got: {q}",
            acc.identity
        );
        // Order matters for notmuch — verify the OR groups correctly
        assert!(
            q.contains(&format!("(not tag:sent or from:{})", acc.identity)),
            "from-self exception must be parenthesised correctly, got: {q}"
        );
    }

    #[test]
    fn cohs_inbox_hides_sent_unless_from_self() {
        let acc = find("cohs").expect("cohs account exists");
        let q = mailbox_query(acc, "inbox").expect("inbox mapping exists");
        assert!(q.contains("not tag:sent"), "got: {q}");
        assert!(
            q.contains(&format!("from:{}", acc.identity)),
            "expected from:{} clause, got: {q}",
            acc.identity
        );
    }

    #[test]
    fn cohs_specific_mailboxes() {
        let acc = find("cohs").expect("cohs account exists");
        for mbox in &["inbox", "unread", "sent", "drafts", "archive", "trash", "spam"] {
            assert!(
                mailbox_query(acc, mbox).is_some(),
                "cohs/{mbox} mapping missing"
            );
        }
        // Personal-only mailboxes don't apply to cohs:
        assert!(mailbox_query(acc, "promotions").is_none());
        assert!(mailbox_query(acc, "all-mail").is_none());
    }

    // ---------------- search JSON parser ----------------

    #[test]
    fn parse_search_json_typical_row() {
        // Real-shape sample (from `notmuch search --format=json` output).
        let raw = br#"[
            {
              "thread": "0000000000031458",
              "timestamp": 1777537591,
              "date_relative": "16 mins. ago",
              "matched": 1,
              "total": 1,
              "authors": "Avigilon Alta",
              "subject": "Camera offline alert",
              "query": ["id:foo@example.com", null],
              "tags": ["inbox", "unread"]
            }
        ]"#;
        let envs = parse_search_json(raw).expect("parse should succeed");
        assert_eq!(envs.len(), 1);
        let e = &envs[0];
        assert_eq!(e.thread, "0000000000031458");
        assert_eq!(e.timestamp, 1777537591);
        assert_eq!(e.matched, 1);
        assert_eq!(e.total, 1);
        assert_eq!(e.authors, "Avigilon Alta");
        assert_eq!(e.subject, "Camera offline alert");
        assert_eq!(e.tags, vec!["inbox".to_string(), "unread".to_string()]);
        assert_eq!(e.message_id(), Some("foo@example.com"));
    }

    #[test]
    fn parse_search_json_empty_array() {
        let envs = parse_search_json(b"[]").expect("empty array parses");
        assert!(envs.is_empty());
    }

    #[test]
    fn parse_search_json_multi_message_thread() {
        // Multi-matched threads have query[0] = id:... but matched > 1
        // so message_id() should return None.
        let raw = br#"[
            {
              "thread": "deadbeef",
              "timestamp": 1700000000,
              "date_relative": "2 days ago",
              "matched": 3,
              "total": 5,
              "authors": "Alice, Bob, Carol",
              "subject": "Re: project",
              "query": ["id:msg1@x.com", null],
              "tags": ["inbox"]
            }
        ]"#;
        let envs = parse_search_json(raw).expect("parse");
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].matched, 3);
        // message_id() refuses multi-message threads (the URL would be
        // ambiguous — handler should use thread-view instead).
        assert_eq!(envs[0].message_id(), None);
    }

    #[test]
    fn parse_search_json_invalid_input_errors() {
        assert!(parse_search_json(b"not json").is_err());
        assert!(parse_search_json(b"{}").is_err()); // expected array
    }

    // ---------------- helpers ----------------

    #[test]
    fn ids_to_query_empty() {
        let q = ids_to_query(&[]);
        assert_eq!(q, "");
    }

    #[test]
    fn ids_to_query_single() {
        let q = ids_to_query(&["a@b.com".to_string()]);
        assert_eq!(q, "id:a@b.com");
    }

    #[test]
    fn ids_to_query_multiple() {
        let q = ids_to_query(&[
            "a@b.com".to_string(),
            "c@d.com".to_string(),
            "e@f.com".to_string(),
        ]);
        assert_eq!(q, "id:a@b.com or id:c@d.com or id:e@f.com");
    }

    #[test]
    fn encode_id_handles_at_sign() {
        // `@` survives form-urlencoded byte_serialize as %40 — that's fine
        // for path components.
        let encoded = encode_id("foo@bar.com");
        assert!(
            encoded.contains("%40") || encoded.contains('@'),
            "encoded form must represent @: {encoded}"
        );
    }

    #[test]
    fn encode_id_escapes_angle_brackets() {
        let encoded = encode_id("<foo@bar.com>");
        assert!(!encoded.starts_with('<'), "got: {encoded}");
        assert!(!encoded.ends_with('>'), "got: {encoded}");
    }

    #[test]
    fn envelope_message_id_strips_prefix() {
        let env = Envelope {
            thread: "tid".into(),
            timestamp: 0,
            date_relative: String::new(),
            matched: 1,
            total: 1,
            authors: String::new(),
            subject: String::new(),
            query: [Some("id:abc@example.com".to_string()), None],
            tags: vec![],
            has_unsubscribe: false,
        };
        assert_eq!(env.message_id(), Some("abc@example.com"));
    }

    #[test]
    fn envelope_message_id_returns_none_when_query_missing() {
        let env = Envelope {
            thread: "tid".into(),
            timestamp: 0,
            date_relative: String::new(),
            matched: 1,
            total: 1,
            authors: String::new(),
            subject: String::new(),
            query: [None, None],
            tags: vec![],
            has_unsubscribe: false,
        };
        assert_eq!(env.message_id(), None);
    }

    #[test]
    fn envelope_message_id_returns_none_when_no_id_prefix() {
        let env = Envelope {
            thread: "tid".into(),
            timestamp: 0,
            date_relative: String::new(),
            matched: 1,
            total: 1,
            authors: String::new(),
            subject: String::new(),
            // notmuch should always produce id:..., but be defensive.
            query: [Some("not-an-id-term".to_string()), None],
            tags: vec![],
            has_unsubscribe: false,
        };
        assert_eq!(env.message_id(), None);
    }

    #[test]
    fn envelope_deserializes_without_has_unsubscribe_field() {
        // Notmuch's JSON output doesn't carry has_unsubscribe; serde
        // default must fill it in as false so deserialization stays
        // backward-compatible.
        let raw = br#"[
            {
              "thread": "abc",
              "timestamp": 1,
              "date_relative": "now",
              "matched": 1,
              "total": 1,
              "authors": "x",
              "subject": "y",
              "query": ["id:m@x.com", null],
              "tags": []
            }
        ]"#;
        let envs = parse_search_json(raw).expect("parse");
        assert_eq!(envs.len(), 1);
        assert!(!envs[0].has_unsubscribe);
    }
}
