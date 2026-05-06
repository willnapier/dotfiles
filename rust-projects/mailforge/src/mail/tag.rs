//! Tag operations: generic tag editor + sugar for trash/archive.
//!
//! All three endpoints are JSON-in / JSON-out. They're called from the
//! client-side keyboard handler (`static/js/keys.js`) via `fetch()`.
//!
//! POST `/api/tag`     — generic add/remove for arbitrary tags.
//! POST `/api/trash`   — sugar: add `trash`, remove `inbox`.
//! POST `/api/archive` — sugar: remove `inbox` (no add tag — archive is
//!                       defined by the absence of inbox/trash/spam/sent).
//!
//! ## Server-side propagation
//!
//! mailforge touches the local notmuch DB only. Server-side mirroring
//! happens via existing infrastructure:
//! - Personal (Gmail): `gmail-push-tags` (15-min launchd timer) translates
//!   notmuch tags to Gmail label changes.
//! - COHS (M365): `cohs-trash-mover` (5-min timer) moves trash-tagged
//!   messages to the M365 Deleted Items folder; mbsync replicates back.
//!
//! End-to-end latency: 0-20 minutes per existing meli config docs. No
//! change needed for mailforge.
//!
//! ## Concurrency
//!
//! `notmuch tag` is atomic per invocation. Multiple parallel requests are
//! safe. notmuch's lock-retry mechanism (the `built_with.retry_lock=true`
//! flag from `notmuch config list`) handles concurrent writers.

use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};

use crate::mail::notmuch_db;

/// Body of POST `/api/tag`.
#[derive(Debug, Deserialize)]
pub struct TagRequest {
    /// Bare message ids (no `id:` prefix). Folded into a notmuch query
    /// of the form `id:a or id:b or ...`.
    pub ids: Vec<String>,
    /// Tags to add (no `+` prefix; this layer adds it).
    #[serde(default)]
    pub add: Vec<String>,
    /// Tags to remove (no `-` prefix; this layer adds it).
    #[serde(default)]
    pub remove: Vec<String>,
}

/// Body of POST `/api/trash`, `/api/archive`, and friends.
///
/// Accepts either explicit message ids (preferred when known) or thread
/// ids (used when the listing row represents a multi-message thread —
/// `Envelope::message_id()` returns None for those, so the per-row JS
/// has no single id to send. Still, the user's intent is "act on this
/// row" → "act on this thread", which is the Gmail-style convention).
///
/// At least one of `ids` / `thread_ids` should be non-empty; both empty
/// short-circuits to a no-op success in [`run_tag_changes`].
#[derive(Debug, Deserialize, Default)]
pub struct IdsRequest {
    #[serde(default)]
    pub ids: Vec<String>,
    #[serde(default)]
    pub thread_ids: Vec<String>,
}

/// Standard JSON response shape.
#[derive(Debug, Serialize)]
pub struct TagResponse {
    pub ok: bool,
    /// Number of messages affected (= len(ids) on success). Set to 0
    /// when the request was a no-op.
    pub affected: usize,
    /// Empty on success, error string on failure.
    pub error: Option<String>,
}

/// Shared implementation for all tag-mutating endpoints. Folds the ids
/// into a notmuch query, atomically applies the add/remove sets via
/// `notmuch tag`, and returns a TagResponse with the appropriate HTTP
/// status: 200 OK on success (so the client's `r.ok` check works
/// unambiguously), 500 on notmuch failure with the error string in the
/// JSON body for diagnostics. Empty `ids` short-circuits to a no-op
/// success — `notmuch_db::ids_to_query(&[])` would otherwise produce
/// notmuch-illegal empty query string.
fn run_tag_changes(
    ids: &[String],
    thread_ids: &[String],
    add: &[&str],
    remove: &[&str],
) -> (StatusCode, Json<TagResponse>) {
    if ids.is_empty() && thread_ids.is_empty() {
        return (
            StatusCode::OK,
            Json(TagResponse { ok: true, affected: 0, error: None }),
        );
    }
    // Build a notmuch query that ORs id-clauses and thread-clauses
    // together. Either side may be empty; only the non-empty halves
    // contribute. Thread queries expand to all messages in those
    // threads, so a multi-message thread row becomes "act on every
    // message of this thread" — the Gmail convention.
    let mut clauses: Vec<String> = Vec::with_capacity(ids.len() + thread_ids.len());
    for id in ids {
        clauses.push(format!("id:{id}"));
    }
    for tid in thread_ids {
        clauses.push(format!("thread:{tid}"));
    }
    let query = clauses.join(" or ");
    match notmuch_db::apply_tag_changes(&query, add, remove) {
        Ok(_) => (
            StatusCode::OK,
            Json(TagResponse {
                ok: true,
                // We don't know the actual count when thread_ids are
                // involved without a separate notmuch search; report
                // the request shape (one count per submitted id/thread
                // is "good enough" for the UI's success toast).
                affected: ids.len() + thread_ids.len(),
                error: None,
            }),
        ),
        Err(e) => {
            tracing::warn!("tag op failed (add={add:?} remove={remove:?}): {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(TagResponse {
                    ok: false,
                    affected: 0,
                    error: Some(e.to_string()),
                }),
            )
        }
    }
}

/// POST `/api/tag` — apply arbitrary add/remove to a list of ids.
pub async fn tag_post(Json(req): Json<TagRequest>) -> impl IntoResponse {
    let add: Vec<&str> = req.add.iter().map(|s| s.as_str()).collect();
    let remove: Vec<&str> = req.remove.iter().map(|s| s.as_str()).collect();
    run_tag_changes(&req.ids, &[], &add, &remove)
}

/// POST `/api/trash`. Sugar for `+trash -inbox` (matches meli config
/// trash command in `~/.config/meli/config.toml`).
///
/// Implementation note: meli's config also sets `flag set trash` on the
/// maildir file; that's a meli-specific UI cue. mailforge doesn't need it
/// because the client-side keyboard handler removes the row from the DOM
/// optimistically. The maildir T flag will be set by the next
/// `gmail-push-tags` run for Gmail; for COHS, `cohs-trash-mover` handles
/// the file relocation directly.
pub async fn trash_post(Json(req): Json<IdsRequest>) -> impl IntoResponse {
    run_tag_changes(&req.ids, &req.thread_ids, &["trash"], &["inbox"])
}

/// POST `/api/archive`. Sugar for `-inbox` (no add — archive is just the
/// absence of inbox). Equivalent to meli's archive workflow.
pub async fn archive_post(Json(req): Json<IdsRequest>) -> impl IntoResponse {
    run_tag_changes(&req.ids, &req.thread_ids, &[], &["inbox"])
}

/// POST `/api/seen`. Sugar for `-unread`. Marks message(s) as read locally.
/// Server-side propagation: `gmail-push-tags` translates `-unread` to
/// removing the Gmail UNREAD label; mbsync replicates the maildir Seen
/// flag for COHS via mbsync's own flag-tracking on next sync.
pub async fn seen_post(Json(req): Json<IdsRequest>) -> impl IntoResponse {
    run_tag_changes(&req.ids, &req.thread_ids, &[], &["unread"])
}

/// POST `/api/unarchive`. Inverse of [`archive_post`]. Adds `inbox` and
/// removes `archive` so the message reappears in its account's inbox view.
///
/// Both tag changes are applied even on messages that already lack the
/// archive tag (personal archives use absence-of-inbox, not an explicit
/// archive tag, so `-archive` is a harmless no-op there). For COHS
/// archives that DO carry an explicit `archive` tag, the removal is
/// what clears them out of the archive view; the `+inbox` is what
/// restores them to the inbox view. One handler covers both conventions.
pub async fn unarchive_post(Json(req): Json<IdsRequest>) -> impl IntoResponse {
    run_tag_changes(&req.ids, &req.thread_ids, &["inbox"], &["archive"])
}

/// POST `/api/untrash`. Inverse of [`trash_post`]. Adds `inbox` and
/// removes `trash` so the message reappears in its account's inbox view.
///
/// Bound to `D` in the listing context, mirroring `A`'s relationship to
/// `a` for the archive pair. Only meaningful when called from the trash
/// view; calling it on a non-trashed message is a harmless no-op (the
/// `-trash` finds nothing to remove and the `+inbox` either already
/// applies or is a no-op for a message already in inbox).
pub async fn untrash_post(Json(req): Json<IdsRequest>) -> impl IntoResponse {
    run_tag_changes(&req.ids, &req.thread_ids, &["inbox"], &["trash"])
}
