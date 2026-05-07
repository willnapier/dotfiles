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

use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use maud::html;
use serde::Deserialize;

use crate::mail::accounts;
use crate::mail::notmuch_db;
use crate::mail::templates::{
    self, envelope_row_indexed, mailbox_label, paginator_with_query, status_banner, PageContext,
};
use crate::mail::unsubscribe;

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
    let acc = accounts::default_account();
    Redirect::to(&format!("/mail/{}/inbox", acc.slug))
}

/// GET `/mail/<account>/<mailbox>`.
///
/// Query string: `page` (default 0), `q` (default empty).
///
/// Renders the mailbox table plus pagination. Returns 404 for unknown
/// account or mailbox.
pub async fn list_mailbox(
    Path((account_slug, mailbox)): Path<(String, String)>,
    Query(q): Query<ListingQuery>,
) -> Response {
    // 1. Resolve account.
    let Some(account) = accounts::find(&account_slug) else {
        return (
            StatusCode::NOT_FOUND,
            format!("unknown account: {account_slug}"),
        )
            .into_response();
    };

    // 2. Build mailbox query.
    let Some(mailbox_q) = notmuch_db::mailbox_query(account, &mailbox) else {
        return (
            StatusCode::NOT_FOUND,
            format!("unknown mailbox: {account_slug}/{mailbox}"),
        )
            .into_response();
    };

    // 3. AND the user filter onto the mailbox query, parenthesised so
    //    notmuch's parser treats them as discrete sub-expressions.
    let user_filter = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let final_query = match user_filter {
        Some(uq) => format!("({mailbox_q}) and ({uq})"),
        None => mailbox_q,
    };

    let page = q.page.unwrap_or(0);
    let offset = page.saturating_mul(PER_PAGE);

    // 4. Run search + count. We accept whatever the notmuch_db layer
    //    returns; on error we render an empty page with an error banner
    //    rather than 500ing.
    let envelopes_result = notmuch_db::search(&final_query, offset, PER_PAGE);
    let total_result = notmuch_db::count(&final_query);

    let (envelopes, fetch_error) = match envelopes_result {
        Ok(envs) => (envs, None),
        Err(e) => (Vec::new(), Some(format!("search failed: {e}"))),
    };
    let total = total_result.unwrap_or(0);

    // Pre-load List-Unsubscribe presence per envelope, batched: a
    // single `notmuch show --format=json` call resolves all the
    // file paths for the page, then we read each file and parse the
    // List-Unsubscribe header locally. ~50ms total for a 50-message
    // page (vs the 2.5s the per-message variant cost). Defensive on
    // failures: any per-message lookup miss defaults to false; we
    // never bubble parse errors to the listing render.
    let mut envelopes = envelopes;
    let single_msg_ids: Vec<String> = envelopes
        .iter()
        .filter_map(|e| e.message_id().map(|s| s.to_string()))
        .collect();
    if !single_msg_ids.is_empty() {
        let presence = unsubscribe::batch_check_unsubscribe(&single_msg_ids);
        // Re-walk envelopes in the same order; bool vector is aligned
        // 1:1 with single_msg_ids, so we step through it as we encounter
        // each single-message-thread row.
        let mut i = 0;
        for env in envelopes.iter_mut() {
            if env.message_id().is_some() {
                if let Some(b) = presence.get(i) {
                    env.has_unsubscribe = *b;
                }
                i += 1;
            }
        }
    }

    let base_url = format!("/mail/{}/{}", account.slug, mailbox);
    let from_ctx = format!("{}/{}", account.slug, mailbox);
    let extra_query = user_filter.map(|q| format!("q={}", url::form_urlencoded::byte_serialize(q.as_bytes()).collect::<String>()));

    let mailbox_label = mailbox_label(&mailbox);
    let banner_subtitle = match (envelopes.is_empty(), total) {
        (true, 0) => "No messages.".to_string(),
        (_, n) => format!("{n} messages — page {} of {}", page + 1, total_pages(n, PER_PAGE)),
    };

    let body = html! {
        (status_banner(
            &format!("{} — {}", mailbox_label, account.display_name),
            Some(&banner_subtitle),
        ))

        // In-mailbox search/filter form. The keyboard JS focuses this
        // when the user presses `/`. Sweep used to live here as a
        // toolbar button; it's now per-row hover-reveal (see the
        // .row-action--sweep icon in templates.rs::envelope_row_indexed).
        // Rationale: the curatorial impulse fires WHILE looking at a
        // row, not at the toolbar — keep the affordance at zero clicks
        // from the trigger moment. Same pattern for the new
        // List-Unsubscribe icon.
        form class="mailbox-filter" method="get" action=(base_url) {
            input type="text"
                name="q"
                value=[user_filter.as_deref()]
                placeholder="Filter this mailbox…"
                title="notmuch query syntax — e.g. tag:unread, from:alice, subject:invoice"
                aria-label="Filter mailbox"
                id="mailbox-filter-input";
            button type="submit" { "Filter" }
            @if user_filter.is_some() {
                a class="mailbox-filter__clear" href=(base_url) { "Clear" }
            }
            // Inline collapsible cheat-sheet — same content as the
            // /mail/search variant. Sits next to the Filter/Clear
            // controls when closed; flex-wrap pushes the expanded
            // help to its own full-width row when open.
            details class="search-help-toggle" {
                summary { "Syntax help" }
                (crate::mail::search::search_help())
            }
        }

        @if let Some(err) = &fetch_error {
            div class="banner banner--error" role="alert" {
                strong { "Search error: " }
                (err)
            }
        }

        @if envelopes.is_empty() && fetch_error.is_none() {
            div class="empty-state panel" {
                h2 { "Empty mailbox" }
                p { "No messages match this view." }
            }
        } @else {
            // Native <table> semantics; previously declared
            // role="grid" but lacked the row/cell roles, aria-selected,
            // and roving tabindex that ARIA grid expects. Bare <table>
            // is what AT actually consumes for tabular mail listings.
            table class="listing" aria-label="Messages" {
                thead {
                    tr {
                        // Resize handles on every fixed-width column.
                        // The `col-resizer` element captures mousedown
                        // and the JS in keys.js drives the live drag
                        // + localStorage persistence. Subject column
                        // is auto-grow so doesn't need a handle (it
                        // takes whatever space is left).
                        th class="col-from" {
                            "From"
                            span class="col-resizer" data-col="from" aria-hidden="true" {}
                        }
                        th class="col-tags" {
                            "Tags"
                            span class="col-resizer" data-col="tags" aria-hidden="true" {}
                        }
                        th class="col-subject" { "Subject" }
                        th class="col-date" {
                            "Date"
                            span class="col-resizer" data-col="date" aria-hidden="true" {}
                        }
                        // Empty header for the per-row hover-reveal
                        // actions column (sweep / unsubscribe icons).
                        // Rows without actions render an empty cell,
                        // preserving visual alignment.
                        th class="col-actions" aria-hidden="true" {
                            span class="col-resizer" data-col="actions" aria-hidden="true" {}
                        }
                    }
                }
                tbody {
                    @for (idx, env) in envelopes.iter().enumerate() {
                        (envelope_row_indexed(env, idx, Some(&from_ctx)))
                    }
                }
            }

            (paginator_with_query(
                page,
                total,
                PER_PAGE,
                &base_url,
                extra_query.as_deref(),
            ))
        }
    };

    let title = format!(
        "{} ({}) — MailForge",
        mailbox_label, account.display_name
    );
    let doc = templates::page(
        &title,
        PageContext::Listing,
        Some(account.slug),
        Some(&mailbox),
        body,
    );
    Html(doc).into_response()
}

fn total_pages(total: u64, per_page: usize) -> u64 {
    let per_page = per_page.max(1) as u64;
    if total == 0 {
        1
    } else {
        ((total - 1) / per_page) + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_math_handles_empty() {
        assert_eq!(total_pages(0, 50), 1);
    }

    #[test]
    fn pages_math_handles_exact_multiples() {
        assert_eq!(total_pages(50, 50), 1);
        assert_eq!(total_pages(100, 50), 2);
    }

    #[test]
    fn pages_math_handles_partial_last_page() {
        assert_eq!(total_pages(51, 50), 2);
        assert_eq!(total_pages(101, 50), 3);
    }

    // mailbox_label test moved to templates.rs alongside the
    // canonical implementation (was duplicated here per audit #19).
}
