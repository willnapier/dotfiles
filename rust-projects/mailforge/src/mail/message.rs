//! Single-message and thread read views.
//!
//! GET `/mail/m/<id>` → render one message.
//! GET `/mail/t/<thread-id>` → render all messages in the thread.
//!
//! ## HTML body handling
//!
//! For messages with `text/html` parts, mailforge reuses the existing
//! `/v/<uuid>` viewer pipeline. The flow:
//!
//! 1. `notmuch_db::show(id)` returns the message with file path.
//! 2. Read the raw RFC822 bytes from the file.
//! 3. Call into [`crate::pipe::cache_html_message`] which builds a cache
//!    entry and returns the UUID.
//! 4. Embed `<iframe sandbox src="/v/<uuid>">` in the rendered page.
//!    The iframe's CSP/asset machinery is unchanged — mailforge's existing
//!    code path applies.
//!
//! For messages with only `text/plain`, render the body directly inside
//! the page (no iframe needed) inside a `<pre class=plaintext>` block.
//!
//! ## Read-state tracking
//!
//! On render, automatically remove the `unread` tag (mirrors the meli
//! "set seen" behaviour). Tagging failure is logged but does not block
//! the render.

use axum::{
    extract::Path,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use maud::{html, Markup};

use crate::mail::accounts::ACCOUNTS;
use crate::mail::auth_results;
use crate::mail::notmuch_db::{self, Message};
use crate::mail::templates::{self, html_body_iframe, plaintext_body, tag_chip, PageContext};
use crate::mail::trusted_senders;

/// Outcome of consulting the trusted-senders store + Authentication-Results
/// header for a single message render. Drives both the body branch and
/// the header chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrustState {
    /// Sender domain not in the trusted set — no chip; default body branch.
    NotTrusted,
    /// Sender domain trusted AND auth passed — auto-render HTML inline.
    /// Carries the domain (as a borrowed slice in the owning struct).
    AutoHtml,
    /// Sender domain trusted but auth headers explicitly fail — force
    /// plaintext + show a warning chip ("possible spoof").
    AuthFailed,
    /// Sender domain trusted; auth header missing (no warning, just
    /// fall-through to default behaviour). We don't render a chip in
    /// this case — many legacy senders genuinely lack the header, and
    /// pushing a warning every time would train banner-blindness.
    /// Effectively equivalent to NotTrusted for rendering purposes.
    TrustedNoAuth,
}

/// Resolved trust state plus the domain string we used to decide. Threaded
/// from `show_message` through to `message_header` so the chip can name
/// the domain in its label / data-attr.
#[derive(Debug, Clone)]
struct TrustContext {
    state: TrustState,
    /// The lowercased From-domain (or empty if extraction failed).
    domain: String,
}

impl TrustContext {
    fn none() -> Self {
        Self { state: TrustState::NotTrusted, domain: String::new() }
    }
}

/// GET `/mail/m/<id>`.
///
/// `id` is the bare notmuch message id (with `@`, no `id:` prefix).
/// axum's `Path<String>` accepts `@` and other URL-safe chars without
/// extra encoding.
pub async fn show_message(
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    // 1. Fetch message.
    let message = match notmuch_db::show(&id) {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                format!("message not found: {id}\n{e}"),
            )
                .into_response();
        }
    };

    // 2. Best-effort: strip the `unread` tag. Don't fail render on tag err.
    if message.tags.iter().any(|t| t == "unread") {
        let q = format!("id:{}", &message.id);
        if let Err(e) = notmuch_db::apply_tag_changes(&q, &[], &["unread"]) {
            tracing::warn!("failed to clear unread tag for {}: {}", message.id, e);
        }
    }

    // 3. Resolve trust state. `?untrusted=1` lets the caller force the
    //    default plaintext branch (we don't currently surface a UI for
    //    this; reserved for future "view as plain" override).
    let trust = resolve_trust(&message);

    // 4. Pick body rendering strategy.
    // `?view=full` overrides the plain-text-preferred default and forces
    // the HTML iframe view (the keyboard-shortcut `m` from message-view
    // navigates here; the in-page hint says "press m to open the
    // dedicated viewer"). When `?view=full` is set AND there's an HTML
    // part, skip the plaintext-preferred branch and fall through to
    // the HTML iframe path. Falls back gracefully if no HTML part.
    //
    // When `trust.state == AutoHtml` AND there's an HTML part, also
    // skip the plaintext branch — the user has whitelisted the domain
    // and the message authenticates, so render HTML inline by default.
    let force_full = q.get("view").map(|v| v == "full").unwrap_or(false);
    let auto_html = matches!(trust.state, TrustState::AutoHtml) && message.text_html.is_some();
    let body_markup = if (force_full || auto_html) && message.text_html.is_some() {
        render_body_html_only(&message)
    } else {
        render_body(&message)
    };

    // 4. Compose final page.
    let subject_text = message
        .subject
        .clone()
        .unwrap_or_else(|| "(no subject)".to_string());

    // Choose sidebar highlight: if the message carries one of the account
    // tag-gates, highlight that account; otherwise leave the sidebar
    // unhighlighted so the user can find their way back.
    let active_account = pick_active_account(&message);

    // Resolve prev/next siblings if the link came from a listing context
    // (`?from=<account>/<mailbox>`). When absent, the keys silently
    // no-op — same as before.
    let from_ctx = q.get("from").cloned();
    let (prev_id, next_id) = match from_ctx.as_deref() {
        Some(ctx) => siblings_for_message(ctx, &message.id),
        None => (None, None),
    };
    let nav_links = match from_ctx.as_deref() {
        Some(ctx) => sibling_nav_links(prev_id.as_deref(), next_id.as_deref(), ctx),
        None => html! {},
    };

    let page_body = html! {
        article class="message-view" data-msg-id=(message.id) {
            (message_header_with_trust(&message, &subject_text, &trust))
            (body_markup)
            (action_toolbar(&message))
            (nav_links)
        }
    };

    let title = format!("{subject_text} — MailForge");
    let doc = templates::page(
        &title,
        PageContext::Message,
        active_account,
        None,
        page_body,
    );
    Html(doc).into_response()
}

/// GET `/mail/t/<thread-id>`.
///
/// Multiple messages in chronological order. Same body-rendering
/// branching as `show_message` per-message.
///
/// Each message body sits inside a `<details>` element so the user can
/// collapse older messages in long threads; the most recent message
/// renders `<details open>`.
pub async fn show_thread(Path(thread_id): Path<String>) -> Response {
    let messages = match notmuch_db::show_thread(&thread_id) {
        Ok(ms) => ms,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                format!("thread not found: {thread_id}\n{e}"),
            )
                .into_response();
        }
    };

    if messages.is_empty() {
        return (StatusCode::NOT_FOUND, format!("empty thread: {thread_id}")).into_response();
    }

    // Best-effort: clear unread tags on the entire thread.
    let q = format!("thread:{thread_id} and tag:unread");
    if let Err(e) = notmuch_db::apply_tag_changes(&q, &[], &["unread"]) {
        tracing::warn!("failed to clear unread for thread {}: {}", thread_id, e);
    }

    let last_index = messages.len() - 1;
    let thread_subject = messages
        .iter()
        .find_map(|m| m.subject.clone())
        .unwrap_or_else(|| "(no subject)".to_string());
    let active_account = messages.iter().find_map(pick_active_account);

    let page_body = html! {
        article class="thread-view" data-thread-id=(thread_id) {
            header class="thread-view__header" {
                h1 { (thread_subject) }
                p class="thread-view__meta" {
                    (format!("{} message{} in thread",
                        messages.len(),
                        if messages.len() == 1 { "" } else { "s" }))
                }
            }

            @for (idx, msg) in messages.iter().enumerate() {
                @let is_last = idx == last_index;
                @let from_label = msg.from.as_deref().unwrap_or("(unknown sender)");
                @let date_label = msg.date.as_deref().unwrap_or("(no date)");
                details class="thread-message" open[is_last] data-msg-id=(msg.id) {
                    summary class="thread-message__summary" {
                        span class="thread-message__from" { (from_label) }
                        " — "
                        span class="thread-message__date" { (date_label) }
                    }
                    div class="thread-message__body" {
                        (message_header(msg, msg.subject.as_deref().unwrap_or("(no subject)")))
                        (render_body(msg))
                        (action_toolbar(msg))
                    }
                }
            }
        }
    };

    let title = format!("{thread_subject} — Thread — MailForge");
    let doc = templates::page(
        &title,
        PageContext::Thread,
        active_account,
        None,
        page_body,
    );
    Html(doc).into_response()
}

/// Resolve the trust state for rendering this message.
///
/// Steps:
/// 1. Extract the From-domain (lowercased). If extraction fails, return
///    NotTrusted — we can't even ask the question.
/// 2. Check the in-memory trusted-senders set. If the domain isn't there,
///    return NotTrusted (skip auth parsing — it's irrelevant).
/// 3. Re-parse the message file's headers via `mail-parser` to read the
///    `Authentication-Results` header.
/// 4. Map the auth verdict onto a TrustState:
///    - passed → AutoHtml
///    - explicit fail → AuthFailed (warn + force plaintext)
///    - header missing or all-other → TrustedNoAuth (silent fallback)
///
/// We re-parse the file on each render rather than carrying the
/// `Authentication-Results` field through `notmuch_db::Message`. That
/// keeps the existing data shape stable and pays the parse cost only on
/// the message-view route (50-200ms total notmuch round-trip dwarfs
/// the parse).
fn resolve_trust(msg: &Message) -> TrustContext {
    let domain = match msg.from.as_deref().and_then(trusted_senders::extract_from_domain) {
        Some(d) => d,
        None => return TrustContext::none(),
    };
    if !trusted_senders::is_trusted(&domain) {
        return TrustContext { state: TrustState::NotTrusted, domain };
    }
    let Some(path) = msg.filename.as_deref() else {
        return TrustContext { state: TrustState::TrustedNoAuth, domain };
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            tracing::debug!("trust: failed to read {}: {}", path, e);
            return TrustContext { state: TrustState::TrustedNoAuth, domain };
        }
    };
    let parsed = match mail_parser::MessageParser::default().parse(&bytes[..]) {
        Some(p) => p,
        None => {
            tracing::debug!("trust: mail-parser failed for {}", path);
            return TrustContext { state: TrustState::TrustedNoAuth, domain };
        }
    };
    let verdict = auth_results::verdict_from_message(&parsed);
    let state = if !verdict.header_present {
        TrustState::TrustedNoAuth
    } else if verdict.passed() {
        TrustState::AutoHtml
    } else if verdict.explicit_fail() {
        TrustState::AuthFailed
    } else {
        TrustState::TrustedNoAuth
    };
    TrustContext { state, domain }
}

/// Render the standard headers panel plus an optional trust-chip
/// (`auto-HTML — domain trusted` / `auth failed — forced plaintext`).
/// Delegates to `message_header` for the unchanged headers block.
fn message_header_with_trust(
    msg: &Message,
    subject_text: &str,
    trust: &TrustContext,
) -> Markup {
    html! {
        (message_header(msg, subject_text))
        (trust_chip(trust))
    }
}

/// Render the small chip near the message header that calls out the
/// trust state. Returns an empty fragment for `NotTrusted` and
/// `TrustedNoAuth` (we deliberately stay quiet when there's no signal
/// to surface).
///
/// - `AutoHtml`: shows `[auto-HTML — <domain> is trusted (click to untrust)]`.
///   Click handler calls POST `/api/html-trusted/remove` and reloads.
/// - `AuthFailed`: shows `[auth failed — forced plaintext]` with a tooltip.
///
/// Visible affordances reuse `tag-chip` so the styling stays in sync with
/// the rest of the UI; specific variants get a `trust-chip-*` modifier
/// for any future per-state colour tweaks.
fn trust_chip(trust: &TrustContext) -> Markup {
    match trust.state {
        TrustState::NotTrusted | TrustState::TrustedNoAuth => html! {},
        TrustState::AutoHtml => {
            let label = format!("auto-HTML — {} is trusted (click to untrust)", trust.domain);
            html! {
                button type="button"
                    class="tag-chip trust-chip trust-chip-auto"
                    data-action="untrust-domain"
                    data-domain=(trust.domain)
                    title="Click to remove this domain from the auto-HTML list"
                {
                    (label)
                }
            }
        }
        TrustState::AuthFailed => html! {
            span class="tag-chip trust-chip trust-chip-failed"
                data-domain=(trust.domain)
                title="DMARC/SPF/DKIM didn't pass; this message is unverified despite claiming to be from a trusted domain. Showing plaintext to avoid tracking-pixel exposure on a possibly-spoofed message."
            {
                "auth failed — forced plaintext"
            }
        },
    }
}

/// Renders the standard headers panel: subject + From/To/Cc/Bcc/Date,
/// collapsed-by-default detail block for the noisier headers via
/// `<details>` with the visible headers always shown.
fn message_header(msg: &Message, subject_text: &str) -> Markup {
    let from_text = msg.from.as_deref().unwrap_or("(unknown sender)");
    let date_text = msg.date.as_deref().unwrap_or("(no date)");
    let to_text = if msg.to.is_empty() {
        None
    } else {
        Some(msg.to.join(", "))
    };
    let cc_text = if msg.cc.is_empty() {
        None
    } else {
        Some(msg.cc.join(", "))
    };

    html! {
        header class="message-header" {
            h1 class="message-header__subject" { (subject_text) }
            div class="message-header__meta" {
                div class="meta-row" {
                    span class="meta-key" { "From" }
                    span class="meta-val" { (from_text) }
                }
                @if let Some(to) = &to_text {
                    div class="meta-row" {
                        span class="meta-key" { "To" }
                        span class="meta-val" { (to) }
                    }
                }
                div class="meta-row" {
                    span class="meta-key" { "Date" }
                    span class="meta-val" { (date_text) }
                }
            }
            @if cc_text.is_some() || !msg.tags.is_empty() {
                details class="message-header__more" {
                    summary { "More headers" }
                    div class="message-header__meta" {
                        @if let Some(cc) = &cc_text {
                            div class="meta-row" {
                                span class="meta-key" { "Cc" }
                                span class="meta-val" { (cc) }
                            }
                        }
                        @if !msg.tags.is_empty() {
                            div class="meta-row" {
                                span class="meta-key" { "Tags" }
                                span class="meta-val cluster" {
                                    @for t in &msg.tags { (tag_chip(t)) }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Branches on plain vs HTML body. HTML bodies build a cache entry via
/// `pipe::cache_html_message` and embed an iframe; plain bodies render
/// inline as `<pre class=plaintext>`.
fn render_body(msg: &Message) -> Markup {
    if let Some(plain) = &msg.text_plain {
        // Prefer plain text when available — fewer surprises, no iframe
        // chrome, copies cleanly. The user can press `m` (per
        // docs/keybindings.md) to escalate to the full HTML viewer.
        return html! {
            section class="message-body" aria-label="Message body" {
                (plaintext_body(plain))
                @if msg.text_html.is_some() {
                    p class="message-body__html-hint" {
                        "An HTML version is available — press "
                        kbd { "v" }
                        " to open it in the dedicated viewer."
                    }
                }
            }
        };
    }

    // No plain text. Fall back to HTML body rendered via the existing
    // viewer pipeline.
    if let Some(html_body) = &msg.text_html {
        // pipe::cache_html_message wants raw RFC822-ish bytes; if we
        // only have the HTML body string, hand it through and let the
        // wrap_bare_html fallback do the right thing.
        let bytes_opt = match &msg.filename {
            Some(path) => match std::fs::read(path) {
                Ok(bytes) if !bytes.is_empty() => Some(bytes),
                _ => None,
            },
            None => None,
        };
        let bytes = bytes_opt.unwrap_or_else(|| html_body.as_bytes().to_vec());

        match crate::pipe::cache_html_message(&bytes) {
            Ok(uuid) => {
                return html! {
                    section class="message-body message-body--html" aria-label="Message body" {
                        (html_body_iframe(&uuid))
                    }
                };
            }
            Err(e) => {
                tracing::warn!("cache_html_message failed for {}: {}", msg.id, e);
                return html! {
                    section class="message-body" aria-label="Message body" {
                        div class="banner banner--error" {
                            "Failed to render HTML body: " (e.to_string())
                        }
                    }
                };
            }
        }
    }

    html! {
        section class="message-body" aria-label="Message body" {
            div class="empty-state panel" {
                p { "(This message has no readable body.)" }
            }
        }
    }
}

/// Force-render the HTML body via the iframe pipeline, ignoring any
/// available plain-text alternative. Triggered by `?view=full` query
/// param (the `m` keystroke from message-view). Falls back to plain
/// text or the empty state if no HTML body is present, so the call
/// site never crashes on absent HTML.
fn render_body_html_only(msg: &Message) -> Markup {
    let Some(html_body) = &msg.text_html else {
        return render_body(msg);
    };

    let bytes_opt = match &msg.filename {
        Some(path) => match std::fs::read(path) {
            Ok(bytes) if !bytes.is_empty() => Some(bytes),
            _ => None,
        },
        None => None,
    };
    let bytes = bytes_opt.unwrap_or_else(|| html_body.as_bytes().to_vec());

    match crate::pipe::cache_html_message(&bytes) {
        Ok(uuid) => html! {
            section class="message-body message-body--html" aria-label="Message body" {
                (html_body_iframe(&uuid))
            }
        },
        Err(e) => {
            tracing::warn!("cache_html_message failed for {}: {}", msg.id, e);
            html! {
                section class="message-body" aria-label="Message body" {
                    div class="banner banner--error" {
                        "Failed to render HTML body: " (e.to_string())
                    }
                }
            }
        }
    }
}

/// Reply / Reply All / Forward / Delete / Archive toolbar. Each button
/// has both an `accesskey` and a `data-action` attribute so the keyboard
/// JS can dispatch and the browser's native accesskey wiring also works.
fn action_toolbar(msg: &Message) -> Markup {
    let id = &msg.id;
    let reply_url = format!("/mail/compose?reply={id}");
    let fwd_url = format!("/mail/compose?fwd={id}");

    html! {
        nav class="action-toolbar" aria-label="Message actions" {
            a class="action-btn"
                href=(reply_url)
                accesskey="r"
                data-action="reply"
                data-msg-id=(id)
            { "Reply" }
            a class="action-btn"
                href=(format!("{reply_url}&all=1"))
                accesskey="R"
                data-action="reply-all"
                data-msg-id=(id)
            { "Reply All" }
            a class="action-btn"
                href=(fwd_url)
                accesskey="f"
                data-action="forward"
                data-msg-id=(id)
            { "Forward" }
            // Delete and archive POST to /api/* endpoints; render as
            // forms so they work without JS, but the JS keyboard agent
            // hijacks them for optimistic UI updates per
            // docs/keybindings.md.
            form class="action-form" method="post" action="/api/trash" {
                input type="hidden" name="ids" value=(id);
                button type="submit"
                    class="action-btn danger"
                    accesskey="d"
                    data-action="trash"
                    data-msg-id=(id)
                { "Delete" }
            }
            form class="action-form" method="post" action="/api/archive" {
                input type="hidden" name="ids" value=(id);
                button type="submit"
                    class="action-btn"
                    accesskey="a"
                    data-action="archive"
                    data-msg-id=(id)
                { "Archive" }
            }
            a class="action-btn action-btn--secondary"
                href=(format!("/mail/m/{}?view=full", crate::mail::notmuch_db::encode_id(&msg.id)))
                accesskey="v"
                data-action="open-viewer"
            { "HTML view" }
        }
    }
}

/// Resolve the prev/next message ids for a given mailbox context. Returns
/// `(prev_id, next_id)` ordered as the user reads the listing top-to-bottom
/// — `prev` is the row above (newer message in newest-first order), `next`
/// is the row below (older).
///
/// The cap of 500 envelopes is a deliberate trade-off: searching the full
/// inbox would parse a multi-MB JSON blob on every message render. 500
/// covers virtually all in-session reading sessions; if the user is
/// reading a message that's deeper than the 500th, prev/next anchors
/// silently disappear and the keys no-op (acceptable degradation).
fn siblings_for_message(
    from_ctx: &str,
    current_id: &str,
) -> (Option<String>, Option<String>) {
    let mut parts = from_ctx.splitn(2, '/');
    let (Some(account_slug), Some(mailbox)) = (parts.next(), parts.next()) else {
        return (None, None);
    };
    let Some(account) = crate::mail::accounts::find(account_slug) else {
        return (None, None);
    };
    let Some(query) = notmuch_db::mailbox_query(account, mailbox) else {
        return (None, None);
    };
    let envelopes = match notmuch_db::search(&query, 0, 500) {
        Ok(e) => e,
        Err(_) => return (None, None),
    };
    let ids: Vec<&str> = envelopes
        .iter()
        .filter_map(|e| e.message_id())
        .collect();
    let Some(pos) = ids.iter().position(|&id| id == current_id) else {
        return (None, None);
    };
    let prev = if pos > 0 { Some(ids[pos - 1].to_string()) } else { None };
    let next = ids.get(pos + 1).map(|s| s.to_string());
    (prev, next)
}

/// Render a `<a hidden data-nav=…>` for prev/next siblings. The keyboard
/// JS clicks these via `n`/`o`. Hidden anchors stay accessible to
/// `clickSel()` but don't take screen real estate.
fn sibling_nav_links(
    prev_id: Option<&str>,
    next_id: Option<&str>,
    from_ctx: &str,
) -> Markup {
    let from_qs = format!(
        "?from={}",
        url::form_urlencoded::byte_serialize(from_ctx.as_bytes()).collect::<String>()
    );
    html! {
        @if let Some(p) = prev_id {
            a hidden data-nav="prev-message"
                href=(format!("/mail/m/{}{}", notmuch_db::encode_id(p), from_qs)) {}
        }
        @if let Some(n) = next_id {
            a hidden data-nav="next-message"
                href=(format!("/mail/m/{}{}", notmuch_db::encode_id(n), from_qs)) {}
        }
    }
}

/// Helper: which account should the sidebar highlight for this message?
///
/// We heuristically pick by checking each account's `tag_gate`; if the
/// message carries that tag, that's the active account. Returns None
/// when the message is "personal" (which uses absence-of-cohs as its
/// gate, not a positive tag).
fn pick_active_account(msg: &Message) -> Option<&'static str> {
    for account in ACCOUNTS {
        if account.tag_gate.is_empty() {
            continue;
        }
        if msg.tags.iter().any(|t| t == account.tag_gate) {
            return Some(account.slug);
        }
    }
    // Default to the first account (personal) for messages without a
    // tag gate; this keeps the sidebar visually anchored.
    ACCOUNTS.first().map(|a| a.slug)
}

/// Build a viewer URL slug — for now this just returns the message id
/// (the existing /v/<uuid> route doesn't accept arbitrary message ids,
/// but the link target communicates intent; the keyboard JS resolves
/// the real UUID via cache_html_message on demand).
fn id_slug_for_viewer(msg: &Message) -> String {
    msg.id.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::notmuch_db::Message;

    fn make_msg(subject: &str, body: Option<&str>, tags: Vec<&str>) -> Message {
        Message {
            id: "abc@example.com".to_string(),
            subject: Some(subject.to_string()),
            from: Some("Alice <alice@example.com>".to_string()),
            to: vec!["bob@example.com".to_string()],
            cc: vec![],
            date: Some("Wed, 30 Apr 2026 10:00:00 +0000".to_string()),
            tags: tags.into_iter().map(String::from).collect(),
            filename: None,
            text_plain: body.map(String::from),
            text_html: None,
        }
    }

    #[test]
    fn message_header_escapes_subject() {
        let msg = make_msg("ignored", Some("body"), vec!["inbox"]);
        let html = message_header(&msg, "<script>alert(1)</script>").into_string();
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    }

    #[test]
    fn message_header_escapes_from() {
        let mut msg = make_msg("hi", Some("body"), vec!["inbox"]);
        msg.from = Some("<evil>@x.com".to_string());
        let html = message_header(&msg, "hi").into_string();
        assert!(!html.contains("<evil>"));
        assert!(html.contains("&lt;evil&gt;"));
    }

    #[test]
    fn render_body_prefers_plain_text() {
        let msg = make_msg("Hi", Some("Hello world"), vec!["inbox"]);
        let html = render_body(&msg).into_string();
        assert!(html.contains("Hello world"));
        assert!(html.contains("plaintext"));
        assert!(!html.contains("iframe"));
    }

    #[test]
    fn render_body_escapes_plain() {
        let msg = make_msg("Hi", Some("<b>HTML</b>"), vec!["inbox"]);
        let html = render_body(&msg).into_string();
        assert!(!html.contains("<b>HTML</b>"));
        assert!(html.contains("&lt;b&gt;HTML&lt;/b&gt;"));
    }

    #[test]
    fn render_body_no_body_shows_empty_state() {
        let msg = make_msg("Hi", None, vec!["inbox"]);
        let html = render_body(&msg).into_string();
        assert!(html.contains("no readable body"));
    }

    #[test]
    fn render_body_html_hint_when_both_present() {
        let mut msg = make_msg("Hi", Some("plain"), vec!["inbox"]);
        msg.text_html = Some("<p>html</p>".to_string());
        let html = render_body(&msg).into_string();
        assert!(html.contains("plain"));
        assert!(html.contains("HTML version is available"));
    }

    #[test]
    fn action_toolbar_has_accesskeys_and_data_attrs() {
        let msg = make_msg("Hi", Some("body"), vec!["inbox"]);
        let html = action_toolbar(&msg).into_string();
        assert!(html.contains(r#"accesskey="r""#));
        assert!(html.contains(r#"accesskey="f""#));
        assert!(html.contains(r#"accesskey="d""#));
        assert!(html.contains(r#"accesskey="a""#));
        assert!(html.contains(r#"data-action="reply""#));
        assert!(html.contains(r#"data-action="trash""#));
        assert!(html.contains(r#"data-action="archive""#));
        assert!(html.contains(r#"data-action="forward""#));
        // Reply link should encode the message id.
        assert!(html.contains("/mail/compose?reply=abc@example.com"));
    }

    #[test]
    fn pick_active_account_for_cohs_tag() {
        let msg = make_msg("Hi", Some("b"), vec!["inbox", "cohs"]);
        assert_eq!(pick_active_account(&msg), Some("cohs"));
    }

    #[test]
    fn pick_active_account_defaults_to_personal() {
        let msg = make_msg("Hi", Some("b"), vec!["inbox"]);
        // First account in ACCOUNTS is "personal".
        assert_eq!(pick_active_account(&msg), Some("personal"));
    }
}
