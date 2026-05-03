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

use maud::{html, Markup, PreEscaped, DOCTYPE};

use crate::mail::accounts::{Account, ACCOUNTS};
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

/// Per-account mailbox vocabulary. Mirrors the queries in
/// `~/.config/meli/config.toml` (and the `notmuch_db::mailbox_query` doc
/// comment). Used by the sidebar to render the mailbox tree.
fn mailboxes_for(account_slug: &str) -> &'static [&'static str] {
    match account_slug {
        "personal" => &[
            "inbox",
            "unread",
            "sent",
            "archive",
            "promotions",
            "all-mail",
        ],
        "cohs" => &[
            "inbox",
            "unread",
            "sent",
            "drafts",
            "archive",
            "trash",
            "spam",
        ],
        _ => &[],
    }
}

/// Display label for a mailbox slug. Inbox stays "Inbox", "all-mail"
/// becomes "All Mail", etc.
pub fn mailbox_label(slug: &str) -> String {
    match slug {
        "all-mail" => "All Mail".to_string(),
        other => {
            // Capitalise first letter; leave rest lowercase.
            let mut chars = other.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        }
    }
}

/// Tags hidden in the UI (system / internal). Mirrors meli's
/// `tags.ignore_tags` convention. Keep this list short — every tag in
/// it is one less signal in the listing row.
fn is_ignored_tag(tag: &str) -> bool {
    if tag.starts_with("curator-") {
        return true;
    }
    matches!(
        tag,
        "inbox" | "unread" | "attachment" | "signed" | "encrypted" | "replied" | "passed" | "sent"
    )
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
    title: &str,
    ctx: PageContext,
    active_account: Option<&str>,
    active_mailbox: Option<&str>,
    body: Markup,
) -> String {
    let doc = html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                link rel="stylesheet" href="/static/css/theme.css";
                link rel="stylesheet" href="/static/css/base.css";
                link rel="stylesheet" href="/static/css/mailforge.css";
                script defer src="/static/js/keys.js" {}
            }
            body data-context=(ctx.as_str()) {
                div class="row fill app-shell" {
                    (sidebar(ACCOUNTS, active_account, active_mailbox))
                    main class="main" role="main" {
                        (body)
                    }
                }
                (helpbar(ctx))
                div id="toast-container" class="toast-container" {}
            }
        }
    };
    doc.into_string()
}

/// Sidebar with account → mailbox tree. Active mailbox highlighted.
///
/// Iterate [`crate::mail::accounts::ACCOUNTS`]; for each account emit a
/// header row and the standard mailbox links.
pub fn sidebar(
    accounts: &[Account],
    active_account: Option<&str>,
    active_mailbox: Option<&str>,
) -> Markup {
    html! {
        aside class="sidebar" aria-label="Mailbox navigation" {
            div class="sidebar-brand" {
                h1 { "MailForge" }
            }
            @for account in accounts {
                section class="sidebar-account" data-account=(account.slug) {
                    h2 class={
                        "sidebar-account__name"
                        @if active_account == Some(account.slug) { " active" }
                    } {
                        (account.display_name)
                    }
                    ul class="sidebar-mailboxes" {
                        @for mb in mailboxes_for(account.slug) {
                            @let is_active = active_account == Some(account.slug)
                                && active_mailbox == Some(*mb);
                            li {
                                a href=(format!("/mail/{}/{}", account.slug, mb))
                                    class=(if is_active { "active" } else { "" })
                                    data-account=(account.slug)
                                    data-mailbox=(*mb)
                                {
                                    (mailbox_label(mb))
                                }
                            }
                        }
                    }
                }
            }
            section class="sidebar-account sidebar-tools" {
                h2 class="sidebar-account__name" { "Tools" }
                ul class="sidebar-mailboxes" {
                    li { a href="/mail/compose" data-action="compose" { "Compose" } }
                    li { a href="/mail/search" data-action="search" { "Search" } }
                }
            }
        }
    }
}

/// One row in a mailbox listing table. Renders as a `<tr>` with classes
/// reflecting the read/unread state and tag chips.
///
/// Read-state: a row is "unread" iff `env.tags` contains `"unread"`.
/// Apply `class=unread` when so.
///
/// Subject column links to `/mail/m/<id>` or `/mail/t/<thread>` depending
/// on whether the thread is single-message (use message URL) or
/// multi-message (thread URL).
pub fn envelope_row(env: &Envelope) -> Markup {
    envelope_row_indexed(env, 0, None)
}

/// Same as [`envelope_row`] but stamps `data-row-index="<n>"` for keyboard
/// nav. The listing handler iterates with enumerate() and passes the
/// 0-based index in.
///
/// `from_ctx` is the `<account>/<mailbox>` slug pair the row is being
/// rendered inside. When set, the message link gets `?from=<ctx>` so
/// `show_message` can resolve prev/next siblings without a referer
/// header. Pass None from contexts that don't have a single anchoring
/// mailbox (search results across mailboxes, tests).
pub fn envelope_row_indexed(env: &Envelope, row_index: usize, from_ctx: Option<&str>) -> Markup {
    let unread = env.tags.iter().any(|t| t == "unread");
    let visible_tags: Vec<&String> = env
        .tags
        .iter()
        .filter(|t| !is_ignored_tag(t))
        .collect();

    // Extract mailcurator policy names from `curator-<name>-seen` tags.
    // Surfaces them on the row so the "Sweep like this" button can scope
    // to the matching policy. Comma-separated for the rare multi-policy
    // case; the JS picks the first one.
    let curator_policies: Vec<&str> = env
        .tags
        .iter()
        .filter_map(|t| t.strip_prefix("curator-").and_then(|s| s.strip_suffix("-seen")))
        .collect();
    let curator_policies_attr = if curator_policies.is_empty() {
        None
    } else {
        Some(curator_policies.join(","))
    };

    // Choose subject link target: message URL for single-message threads,
    // thread URL otherwise. Encode the id segment — GitHub notification
    // ids contain `/` (e.g. owner/repo/check-suites/...@github.com) which
    // would otherwise eat the route's `:id` matcher.
    let from_qs = from_ctx
        .map(|ctx| format!(
            "?from={}",
            url::form_urlencoded::byte_serialize(ctx.as_bytes()).collect::<String>()
        ))
        .unwrap_or_default();
    let (link, link_id_attr) = if let Some(msg_id) = env.message_id() {
        (
            format!("/mail/m/{}{}", crate::mail::notmuch_db::encode_id(msg_id), from_qs),
            Some(msg_id.to_string()),
        )
    } else {
        (format!("/mail/t/{}{}", env.thread, from_qs), None)
    };

    let subject = if env.subject.is_empty() {
        "(no subject)".to_string()
    } else {
        env.subject.clone()
    };

    let row_class = if unread { "envelope-row unread" } else { "envelope-row" };

    let has_unsub_attr = if env.has_unsubscribe { Some("true") } else { None };

    html! {
        tr class=(row_class)
            data-row-index=(row_index)
            data-thread-id=(env.thread)
            data-msg-id=[link_id_attr.as_deref()]
            data-curator-policies=[curator_policies_attr.as_deref()]
            data-has-unsubscribe=[has_unsub_attr]
        {
            td class="col-from" {
                span class="from-name" { (env.authors) }
            }
            td class="col-tags" {
                @for tag in &visible_tags {
                    (tag_chip(tag))
                }
            }
            td class="col-subject" {
                a href=(link)
                    data-row-index=(row_index)
                    data-msg-id=[link_id_attr.as_deref()]
                    data-thread-id=(env.thread)
                {
                    (subject)
                }
                @if env.total > 1 {
                    span class="thread-count" title=(format!("{} messages in thread", env.total)) {
                        " (" (env.total) ")"
                    }
                }
            }
            td class="col-date" { (env.date_relative) }
            // Per-row hover-reveal action icons. Now in their own
            // dedicated rightmost column (was inside col-from but
            // crowded the sender name on hover). Visibility is gated
            // by the row's data-* attrs in CSS so visually-parsimonious
            // rows (no policy match, no List-Unsubscribe) render an
            // empty cell. Buttons are type=button so they don't
            // accidentally trigger any wrapping form submit.
            td class="col-actions" {
                span class="row-actions" {
                    @if curator_policies_attr.is_some() {
                        button type="button"
                            class="row-action row-action--sweep"
                            data-action="sweep-row"
                            tabindex="-1"
                            aria-label="Sweep messages like this one"
                            title="Sweep all messages matched by this row's mailcurator policy (S)"
                        { "broom" }
                    }
                    @if env.has_unsubscribe {
                        button type="button"
                            class="row-action row-action--unsubscribe"
                            data-action="unsubscribe-row"
                            tabindex="-1"
                            aria-label="Unsubscribe from this sender"
                            title="Unsubscribe from this sender (U)"
                        { "unsub" }
                    }
                }
            }
        }
    }
}

/// Pagination controls. `current` is 0-indexed, `total` is total message
/// count (not pages). `per_page` and `base_url` (e.g.
/// `/mail/personal/inbox`) drive the link shape.
///
/// `extra_query` is an optional pre-formatted query-string fragment
/// (without leading `&`) to append to each link, e.g. `q=foo+bar`.
/// Pass `None` for plain mailbox pagination.
pub fn paginator(
    current_page: usize,
    total: u64,
    per_page: usize,
    base_url: &str,
) -> Markup {
    paginator_with_query(current_page, total, per_page, base_url, None)
}

/// Same as [`paginator`] but lets the caller add an extra query-string
/// fragment (e.g. `q=foo+bar`) so search and filter pagination preserve
/// the active filter.
pub fn paginator_with_query(
    current_page: usize,
    total: u64,
    per_page: usize,
    base_url: &str,
    extra_query: Option<&str>,
) -> Markup {
    let per_page = per_page.max(1);
    // Total number of pages. ceil_div without overflow worry: total fits u64.
    let total_pages = if total == 0 {
        1
    } else {
        ((total - 1) / per_page as u64) + 1
    } as usize;

    let has_prev = current_page > 0;
    let has_next = current_page + 1 < total_pages;

    let make_url = |page: usize| -> String {
        let q = extra_query.unwrap_or("");
        if q.is_empty() {
            format!("{base_url}?page={page}")
        } else {
            format!("{base_url}?page={page}&{q}")
        }
    };

    html! {
        nav class="paginator" aria-label="Pagination" {
            div class="paginator__counts" {
                @if total == 0 {
                    "No messages"
                } @else {
                    (format!("Page {} of {} ({} messages)",
                             current_page + 1, total_pages, total))
                }
            }
            div class="paginator__controls cluster" {
                @if has_prev {
                    a class="paginator__link"
                        href=(make_url(current_page - 1))
                        rel="prev"
                        data-action="prev-page"
                    { "← Prev" }
                } @else {
                    span class="paginator__link disabled" aria-disabled="true" { "← Prev" }
                }
                span class="paginator__current" {
                    (format!("{} / {}", current_page + 1, total_pages))
                }
                @if has_next {
                    a class="paginator__link"
                        href=(make_url(current_page + 1))
                        rel="next"
                        data-action="next-page"
                    { "Next →" }
                } @else {
                    span class="paginator__link disabled" aria-disabled="true" { "Next →" }
                }
            }
        }
    }
}

/// Single tag chip. Used inline in envelope rows and headers.
pub fn tag_chip(tag: &str) -> Markup {
    let variant = match tag {
        "sent" => "tag-chip tag-success",
        "trash" | "spam" => "tag-chip tag-error",
        "drafts" | "draft" => "tag-chip tag-warning",
        "billing" | "promotions" => "tag-chip tag-info",
        _ => "tag-chip",
    };
    html! {
        span class=(variant) data-tag=(tag) { (tag) }
    }
}

/// Helpbar (footer) showing the most relevant key bindings for the
/// current context. Server-rendered (no JS state) and matches
/// `docs/keybindings.md`.
pub fn helpbar(ctx: PageContext) -> Markup {
    let entries: &[(&str, &str)] = match ctx {
        PageContext::Listing => &[
            ("e/i", "nav"),
            ("Enter", "open"),
            ("Backspace", "back"),
            ("r", "reply"),
            ("d", "trash"),
            ("a/A", "archive/un"),
            ("S", "sweep"),
            ("U", "unsub"),
            ("c", "compose"),
            ("/", "search"),
            ("?", "help"),
        ],
        PageContext::Message => &[
            ("Backspace", "back"),
            ("r", "reply"),
            ("f", "fwd"),
            ("d", "trash"),
            ("a/A", "archive/un"),
            ("v/V", "HTML/untrust"),
            ("n/o", "prev/next"),
            ("?", "help"),
        ],
        PageContext::Thread => &[
            ("Tab", "next msg"),
            ("o", "open"),
            ("r", "reply"),
            ("Backspace", "back"),
            ("?", "help"),
        ],
        PageContext::Compose => &[
            ("Tab", "next field"),
            ("Ctrl+Enter", "send"),
            ("Ctrl+S", "save draft"),
            ("Ctrl+E", "Helix"),
            ("Esc", "cancel"),
        ],
        PageContext::Search => &[
            ("Enter", "submit"),
            ("e/i", "nav"),
            ("Esc", "blur"),
            ("?", "help"),
        ],
    };

    html! {
        footer class="helpbar" role="contentinfo" {
            div class="helpbar__bindings cluster" {
                @for (key, action) in entries {
                    span class="helpbar__binding" {
                        kbd { (*key) } " " span class="helpbar__action" { (*action) }
                    }
                }
            }
        }
    }
}

/// Subtle status banner shown near the top of the main column. Used by
/// listings to surface the current mailbox name + counts.
pub fn status_banner(title: &str, subtitle: Option<&str>) -> Markup {
    html! {
        header class="status-banner" {
            h1 class="status-banner__title" { (title) }
            @if let Some(sub) = subtitle {
                p class="status-banner__subtitle" { (sub) }
            }
        }
    }
}

/// Helper used by the message view to escape email headers safely. maud
/// already escapes content emitted via `(value)`, but a few call sites
/// build strings outside the macro and pass them in pre-escaped — this
/// makes the intent explicit.
pub fn safe_text(s: &str) -> Markup {
    html! { (s) }
}

/// Render an iframe-embedded HTML body, pointing directly at the strict-CSP
/// body endpoint `/v/<uuid>/body.html`.
///
/// History: an earlier design wrapped the body in a second HTML page (the
/// `/v/<uuid>` "wrapper") which itself contained an iframe pointing at
/// `body.html` — a nested-iframe shape. That intermittently failed on first
/// navigation (inner iframe loaded blank, refresh fixed it) due to a
/// browser-side timing race we never pinned down. Collapsing to a single
/// iframe eliminates the failure mode structurally; the image-toggle UI
/// (previously rendered inside the wrapper) now lives in the message-view
/// chrome via [`image_toggle_banner`]. See 2026-05-02 commit notes.
///
/// `images_allowed` controls the iframe's query string: default (true) sends
/// `?images=1`, which the daemon's `serve_body` handler reads to pick the
/// relaxed CSP (external images allowed). `?images=0` falls back to strict
/// CSP. The query string is always emitted explicitly so caches key on it.
///
/// `loading="eager"` is set explicitly. The default is "auto" which browsers
/// implement variably; eager removes one source of "loaded blank" weirdness.
/// `tabindex="-1"` keeps focus on the parent doc for keyboard nav; sandbox
/// flags allow target="_blank" link escape only.
pub fn html_body_iframe(uuid: &str, images_allowed: bool) -> Markup {
    let qs = if images_allowed { "?images=1" } else { "?images=0" };
    let src = format!("/v/{}/body.html{}", uuid, qs);
    html! {
        iframe class="message-body__iframe"
            sandbox="allow-popups allow-popups-to-escape-sandbox"
            tabindex="-1"
            loading="eager"
            src=(src)
            title="Message body"
        {}
    }
}

/// Render the image-toggle banner that sits in the message-view chrome,
/// directly above the body iframe. Replaces the old wrapper-page `img-banner`.
///
/// Submits via GET to the same message-view URL so the toggle is bookmarkable
/// and survives Backspace navigation. `current_path` is `/mail/m/<id>` (no
/// query) and the form preserves `view=full` so the user stays in the HTML
/// view after toggling. PDFs don't load external images so this is HTML-only.
pub fn image_toggle_banner(current_path: &str, images_allowed: bool) -> Markup {
    if images_allowed {
        html! {
            div class="img-banner" {
                "External images loaded."
                form method="get" action=(current_path) class="img-banner__form" {
                    input type="hidden" name="view" value="full";
                    input type="hidden" name="images" value="0";
                    button type="submit" class="img-toggle" { "Block external images" }
                }
            }
        }
    } else {
        html! {
            div class="img-banner img-banner--active" {
                "External images blocked for this view."
                form method="get" action=(current_path) class="img-banner__form" {
                    input type="hidden" name="view" value="full";
                    button type="submit" class="img-toggle" { "Load external images" }
                }
            }
        }
    }
}

/// Render plain-text body inside a `<pre>` block. maud escapes the
/// content automatically.
pub fn plaintext_body(text: &str) -> Markup {
    html! {
        pre class="plaintext message-body__plaintext" { (text) }
    }
}

/// Pre-escaped raw HTML pass-through. Used by handlers that have already
/// escaped their content (or are intentionally emitting safe HTML).
/// The vast majority of templates should use `(value)` directly, which
/// auto-escapes — only reach for this when you know what you're doing.
#[allow(dead_code)]
pub(crate) fn raw_html(s: &str) -> Markup {
    PreEscaped(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::notmuch_db::Envelope;

    fn make_env(subject: &str, tags: Vec<&str>) -> Envelope {
        Envelope {
            thread: "0000000000000001".to_string(),
            timestamp: 1000,
            date_relative: "1 min. ago".to_string(),
            matched: 1,
            total: 1,
            authors: "Alice <alice@example.com>".to_string(),
            subject: subject.to_string(),
            query: [Some("id:abc@example.com".to_string()), None],
            tags: tags.into_iter().map(String::from).collect(),
            has_unsubscribe: false,
        }
    }

    #[test]
    fn page_emits_doctype_and_context() {
        let body = html! { p { "hello" } };
        let html = page(
            "Test",
            PageContext::Listing,
            Some("personal"),
            Some("inbox"),
            body,
        );
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains(r#"data-context="listing""#));
        assert!(html.contains("<title>Test</title>"));
        assert!(html.contains("/static/css/theme.css"));
        assert!(html.contains("/static/css/base.css"));
        assert!(html.contains("/static/js/keys.js"));
        assert!(html.contains("hello"));
    }

    #[test]
    fn page_escapes_title() {
        let body = html! { p { "x" } };
        let html = page(
            "<script>alert(1)</script>",
            PageContext::Listing,
            None,
            None,
            body,
        );
        // maud escapes title content.
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    }

    #[test]
    fn sidebar_highlights_active_mailbox() {
        let html = sidebar(ACCOUNTS, Some("personal"), Some("inbox")).into_string();
        assert!(html.contains("personal"));
        assert!(html.contains("cohs"));
        // Active mailbox link gets class="active".
        assert!(html.contains(r#"class="active""#));
        assert!(html.contains(r#"href="/mail/personal/inbox""#));
        assert!(html.contains(r#"href="/mail/cohs/inbox""#));
    }

    #[test]
    fn envelope_row_marks_unread() {
        let env = make_env("Hello", vec!["inbox", "unread"]);
        let html = envelope_row(&env).into_string();
        assert!(html.contains("class=\"envelope-row unread\""));
        assert!(html.contains(r#"data-msg-id="abc@example.com""#));
        assert!(html.contains(r#"href="/mail/m/abc%40example.com""#));
    }

    #[test]
    fn envelope_row_read_no_unread_class() {
        let env = make_env("Hello", vec!["inbox"]);
        let html = envelope_row(&env).into_string();
        assert!(html.contains("class=\"envelope-row\""));
        assert!(!html.contains("envelope-row unread"));
    }

    #[test]
    fn envelope_row_escapes_subject() {
        let env = make_env("<script>x</script>", vec!["inbox"]);
        let html = envelope_row(&env).into_string();
        assert!(!html.contains("<script>x</script>"));
        assert!(html.contains("&lt;script&gt;x&lt;/script&gt;"));
    }

    #[test]
    fn envelope_row_no_subject_falls_back() {
        let env = make_env("", vec!["inbox"]);
        let html = envelope_row(&env).into_string();
        assert!(html.contains("(no subject)"));
    }

    #[test]
    fn envelope_row_thread_url_for_multi_message() {
        let mut env = make_env("Discussion", vec!["inbox"]);
        env.matched = 3;
        env.total = 5;
        let html = envelope_row(&env).into_string();
        // Multi-message thread → /mail/t/<thread>
        assert!(html.contains(r#"href="/mail/t/0000000000000001""#));
        // No data-msg-id on multi-message rows (no single bare id).
        assert!(html.contains("(5)"));
    }

    #[test]
    fn envelope_row_hides_ignored_tags() {
        let env = make_env("Hi", vec!["inbox", "unread", "billing", "attachment"]);
        let html = envelope_row(&env).into_string();
        // Only "billing" should appear as a chip; inbox/unread/attachment hidden.
        let billing_count = html.matches("data-tag=\"billing\"").count();
        let inbox_count = html.matches("data-tag=\"inbox\"").count();
        assert_eq!(billing_count, 1);
        assert_eq!(inbox_count, 0);
    }

    #[test]
    fn envelope_row_no_unsubscribe_attr_when_field_false() {
        // has_unsubscribe defaults to false in make_env — the attr must
        // not appear at all (not even as `data-has-unsubscribe="false"`).
        // Visual parsimony: rows without the affordance should have no
        // attr to gate hover CSS off.
        let env = make_env("Hello", vec!["inbox"]);
        let html = envelope_row(&env).into_string();
        assert!(
            !html.contains("data-has-unsubscribe"),
            "row without List-Unsubscribe must not stamp data-has-unsubscribe; got: {html}"
        );
        // And no row-action--unsubscribe button rendered.
        assert!(
            !html.contains("row-action--unsubscribe"),
            "unsubscribe icon must not render when has_unsubscribe=false"
        );
    }

    #[test]
    fn envelope_row_stamps_unsubscribe_attr_when_field_true() {
        let mut env = make_env("Hello", vec!["inbox"]);
        env.has_unsubscribe = true;
        let html = envelope_row(&env).into_string();
        assert!(
            html.contains(r#"data-has-unsubscribe="true""#),
            "row with List-Unsubscribe must stamp data-has-unsubscribe=\"true\"; got: {html}"
        );
        assert!(
            html.contains("row-action--unsubscribe"),
            "unsubscribe icon must render when has_unsubscribe=true"
        );
    }

    #[test]
    fn envelope_row_curator_policies_preserved_after_sweep_refactor() {
        // Sweep moved from toolbar to per-row icon, but the existing
        // data-curator-policies attribute on the <tr> must not regress —
        // sweepNow() reads it.
        let env = make_env("Hi", vec!["inbox", "curator-newsletters-seen"]);
        let html = envelope_row(&env).into_string();
        assert!(
            html.contains(r#"data-curator-policies="newsletters""#),
            "data-curator-policies must still be set after refactor; got: {html}"
        );
        assert!(
            html.contains("row-action--sweep"),
            "sweep icon must render when curator-* tag present"
        );
    }

    #[test]
    fn envelope_row_no_sweep_icon_without_curator_policy() {
        let env = make_env("Hi", vec!["inbox"]);
        let html = envelope_row(&env).into_string();
        assert!(
            !html.contains("row-action--sweep"),
            "sweep icon must not render when no curator-* tag matches"
        );
    }

    #[test]
    fn helpbar_listing_includes_unsubscribe_and_sweep() {
        let html = helpbar(PageContext::Listing).into_string();
        assert!(html.contains(">U<"), "helpbar must list U binding");
        assert!(html.contains(">S<"), "helpbar must list S binding");
        assert!(html.contains("unsub"));
        assert!(html.contains("sweep"));
    }

    #[test]
    fn paginator_math_first_page() {
        let html = paginator(0, 100, 50, "/mail/personal/inbox").into_string();
        assert!(html.contains("Page 1 of 2"));
        // Prev disabled on first page.
        assert!(html.contains(r#"<span class="paginator__link disabled""#));
        // Next link present.
        assert!(html.contains(r#"href="/mail/personal/inbox?page=1""#));
    }

    #[test]
    fn paginator_math_last_page() {
        let html = paginator(1, 100, 50, "/mail/personal/inbox").into_string();
        assert!(html.contains("Page 2 of 2"));
        // Prev link present.
        assert!(html.contains(r#"href="/mail/personal/inbox?page=0""#));
        // Next disabled on last page.
        // Two disabled spans? No — only Next should be disabled here.
        let disabled_count = html.matches(r#"class="paginator__link disabled""#).count();
        assert_eq!(disabled_count, 1);
    }

    #[test]
    fn paginator_zero_messages() {
        let html = paginator(0, 0, 50, "/mail/personal/inbox").into_string();
        assert!(html.contains("No messages"));
        assert!(html.contains("Page 1 of 1") || html.contains("1 / 1"));
    }

    #[test]
    fn paginator_partial_last_page() {
        // 3 pages: 50 + 50 + 1 = 101 messages.
        let html = paginator(2, 101, 50, "/mail/personal/inbox").into_string();
        assert!(html.contains("Page 3 of 3"));
        assert!(html.contains("(101 messages)"));
    }

    #[test]
    fn paginator_with_query_preserves_filter() {
        let html =
            paginator_with_query(0, 100, 50, "/mail/search", Some("q=stripe")).into_string();
        // maud escapes `&` → `&amp;` inside attribute values, so the
        // rendered href reads `/mail/search?page=1&amp;q=stripe`. Both
        // forms are acceptable browser-side.
        assert!(
            html.contains("/mail/search?page=1&amp;q=stripe")
                || html.contains("/mail/search?page=1&q=stripe"),
            "paginator URL not in output: {html}"
        );
    }

    #[test]
    fn helpbar_renders_for_each_context() {
        for ctx in [
            PageContext::Listing,
            PageContext::Message,
            PageContext::Thread,
            PageContext::Compose,
            PageContext::Search,
        ] {
            let html = helpbar(ctx).into_string();
            assert!(html.contains("<footer"));
            assert!(html.contains("helpbar"));
            assert!(html.contains("<kbd>"));
        }
    }

    #[test]
    fn tag_chip_variants() {
        assert!(tag_chip("sent")
            .into_string()
            .contains("tag-chip tag-success"));
        assert!(tag_chip("trash")
            .into_string()
            .contains("tag-chip tag-error"));
        assert!(tag_chip("drafts")
            .into_string()
            .contains("tag-chip tag-warning"));
        assert!(tag_chip("custom")
            .into_string()
            .contains(r#"class="tag-chip""#));
    }

    #[test]
    fn plaintext_body_escapes() {
        let html = plaintext_body("<b>hi</b>").into_string();
        assert!(!html.contains("<b>hi</b>"));
        assert!(html.contains("&lt;b&gt;hi&lt;/b&gt;"));
    }

    #[test]
    fn mailbox_label_handles_special_cases() {
        assert_eq!(mailbox_label("inbox"), "Inbox");
        assert_eq!(mailbox_label("all-mail"), "All Mail");
        assert_eq!(mailbox_label("sent"), "Sent");
    }
}
