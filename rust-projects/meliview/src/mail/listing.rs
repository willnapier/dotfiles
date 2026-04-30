//! Mailbox listing handler.
//!
//! GET `/mail` → 302 redirect to default mailbox.
//! GET `/mail/<account>/<mailbox>` → table view of the messages in that
//! mailbox.
//! GET `/mail/<account>/<mailbox>?page=N&q=...` → paginated and/or
//! filtered.
//!
//! ## Data flow
//!
//! 1. Resolve `account` slug via `accounts::find` (404 on unknown).
//! 2. Build query via `notmuch_db::mailbox_query(account, mailbox)`
//!    (404 on unknown mailbox name).
//! 3. If `q` query-string is present, AND it into the mailbox query as
//!    a sub-clause: `(<mailbox query>) and (<user query>)`. Quote
//!    appropriately to avoid notmuch syntax surprises (use parens).
//! 4. Call `notmuch_db::search(query, offset, PER_PAGE)` and
//!    `notmuch_db::count(query)`.
//! 5. Render via `templates::page` + `templates::envelope_row` +
//!    `templates::paginator`.
//!
//! ## Pagination
//!
//! `PER_PAGE = 50` is a sensible default — large enough to fill a typical
//! viewport, small enough that notmuch search stays fast. `?page=0` is
//! the first page; offset = page * PER_PAGE.

#[allow(unused_imports)]
use axum::{
    extract::{Path, Query},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;

#[allow(dead_code)]
const PER_PAGE: usize = 50;

/// Query string parameters for `/mail/<account>/<mailbox>`.
#[derive(Debug, Default, Deserialize)]
pub struct ListingQuery {
    /// 0-indexed page number. Default 0 (first page).
    #[serde(default)]
    pub page: Option<usize>,
    /// Optional in-mailbox search filter. AND'd into the mailbox query.
    #[serde(default)]
    pub q: Option<String>,
}

/// GET `/mail` → 302 to `/mail/<default>/inbox`.
pub async fn inbox_redirect() -> Redirect {
    let acc = crate::mail::accounts::default_account();
    Redirect::to(&format!("/mail/{}/inbox", acc.slug))
}

/// GET `/mail/<account>/<mailbox>`.
///
/// Query string: `page` (default 0), `q` (default empty).
///
/// Renders the mailbox table plus pagination. Returns 404 for unknown
/// account or mailbox.
pub async fn list_mailbox(
    Path((_account, _mailbox)): Path<(String, String)>,
    Query(_q): Query<ListingQuery>,
) -> Response {
    todo!(
        "1. resolve account via accounts::find (return 404 NOT_FOUND if missing)\n\
         2. translate to query via notmuch_db::mailbox_query (404 if missing)\n\
         3. AND in user query if q.q.is_some()\n\
         4. notmuch_db::search(query, page * PER_PAGE, PER_PAGE) + notmuch_db::count\n\
         5. render via templates::page(PageContext::Listing, ...)"
    )
}
