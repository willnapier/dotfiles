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
    self, envelope_row_indexed, paginator_with_query, status_banner, PageContext,
};

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

    let base_url = format!("/mail/{}/{}", account.slug, mailbox);
    let from_ctx = format!("{}/{}", account.slug, mailbox);
    let extra_query = user_filter.map(|q| format!("q={}", url::form_urlencoded::byte_serialize(q.as_bytes()).collect::<String>()));

    let mailbox_label = mailbox_label_for(&mailbox);
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
        // when the user presses `/`.
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
            // Sweep button — runs `mailcurator run --now --only <policy>`
            // scoped to whichever mailcurator policy matches the current
            // cursor row. Fast (no extractor overhead) and intentional
            // (you're looking at a row, you sweep its kind). JS lives in
            // keys.js (sweepNow()).
            button type="button" class="mailbox-filter__sweep"
                data-action="sweep-now"
                title="Sweep messages like this one (matched by the same mailcurator policy as the row your cursor is on)."
            { "Sweep like this" }
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
            table class="listing" role="grid" aria-label="Messages" {
                thead {
                    tr {
                        th class="col-from"    { "From" }
                        th class="col-tags"    { "Tags" }
                        th class="col-subject" { "Subject" }
                        th class="col-date"    { "Date" }
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

/// Display label for a mailbox slug. Mirrors templates::mailbox_label
/// but kept here so the listing handler doesn't need to reach into the
/// templates module's privates.
fn mailbox_label_for(slug: &str) -> String {
    match slug {
        "all-mail" => "All Mail".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        }
    }
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

    #[test]
    fn mailbox_label_capitalises() {
        assert_eq!(mailbox_label_for("inbox"), "Inbox");
        assert_eq!(mailbox_label_for("sent"), "Sent");
        assert_eq!(mailbox_label_for("all-mail"), "All Mail");
    }
}
