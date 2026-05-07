//! Cross-mailbox search.
//!
//! GET `/mail/search?q=<query>&page=N`
//!
//! Same rendering as `listing::list_mailbox` (table of envelopes, paginator)
//! but the query is the user's raw notmuch input — no account/mailbox
//! prefixing. Lets the user type things like `from:stripe date:7d..` or
//! `tag:billing and not tag:trash`.
//!
//! Plain words match message bodies and headers (notmuch indexes both via
//! Xapian). The 217k-message store searches in 50-200ms.
//!
//! ## Why separate from listing
//!
//! - URL shape: `/mail/search?q=...` is bookmarkable and has no
//!   `<account>/<mailbox>` slot.
//! - Sidebar highlight: search has no active mailbox; the sidebar
//!   renders nothing highlighted (PageContext::Search + None/None).
//! - `from_ctx` on result rows is None — search spans mailboxes, so
//!   prev/next sibling navigation from a clicked result has no anchor.
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
use maud::{html, Markup};
use serde::Deserialize;

use crate::mail::notmuch_db;
use crate::mail::templates::{
    self, envelope_row_indexed, paginator_with_query, status_banner, PageContext,
};
use crate::mail::unsubscribe;

const PER_PAGE: usize = 50;

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
pub async fn search_get(Query(q): Query<SearchQuery>) -> Response {
    let user_query = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let page = q.page.unwrap_or(0);

    let body = match user_query {
        None => render_form_only(),
        Some(uq) => render_results(uq, page),
    };

    let title = match user_query {
        Some(uq) => format!("Search: {uq} — MailForge"),
        None => "Search — MailForge".to_string(),
    };
    let doc = templates::page(&title, PageContext::Search, None, None, body);
    Html(doc).into_response()
}

fn render_form_only() -> Markup {
    html! {
        (status_banner("Search", Some("Across all mailboxes (notmuch full-text)")))
        (search_form(None))
        (search_help())
    }
}

fn render_results(query: &str, page: usize) -> Markup {
    let offset = page.saturating_mul(PER_PAGE);
    let envelopes_result = notmuch_db::search(query, offset, PER_PAGE);
    let total_result = notmuch_db::count(query);

    let (mut envelopes, fetch_error) = match envelopes_result {
        Ok(envs) => (envs, None),
        Err(e) => (Vec::new(), Some(format!("search failed: {e}"))),
    };
    let total = total_result.unwrap_or(0);

    // Pre-load List-Unsubscribe presence (mirrors listing.rs).
    let single_msg_ids: Vec<String> = envelopes
        .iter()
        .filter_map(|e| e.message_id().map(|s| s.to_string()))
        .collect();
    if !single_msg_ids.is_empty() {
        let presence = unsubscribe::batch_check_unsubscribe(&single_msg_ids);
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

    let banner_subtitle = match (envelopes.is_empty(), total) {
        (true, 0) => "No matches.".to_string(),
        (_, n) => format!(
            "{n} matches — page {} of {}",
            page + 1,
            total_pages(n, PER_PAGE)
        ),
    };

    let extra_query = format!(
        "q={}",
        url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>()
    );

    html! {
        (status_banner(
            &format!("Search: {query}"),
            Some(&banner_subtitle),
        ))
        (search_form(Some(query)))

        // Inline cheat-sheet, collapsed by default. Native <details>
        // (no JS) — keyboard-accessible (Tab + Enter/Space). Lets the
        // user consult syntax without losing the current query, which
        // is the friction in the "/, then //, lose your work" path.
        details class="search-help-toggle" {
            summary { "Syntax help" }
            (search_help())
        }

        @if let Some(err) = &fetch_error {
            div class="banner banner--error" role="alert" {
                strong { "Search error: " }
                (err)
            }
        }

        @if envelopes.is_empty() && fetch_error.is_none() {
            div class="empty-state panel" {
                h2 { "No matches" }
                p { "Refine your query and try again." }
            }
        } @else {
            // Table structure mirrors listing::list_mailbox so the same
            // CSS column widths and resize-handle persistence apply.
            table class="listing" aria-label="Search results" {
                thead {
                    tr {
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
                        th class="col-actions" aria-hidden="true" {
                            span class="col-resizer" data-col="actions" aria-hidden="true" {}
                        }
                    }
                }
                tbody {
                    @for (idx, env) in envelopes.iter().enumerate() {
                        (envelope_row_indexed(env, idx, None))
                    }
                }
            }

            (paginator_with_query(
                page,
                total,
                PER_PAGE,
                "/mail/search",
                Some(&extra_query),
            ))
        }
    }
}

fn search_form(prefill: Option<&str>) -> Markup {
    html! {
        form class="mailbox-filter" method="get" action="/mail/search" {
            input type="text"
                name="q"
                value=[prefill]
                placeholder="Search across all mailboxes…"
                title="notmuch query syntax — e.g. from:stripe, subject:invoice, date:7d.., tag:billing"
                aria-label="Search query"
                id="search-input"
                autofocus;
            button type="submit" { "Search" }
            @if prefill.is_some() {
                a class="mailbox-filter__clear" href="/mail/search" { "Clear" }
            }
        }
    }
}

pub fn search_help() -> Markup {
    html! {
        section class="empty-state panel search-help" {
            p class="search-help__lede" {
                "Notmuch full-text query against headers, bodies, and decoded attachment text. Plain words match anywhere; "
                code { "field:" }
                " prefixes scope the match. Combine with "
                code { "and" }
                " / "
                code { "or" }
                " / "
                code { "not" }
                " (parens for grouping)."
            }

            div class="search-help__cols" {
                section {
                    h4 { "Basics" }
                    ul {
                        li { code { "vacation" } " — plain word in body or headers" }
                        li { code { "\"annual leave\"" } " — exact phrase (quoted)" }
                        li { code { "vacation OR holiday" } " — either" }
                        li { code { "vacation and not 2024" } " — negation" }
                        li { code { "(alice or bob) and stripe" } " — grouping" }
                    }
                }

                section {
                    h4 { "People & subject" }
                    ul {
                        li { code { "from:stripe" } " — sender contains" }
                        li { code { "to:will@willnapier.com" } " — direct recipient" }
                        li { code { "cc:olly" } " — CC line" }
                        li { code { "bcc:..." } " — BCC line (if header preserved)" }
                        li { code { "subject:invoice" } " — Subject contains" }
                        li { code { "subject:\"order confirmation\"" } " — Subject phrase" }
                    }
                }

                section {
                    h4 { "Dates" }
                    ul {
                        li { code { "date:today" } " · " code { "date:yesterday" } " · " code { "date:\"last week\"" } }
                        li { code { "date:7d.." } " — last 7 days (open-ended)" }
                        li { code { "date:..30d" } " — older than 30 days" }
                        li { code { "date:2026-01-01.." } " — since absolute date" }
                        li { code { "date:2026-01-01..2026-04-30" } " — bounded range" }
                        li { code { "date:1y.." } " — last year" }
                    }
                }

                section {
                    h4 { "Tags & state" }
                    ul {
                        li { code { "tag:unread" } " · " code { "tag:inbox" } " · " code { "tag:archive" } " · " code { "tag:trash" } " · " code { "tag:sent" } }
                        li { code { "tag:cohs" } " — COHS account messages only" }
                        li { code { "tag:promotions" } " — auto-classified promotions" }
                        li { code { "tag:unsubscribed" } " — messages where you clicked Unsubscribe (audit trail; usually also " code { "tag:trash" } ")" }
                        li { code { "tag:bulk-marketing" } " — mailcurator classification" }
                        li { code { "tag:curator-<policy>-seen" } " — already touched by named mailcurator policy" }
                    }
                }

                section {
                    h4 { "Attachments & MIME" }
                    ul {
                        li { code { "attachment:invoice.pdf" } " — by attachment filename" }
                        li { code { "mimetype:application/pdf" } " — any PDF attachment" }
                        li { code { "mimetype:image/*" } " — any image" }
                        li { code { "attachment:\"\"" } " — has any attachment" }
                    }
                }

                section {
                    h4 { "Regex (slashes)" }
                    ul {
                        li { code { "from:/.*@stripe\\.com$/" } " — sender domain regex" }
                        li { code { "subject:/^Re: /" } " — Subject anchored regex" }
                        li { code { "from:/(noreply|no-reply)@/" } " — alternation" }
                    }
                    p class="search-help__note" {
                        "Slashes delimit the regex; backslash to escape inside. Xapian regex is roughly POSIX-extended."
                    }
                }

                section {
                    h4 { "Folder & path" }
                    ul {
                        li { code { "folder:personal/inbox" } " — Maildir folder" }
                        li { code { "path:personal/inbox/" } " — file path prefix" }
                        li { code { "thread:0000000000031577" } " — all messages in a thread" }
                    }
                }

                section {
                    h4 { "Real-world recipes" }
                    ul {
                        li { code { "from:stripe and date:30d.. and tag:unread" } }
                        li { code { "tag:cohs and from:nhs.uk and date:6M.." } }
                        li { code { "subject:invoice and mimetype:application/pdf and date:1y.." } }
                        li { code { "tag:promotions and date:..30d and not tag:trash" } " — old promo to triage" }
                        li { code { "from:/.*@(stripe|paddle|gocardless)\\.com/" } " — billing senders" }
                    }
                }
            }

            p class="search-help__footer" {
                "Full reference: "
                a href="https://notmuchmail.org/manpages/notmuch-search-terms-7/" target="_blank" rel="noopener" {
                    "notmuch-search-terms(7)"
                }
                " · "
                a href="https://notmuchmail.org/manpages/notmuch-search-7/" target="_blank" rel="noopener" {
                    "notmuch-search(7)"
                }
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
}
