//! Cross-mailbox search.
//!
//! GET `/mail/search?q=<query>&page=N`
//!
//! Same rendering as `listing::list_mailbox` (table of envelopes, paginator)
//! but the query is the user's raw notmuch input — no account/mailbox
//! prefixing. Lets the user type things like `from:stripe date:7d..` or
//! `tag:billing and not tag:trash`.
//!
//! ## Why separate from listing
//!
//! - URL shape: `/mail/search?q=...` is bookmarkable and has no
//!   `<account>/<mailbox>` slot.
//! - Sidebar highlight: search has no active mailbox; the sidebar
//!   renders nothing highlighted.
//! - Helpbar: different bindings make sense in search context (e.g. no
//!   `D` for trash — multi-account selection makes mass trash error-prone).
//!
//! ## Future: saved searches
//!
//! If the user wants to save searches, add a `~/.config/mailforge/saved-searches.toml`
//! file. Each entry would be (name, query). Render in the sidebar under a
//! "Saved" heading. Out of scope for the first build.

use axum::{
    extract::Query,
    response::{Html, IntoResponse, Response},
};
use maud::html;
use serde::Deserialize;

use crate::mail::templates::{self, status_banner, PageContext};

#[derive(Debug, Default, Deserialize)]
pub struct SearchQuery {
    /// User's raw notmuch query string. Empty / missing renders the
    /// search form only (no results table).
    #[serde(default)]
    pub q: Option<String>,
    /// 0-indexed page number.
    #[serde(default)]
    pub page: Option<usize>,
}

/// GET `/mail/search`.
///
/// Placeholder until the full search experience is built. Returns a
/// 200 HTML page with a "not yet implemented" notice, so the route is
/// reachable from the sidebar / `/` shortcut without panicking the
/// handler. See the module-level doc-comment for the intended design.
pub async fn search_get(Query(_q): Query<SearchQuery>) -> Response {
    let body = html! {
        (status_banner("Search", Some("Not yet implemented")))
        div class="empty-state panel" {
            h2 { "Search not yet implemented" }
            p {
                "Cross-mailbox search is on the roadmap. For now, use the "
                "per-mailbox filter in the listing view (press "
                code { "/" }
                " to focus the filter input)."
            }
        }
    };
    let doc = templates::page(
        "Search — MailForge",
        PageContext::Search,
        None,
        None,
        body,
    );
    Html(doc).into_response()
}
