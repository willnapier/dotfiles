//! Shared HTML rendering helpers (maud).
//!
//! All UI handlers in `listing` / `message` / `compose` / `search` call into
//! this module to produce HTML. Centralising the layout chrome (sidebar,
//! header, helpbar, status flash) here means visual changes hit one file
//! and the handlers stay focused on data.
//!
//! ## Design conventions
//!
//! - **Document-level**: handlers return `Html<String>` from
//!   [`page`](self::page), which wraps a body fragment in the `<html>`,
//!   `<head>`, sidebar, helpbar, and footer.
//! - **Component fragments**: smaller helpers return `Markup` (maud's
//!   already-escaped HTML type), which compose. e.g.
//!   `envelope_row(&env)` is a `<tr>` that the listing handler folds
//!   into a `<table>`.
//! - **Context dataset**: every page sets `<body data-context="...">` so
//!   the keyboard JS (`static/js/keys.js`) can dispatch on context.
//!   Values: `listing`, `message`, `thread`, `compose`, `search`.
//! - **Solarized-dark only**: matches William's terminal theme. No light
//!   variant; this is a personal tool.
//!
//! ## Implementation status
//!
//! The function signatures and PageContext shape are stable; bodies are
//! `todo!()`. The implementation agent should fill them following the
//! design notes in `~/Assistants/shared/mailforge-design.md`.

use maud::{html, Markup, DOCTYPE};

use crate::mail::accounts::Account;
use crate::mail::notmuch_db::Envelope;

/// Marker for which UI context this page belongs to. Emitted as
/// `<body data-context="...">` so client-side JS can switch keymap
/// dispatch tables.
#[derive(Debug, Clone, Copy)]
pub enum PageContext {
    Listing,
    Message,
    Thread,
    Compose,
    Search,
}

impl PageContext {
    pub fn as_str(&self) -> &'static str {
        match self {
            PageContext::Listing => "listing",
            PageContext::Message => "message",
            PageContext::Thread => "thread",
            PageContext::Compose => "compose",
            PageContext::Search => "search",
        }
    }
}

/// Wrap a body fragment in the standard chrome (head + sidebar + helpbar +
/// footer). Returns a complete HTML document as a `String`.
///
/// Handlers call this last:
///
/// ```ignore
/// let body = html! { table { /* ... */ } };
/// let doc = templates::page(
///     "Inbox - personal",
///     PageContext::Listing,
///     Some("personal"), Some("inbox"),
///     body,
/// );
/// Html(doc).into_response()
/// ```
///
/// `active_account` and `active_mailbox` drive sidebar highlighting; pass
/// `None` for context-less pages (compose, search).
pub fn page(
    _title: &str,
    _ctx: PageContext,
    _active_account: Option<&str>,
    _active_mailbox: Option<&str>,
    _body: Markup,
) -> String {
    todo!(
        "build the full HTML doc using maud's html! macro;\n\
         include <link rel=stylesheet href=/static/css/mailforge.css>;\n\
         include <script defer src=/static/js/keys.js>;\n\
         set <body data-context=ctx.as_str()>;\n\
         emit sidebar() at the start of <body>, then body fragment, then helpbar()."
    )
}

/// Sidebar with account → mailbox tree. Active mailbox highlighted.
///
/// Iterate [`crate::mail::accounts::ACCOUNTS`]; for each account emit a
/// header row and the standard mailbox links. Mailbox vocabulary is
/// account-specific (see `notmuch_db::mailbox_query`'s doc comment for
/// the per-account list).
pub fn sidebar(
    _accounts: &[Account],
    _active_account: Option<&str>,
    _active_mailbox: Option<&str>,
) -> Markup {
    todo!("emit <aside class=sidebar> ... </aside>")
}

/// One row in a mailbox listing table. Renders as a `<tr>` with classes
/// reflecting the read/unread state and tag chips.
///
/// Read-state: a row is "unread" iff `env.tags` contains `"unread"`.
/// Apply `class=unread` when so.
///
/// Subject column links to `/mail/m/<id>` or `/mail/t/<thread>` depending
/// on `env.matched`/`env.total` (single-message thread → message URL;
/// multi-message → thread URL).
pub fn envelope_row(_env: &Envelope) -> Markup {
    todo!(
        "<tr class=if-unread> \
           <td>tags</td> <td>from</td> <td>subject (link)</td> <td>date</td> \
         </tr>"
    )
}

/// Pagination controls. `current` is 0-indexed, `total` is total message
/// count (not pages). `per_page` and `base_url` (e.g.
/// `/mail/personal/inbox`) drive the link shape.
pub fn paginator(
    _current_page: usize,
    _total: u64,
    _per_page: usize,
    _base_url: &str,
) -> Markup {
    todo!("<nav class=paginator> prev | 1 2 3 ... | next </nav>")
}

/// Single tag chip. Used inline in envelope rows and headers.
/// Some tags (system/internal) get hidden — same convention as meli's
/// `tags.ignore_tags`. The implementation agent should pull the ignore
/// list from `accounts.rs` if it grows.
pub fn tag_chip(_tag: &str) -> Markup {
    todo!("<span class=tag-chip>tag</span>")
}

/// Helpbar (footer) showing the most relevant key bindings for the
/// current context. Server-rendered (no JS state) and matches
/// `docs/keybindings.md`.
pub fn helpbar(_ctx: PageContext) -> Markup {
    todo!(
        "<footer class=helpbar> i/e nav | Enter open | r reply | / search | ? help </footer>\n\
         (varying per context per docs/keybindings.md)"
    )
}

#[allow(dead_code)]
fn _force_maud_in_scope() -> Markup {
    // Marker so `DOCTYPE` and `html!` aren't flagged unused before bodies
    // land. Remove when the first impl lands.
    html! { (DOCTYPE) html { } }
}
