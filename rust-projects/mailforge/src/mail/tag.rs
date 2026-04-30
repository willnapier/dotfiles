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

/// Body of POST `/api/trash` and `/api/archive`.
#[derive(Debug, Deserialize)]
pub struct IdsRequest {
    pub ids: Vec<String>,
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
    add: &[&str],
    remove: &[&str],
) -> (StatusCode, Json<TagResponse>) {
    if ids.is_empty() {
        return (
            StatusCode::OK,
            Json(TagResponse { ok: true, affected: 0, error: None }),
        );
    }
    let query = notmuch_db::ids_to_query(ids);
    match notmuch_db::apply_tag_changes(&query, add, remove) {
        Ok(_) => (
            StatusCode::OK,
            Json(TagResponse {
                ok: true,
                affected: ids.len(),
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
    run_tag_changes(&req.ids, &add, &remove)
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
    run_tag_changes(&req.ids, &["trash"], &["inbox"])
}

/// POST `/api/archive`. Sugar for `-inbox` (no add — archive is just the
/// absence of inbox). Equivalent to meli's archive workflow.
pub async fn archive_post(Json(req): Json<IdsRequest>) -> impl IntoResponse {
    run_tag_changes(&req.ids, &[], &["inbox"])
}

/// POST `/api/seen`. Sugar for `-unread`. Marks message(s) as read locally.
/// Server-side propagation: `gmail-push-tags` translates `-unread` to
/// removing the Gmail UNREAD label; mbsync replicates the maildir Seen
/// flag for COHS via mbsync's own flag-tracking on next sync.
pub async fn seen_post(Json(req): Json<IdsRequest>) -> impl IntoResponse {
    run_tag_changes(&req.ids, &[], &["unread"])
}
