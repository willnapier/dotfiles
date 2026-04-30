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
use serde::{Deserialize, Serialize};

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
    /// raw bytes when piping into the meliview viewer.
    pub filename: Option<String>,
    /// Plain-text body parts (joined). None if the message has no
    /// text/plain alternative.
    pub text_plain: Option<String>,
    /// Raw HTML body, if any. Renderable via meliview's existing pipeline.
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
pub fn mailbox_query(_account: &Account, _mailbox: &str) -> Option<String> {
    todo!("implement query mapping per the doc comment above")
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
pub fn search(_query: &str, _offset: usize, _limit: usize) -> Result<Vec<Envelope>> {
    todo!("shell out to `notmuch search` and parse JSON")
}

/// Total count of messages matching the query. Used by paginator.
///
/// `notmuch count <query>` returns a single integer on stdout. mailcurator
/// has the canonical implementation; copy from there.
pub fn count(_query: &str) -> Result<u64> {
    todo!("shell out to `notmuch count`")
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
pub fn show(_id: &str) -> Result<Message> {
    todo!("notmuch search --output=files + mail-parser parse")
}

/// Fetch all messages in a thread, in chronological order.
///
/// `thread_id` is the bare thread id (no `thread:` prefix). The query is
/// `thread:<id>`.
pub fn show_thread(_thread_id: &str) -> Result<Vec<Message>> {
    todo!("notmuch show + mail-parser per-message")
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
pub fn apply_tag_changes(
    _query: &str,
    _add: &[&str],
    _remove: &[&str],
) -> Result<()> {
    todo!("shell out to `notmuch tag`; see mailcurator/src/notmuch.rs")
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

#[allow(dead_code)]
fn _force_anyhow_in_scope() -> Result<()> {
    // Kept so `anyhow::Context` doesn't get flagged unused before the
    // todo!() bodies are filled. Remove when the first impl lands.
    let r: std::io::Result<()> = Ok(());
    r.context("placeholder")
}
