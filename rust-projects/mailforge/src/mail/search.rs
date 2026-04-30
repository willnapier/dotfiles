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

#[allow(unused_imports)]
use axum::{
    extract::Query,
    response::{IntoResponse, Response},
};
use serde::Deserialize;

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
/// If `q` is empty/missing: render the search form alone.
/// If `q` is present: render the form pre-filled with `q`, plus the
/// envelope-list result table.
pub async fn search_get(Query(_q): Query<SearchQuery>) -> Response {
    todo!(
        "1. if q.q is None or empty: render form-only via templates::page(Search, ...)\n\
         2. else:\n\
            - search(query, page * PER_PAGE, PER_PAGE) + count(query)\n\
            - render form (prefilled) + envelope rows + paginator\n\
         3. handle notmuch syntax errors: surface as banner above results,\n\
            not 500 (notmuch returns non-zero exit on parse errors;\n\
            wrap as user-facing message)"
    )
}
