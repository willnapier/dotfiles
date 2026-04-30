//! Single-message and thread read views.
//!
//! GET `/mail/m/<id>` → render one message.
//! GET `/mail/t/<thread-id>` → render all messages in the thread.
//!
//! ## HTML body handling
//!
//! For messages with `text/html` parts, mailforge reuses mailforge's
//! existing `/v/<uuid>` viewer pipeline. The flow:
//!
//! 1. `notmuch_db::show(id)` returns the message with file path.
//! 2. Read the raw RFC822 bytes from the file.
//! 3. Call into `crate::pipe::run_with_bytes(bytes)` (a refactor of the
//!    existing `pipe::run` that decouples the stdin-read; see TODO in
//!    pipe.rs). It builds a cache entry and returns the UUID.
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
//! "set seen" behaviour). Implementation:
//!
//! ```ignore
//! if message.tags.contains("unread") {
//!     notmuch_db::apply_tag_changes(
//!         &format!("id:{id}"),
//!         &[],
//!         &["unread"],
//!     )?;
//! }
//! ```
//!
//! Don't fail the render if tagging fails; just log and continue.

#[allow(unused_imports)]
use axum::{
    extract::Path,
    response::{IntoResponse, Response},
};

/// GET `/mail/m/<id>`.
///
/// `id` is the bare notmuch message id (with `@`, no `id:` prefix).
/// axum's `Path<String>` accepts `@` and other URL-safe chars without
/// extra encoding.
pub async fn show_message(Path(_id): Path<String>) -> Response {
    todo!(
        "1. notmuch_db::show(&id) → Message\n\
         2. branch on text_html vs text_plain:\n\
            - HTML: build /v/<uuid> cache via pipe::run_with_bytes (refactor)\n\
              and embed iframe\n\
            - plain: emit body inside <pre class=plaintext>\n\
         3. on render success, strip tag:unread (best-effort, log on fail)\n\
         4. render via templates::page(PageContext::Message, ...)"
    )
}

/// GET `/mail/t/<thread-id>`.
///
/// Multiple messages in chronological order. Same body-rendering
/// branching as `show_message` per-message.
///
/// Implementation hint: each message body sits inside a `<details>`
/// element so the user can collapse older messages in long threads;
/// mark the most recent message `<details open>`.
pub async fn show_thread(Path(_thread_id): Path<String>) -> Response {
    todo!(
        "1. notmuch_db::show_thread(&thread_id) → Vec<Message>\n\
         2. for each message: same body-render branching as show_message\n\
         3. wrap each in <details>, most recent <details open>\n\
         4. render via templates::page(PageContext::Thread, ...)"
    )
}
