//! Composer: form rendering, send pipeline, draft autosave.
//!
//! GET `/mail/compose` → blank composer.
//! GET `/mail/compose?reply=<msg-id>` → reply prefilled (To, Subject, quoted body).
//! GET `/mail/compose?fwd=<msg-id>` → forward prefilled.
//! GET `/mail/compose?draft=<id>` → resume saved draft.
//! GET `/mail/draft/<id>` → same as ?draft=<id> (cleaner URL for bookmarks).
//! POST `/api/send` → build MIME, dispatch to msmtp / graph-send, redirect to sent.
//! POST `/api/draft` → save draft to `~/.cache/mailforge/drafts/<uuid>.json`.
//!
//! ## Send pipeline
//!
//! 1. Parse the form fields (`SendForm`).
//! 2. Look up the from-account via `accounts::find` (the form contains
//!    `from_account` as a hidden field equal to the slug; the explicit
//!    From: header is filled from `account.identity` to prevent
//!    spoofing).
//! 3. Build `lettre::Message` (text-only for now; multipart with
//!    attachments deferred — note ATTACHMENTS-DEFERRED below).
//! 4. Serialize to bytes via `Message::formatted()`.
//! 5. Pipe to the account's send backend:
//!    - `Msmtp { account }`: spawn `msmtp --account=<n> --read-recipients
//!      --read-envelope-from`, write MIME to stdin, wait, check exit.
//!    - `GraphSend`: spawn `graph-send`, write MIME to stdin, wait,
//!      check exit.
//! 6. On success: notmuch tag the new message id with `+sent -draft`
//!    (the message file lands in maildir `cur/` once mbsync sees it
//!    in the server-sent folder; for the immediate-feedback path, we
//!    tag the queued copy if available, else skip and rely on the next
//!    `notmuch new` to pick it up).
//! 7. Return 302 to `/mail/<account>/sent`.
//!
//! ## ATTACHMENTS-DEFERRED
//!
//! The first build supports text-only compose (subject + body). Adding
//! file attachments requires multipart/mixed assembly via lettre's
//! `MultiPart` builder (see `bequest/src/send.rs` for the canonical
//! pattern). Hold for phase 4.5 unless a follow-up agent scopes it
//! into the initial compose work.
//!
//! ## Helix escalation
//!
//! POST `/api/edit-buffer` (TODO endpoint, not yet routed): receives
//! current body text, writes to a tempfile, spawns
//! `wezterm start --always-new-process -- helix <tempfile>`, returns
//! `{ id, path }`. The client polls GET `/api/edit-buffer/<id>` every
//! 1s; once the file's mtime changes and the wezterm process is no
//! longer running (poll via `kill -0` on its pid), the daemon returns
//! `{ done: true, content: "..." }` and the client's textarea swaps in.
//!
//! Add this endpoint when implementing the compose flow; keeping it out
//! of `mod.rs::router()` for now since it's optional.

#[allow(unused_imports)]
use axum::{
    extract::{Path, Query},
    response::{IntoResponse, Response},
    Form, Json,
};
use serde::{Deserialize, Serialize};

/// Query string parameters for the GET compose form.
#[derive(Debug, Default, Deserialize)]
pub struct ComposeQuery {
    /// Reply: prefill To, Subject (with "Re: "), and quoted body.
    pub reply: Option<String>,
    /// Forward: prefill Subject (with "Fwd: ") and inline-quote the body.
    pub fwd: Option<String>,
    /// Resume a saved draft. Same effect as `/mail/draft/<id>`.
    pub draft: Option<String>,
    /// Pre-select an account (for "compose from cohs" links). Defaults
    /// to the personal account if absent.
    pub from: Option<String>,
}

/// Form fields posted to `/api/send`.
///
/// `from_account` is the account slug (`personal` / `cohs`) — drives
/// send-backend selection. The actual `From:` header is filled from
/// the account's `identity` field server-side.
#[derive(Debug, Deserialize)]
pub struct SendForm {
    pub from_account: String,
    pub to: String,
    pub cc: Option<String>,
    pub bcc: Option<String>,
    pub subject: String,
    pub body: String,
    /// In-Reply-To header, if this is a reply. Used to thread.
    pub in_reply_to: Option<String>,
}

/// Draft autosave envelope. Matches `SendForm` plus a draft id.
#[derive(Debug, Serialize, Deserialize)]
pub struct DraftBody {
    pub id: String,
    pub from_account: String,
    pub to: String,
    pub cc: Option<String>,
    pub bcc: Option<String>,
    pub subject: String,
    pub body: String,
    pub in_reply_to: Option<String>,
}

/// GET `/mail/compose` (and variants via query string).
pub async fn compose_form(Query(_q): Query<ComposeQuery>) -> Response {
    todo!(
        "1. branch on q.reply / q.fwd / q.draft to pick prefill source\n\
         2. for reply/fwd: fetch via notmuch_db::show, extract headers + body\n\
            - reply: To = original From, Subject = 'Re: ' + original (deduped),\n\
              quoted body = '> ' prefix per line\n\
            - fwd:   To = empty, Subject = 'Fwd: ' + original,\n\
              quoted body = full original headers + body\n\
         3. for draft: read JSON from ~/.cache/mailforge/drafts/<id>.json\n\
         4. render form via templates::page(PageContext::Compose, ...)"
    )
}

/// POST `/api/send`. Form-encoded body; returns redirect on success.
pub async fn send_post(Form(_form): Form<SendForm>) -> Response {
    todo!(
        "1. accounts::find(&form.from_account) (400 BAD_REQUEST on unknown)\n\
         2. build lettre::Message:\n\
              .from(account.identity) .to(form.to) .subject(form.subject)\n\
              .header(In-Reply-To: form.in_reply_to)\n\
              .body(form.body)\n\
         3. serialize via msg.formatted() → Vec<u8>\n\
         4. spawn send backend per accounts::SendBackend, write MIME to stdin\n\
         5. on success: optional notmuch_db::apply_tag_changes for sent tagging\n\
         6. on failure: re-render compose form with error banner (preserve fields)\n\
         7. on success: 302 to /mail/<account>/sent"
    )
}

/// POST `/api/draft`. JSON body. Persists draft to disk; returns the id
/// (so the client can update its in-memory id after first save).
pub async fn draft_save(Json(_draft): Json<DraftBody>) -> Response {
    todo!(
        "1. compute draft path: cache_root().join('drafts').join(format!(\"{{id}}.json\"))\n\
         2. atomic write: write to <path>.tmp then rename\n\
         3. return {{{{ ok: true, id }}}}"
    )
}

/// GET `/mail/draft/<id>`. Convenience URL for resuming drafts; same
/// effect as `/mail/compose?draft=<id>`.
pub async fn draft_get(Path(_id): Path<String>) -> Response {
    todo!("redirect to /mail/compose?draft={{id}} or render directly")
}
