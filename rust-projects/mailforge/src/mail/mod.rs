//! mailforge — browser-native mail client UI.
//!
//! A bolted-on submodule of the `mailforge` daemon. Adds full mailbox listing,
//! message reading, composing, sending, and tagging via Axum routes under
//! `/mail/...` and `/api/...`. Renders HTML on the server using maud
//! templates. Minimal client-side JS (vanilla, no framework) handles
//! keyboard shortcuts and a small set of XHR-driven actions (tag, trash,
//! send, autosave).
//!
//! See `~/Assistants/shared/mailforge-design.md` for the full design doc
//! (architecture decisions, URL scheme, data flow, migration plan, scope
//! estimate per phase).
//!
//! ## Submodule layout
//!
//! - [`accounts`]: Static account → notmuch-tag-prefix → send-backend mapping.
//! - [`notmuch_db`]: CLI subprocess wrapper. Search, show, tag, count.
//! - [`listing`]: GET `/mail/<account>/<mailbox>` — mailbox table view.
//! - [`message`]: GET `/mail/m/<msg-id>` and `/mail/t/<thread-id>` — read view.
//! - [`compose`]: GET `/mail/compose`, POST `/api/send`, POST `/api/draft`.
//! - [`tag`]: POST `/api/tag`, POST `/api/trash`, POST `/api/archive`.
//! - [`search`]: GET `/mail/search` — cross-mailbox search.
//! - [`templates`]: maud HTML helpers (layout, envelope row, sidebar, etc.).
//!
//! ## Router contract
//!
//! [`router()`] returns an [`axum::Router`] that the daemon merges into the
//! main router under no path prefix (the routes themselves carry `/mail` /
//! `/api` / `/static` prefixes). The daemon stays responsible for binding,
//! the `127.0.0.1` lock-down, and the `/v/<uuid>` viewer routes.
//!
//! ## Why no `MailState` here
//!
//! All mail handlers are stateless w.r.t. the in-process server: notmuch
//! holds the index, the filesystem holds the messages, pizauth holds the
//! tokens. The daemon's existing [`AppState`](crate::daemon::AppState) is
//! sufficient for the viewer routes; mail handlers fetch ad-hoc from the
//! filesystem on each request.
//!
//! If draft autosave grows enough state to need an Arc<Mutex<...>>, add it
//! here as `MailState` and pass via `Router::with_state(...)`. Until then
//! the simpler global-CLI shape wins.

// Scaffold-only allow: the public types and functions in this module tree
// are referenced by `router()` (so they're reachable) but their bodies are
// `todo!()` placeholders. Without this allow, every Envelope/Message/handler
// generates a "never used" warning that drowns the real signal. Remove this
// once the first wave of implementation agents fills in the handler bodies.
#![allow(dead_code)]

pub mod accounts;
pub mod addresses;
pub mod auth_results;
pub mod compose;
pub mod curator;
pub mod listing;
pub mod message;
pub mod notmuch_db;
pub mod pull;
pub mod search;
pub mod tag;
pub mod templates;
pub mod trusted_senders;
pub mod unsubscribe;

use axum::Router;
use axum::routing::{get, post};

/// Build the mailforge subrouter.
///
/// All handlers currently `todo!()`; this function returns a real Router so
/// route registration in [`crate::daemon`] compiles even before the
/// implementation agents fill in the bodies.
///
/// Call from `daemon::run` as:
///
/// ```ignore
/// let app = existing_router.merge(crate::mail::router());
/// ```
///
/// Note: the existing `/v/<uuid>/*` routes stay registered in
/// `daemon.rs`. This subrouter adds `/mail/*`, `/api/*`, and `/static/*`.
pub fn router() -> Router {
    Router::new()
        // Mail UI (HTML pages)
        .route("/mail", get(listing::inbox_redirect))
        .route("/mail/:account/:mailbox", get(listing::list_mailbox))
        .route("/mail/m/:id", get(message::show_message))
        .route("/mail/t/:thread_id", get(message::show_thread))
        .route("/mail/compose", get(compose::compose_form))
        .route("/mail/draft/:id", get(compose::draft_get))
        .route("/mail/search", get(search::search_get))
        // JSON / form-handling APIs
        .route("/api/tag", post(tag::tag_post))
        .route("/api/trash", post(tag::trash_post))
        .route("/api/archive", post(tag::archive_post))
        .route("/api/unarchive", post(tag::unarchive_post))
        .route("/api/seen", post(tag::seen_post))
        .route("/api/send", post(compose::send_post))
        .route("/api/draft", post(compose::draft_save))
        .route("/api/draft/:id", get(compose::draft_get_api))
        .route("/api/escalate-helix", post(compose::escalate_helix))
        .route("/api/escalate-helix/status", get(compose::escalate_helix_status))
        .route("/api/escalate-helix/abort", post(compose::escalate_helix_abort))
        .route("/api/mailcurator/sweep", post(curator::sweep_post))
        .route("/api/mailcurator/blacklist", post(curator::blacklist_post))
        // Address-book autocomplete for compose To/Cc/Bcc fields. Reads
        // from an in-memory cache populated by `notmuch address` (built
        // lazily on first hit, refreshed every 10 min by the background
        // task spawned in `daemon::run`).
        .route("/api/addresses", get(addresses::addresses_get))
        // On-demand pull: invokes `gmpull pull --resume && notmuch new`
        // and returns synchronously. The Ctrl+R refresh handler awaits
        // this so the listing re-renders with newly-arrived mail.
        .route("/api/pull-now", post(pull::pull_now_post))
        .route("/api/unsubscribe/probe", get(unsubscribe::probe_get))
        .route("/api/unsubscribe/execute", post(unsubscribe::execute_post))
        .route("/api/unsubscribe/trash-from-sender", post(unsubscribe::trash_from_sender_post))
        // HTML auto-render trust list (per-domain). Add/remove only —
        // the JSON file at ~/.config/mailforge/html-trusted-senders.json
        // IS the listing surface for debugging.
        .route("/api/html-trusted/add", post(trusted_senders::add_post))
        .route("/api/html-trusted/remove", post(trusted_senders::remove_post))
        // Static assets (CSS, JS) — ServeDir registered in daemon.rs
        // because tower_http's ServeDir is easier to compose at the
        // outer Router level. Path: /static/*.
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    /// Smoke test: the unsubscribe routes are registered and don't 404
    /// at the routing layer. We send a HEAD/empty-body request and
    /// accept any non-404 status (the handlers themselves will fail
    /// without a real notmuch DB, but the routing layer must match).
    #[tokio::test]
    async fn unsubscribe_routes_are_registered() {
        let app = router();

        // GET /api/unsubscribe/probe — without ?id this 400s on the
        // Query extractor; that's fine, we only assert "not 404".
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/unsubscribe/probe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "probe route must be registered; got {}",
            resp.status()
        );

        // POST /api/unsubscribe/execute — same idea.
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/unsubscribe/execute")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "execute route must be registered; got {}",
            resp.status()
        );

        // Wrong method on probe → 405, not 404.
        let app = router();
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/unsubscribe/probe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "POST on probe must be 405 (method exists, wrong verb)"
        );
    }
}
