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
//! mailpost touches the local notmuch DB only. Server-side mirroring
//! happens via existing infrastructure:
//! - Personal (Gmail): `gmail-push-tags` (15-min launchd timer) translates
//!   notmuch tags to Gmail label changes.
//! - COHS (M365): `cohs-trash-mover` (5-min timer) moves trash-tagged
//!   messages to the M365 Deleted Items folder; mbsync replicates back.
//!
//! End-to-end latency: 0-20 minutes per existing meli config docs. No
//! change needed for mailpost.
//!
//! ## Concurrency
//!
//! `notmuch tag` is atomic per invocation. Multiple parallel requests are
//! safe. notmuch's lock-retry mechanism (the `built_with.retry_lock=true`
//! flag from `notmuch config list`) handles concurrent writers.

use axum::{response::IntoResponse, Json};
use serde::{Deserialize, Serialize};

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

/// POST `/api/tag` — apply arbitrary add/remove to a list of ids.
pub async fn tag_post(Json(_req): Json<TagRequest>) -> impl IntoResponse {
    todo!(
        "1. if req.ids.is_empty(): return ok=true, affected=0\n\
         2. query = notmuch_db::ids_to_query(&req.ids)\n\
         3. apply_tag_changes(&query, &req.add.iter().map(|s| s.as_str()).collect::<Vec<_>>(),\n\
                                       &req.remove.iter().map(|s| s.as_str()).collect::<Vec<_>>())\n\
         4. on err: return ok=false, error=Some(e.to_string())\n\
         5. on ok: return ok=true, affected=req.ids.len()"
    );
    #[allow(unreachable_code)]
    Json(TagResponse {
        ok: false,
        affected: 0,
        error: None,
    })
}

/// POST `/api/trash`. Sugar for `+trash -inbox` (matches meli config
/// trash command in `~/.config/meli/config.toml`).
///
/// Implementation note: meli's config also sets `flag set trash` on the
/// maildir file; that's a meli-specific UI cue (forces a refresh of the
/// row). mailpost doesn't need it because the client-side keyboard
/// handler removes the row from the DOM optimistically. The maildir T
/// flag will be set by the next `gmail-push-tags` run for Gmail; for
/// COHS, `cohs-trash-mover` handles the file relocation directly.
pub async fn trash_post(Json(_req): Json<IdsRequest>) -> impl IntoResponse {
    todo!(
        "delegate to apply_tag_changes(query, &[\"trash\"], &[\"inbox\"])"
    );
    #[allow(unreachable_code)]
    Json(TagResponse {
        ok: false,
        affected: 0,
        error: None,
    })
}

/// POST `/api/archive`. Sugar for `-inbox` (no add — archive is just the
/// absence of inbox). Equivalent to meli's archive workflow.
pub async fn archive_post(Json(_req): Json<IdsRequest>) -> impl IntoResponse {
    todo!(
        "delegate to apply_tag_changes(query, &[], &[\"inbox\"])"
    );
    #[allow(unreachable_code)]
    Json(TagResponse {
        ok: false,
        affected: 0,
        error: None,
    })
}
