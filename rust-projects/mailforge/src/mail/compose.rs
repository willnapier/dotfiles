//! Composer: form rendering, send pipeline, draft autosave.
//!
//! GET `/mail/compose` → blank composer.
//! GET `/mail/compose?reply=<msg-id>` → reply prefilled (To, Subject, quoted body).
//! GET `/mail/compose?reply_all=<msg-id>` → reply-all prefilled (To+Cc populated).
//! GET `/mail/compose?fwd=<msg-id>` → forward prefilled.
//! GET `/mail/compose?draft=<id>` → resume saved draft.
//! GET `/mail/draft/<id>` → same as ?draft=<id> (cleaner URL for bookmarks).
//! POST `/api/send` → build MIME, dispatch to msmtp / graph-send, return JSON.
//! POST `/api/draft` → save draft to `~/.cache/mailforge/drafts/<uuid>.json`.
//! GET  `/api/draft/<uuid>` → read draft from disk (Helix-roundtrip support).
//! POST `/api/escalate-helix` → spawn Helix in WezTerm against a tempfile,
//!   return tempfile path so JS can poll for changes.
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
//! 6. On success: move the draft (if any) from drafts/ to sent/. Return
//!    `{ok: true, msg_id}` JSON. Notmuch tag-application is left for the
//!    next `notmuch new` run; the message file lands in maildir `cur/`
//!    once mbsync sees it in the server-sent folder.
//! 7. On failure: leave draft in-place, return `{ok: false, error: ...}`
//!    JSON with retry hint.
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
//! POST `/api/escalate-helix`: receives current body text, writes to a
//! tempfile under `~/.cache/mailforge/helix-tmp/<uuid>.txt`, spawns
//! `wezterm cli spawn -- helix <tempfile>`, returns
//! `{ tempfile_path, expected_re_post: true }`. The client polls
//! GET `/api/draft/<uuid>` after Helix exit (it can stat the file's
//! mtime + read its contents directly). The tempfile is written
//! atomically (write+rename) so polls never see partial content.

#[allow(unused_imports)]
use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    Form, Json,
};
use maud::{html, Markup, PreEscaped};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::mail::accounts::{self, SendBackend};
use crate::mail::notmuch_db;
use crate::mail::templates::{self, PageContext};

// ------------------------------------------------------------------
// Public types
// ------------------------------------------------------------------

/// Query string parameters for the GET compose form.
#[derive(Debug, Default, Deserialize)]
pub struct ComposeQuery {
    /// Reply: prefill To, Subject (with "Re: "), and quoted body.
    pub reply: Option<String>,
    /// Reply-all: prefill To+Cc, Subject, quoted body.
    pub reply_all: Option<String>,
    /// Forward: prefill Subject (with "Fwd: ") and inline-quote the body.
    pub fwd: Option<String>,
    /// Backwards-compat alias for `fwd`.
    pub forward: Option<String>,
    /// Resume a saved draft. Same effect as `/mail/draft/<id>`.
    pub draft: Option<String>,
    /// Pre-select an account (for "compose from cohs" links). Defaults
    /// to the personal account if absent.
    pub from: Option<String>,
    /// Ad-hoc prefill: To. Used by the unsubscribe-via-mailto flow,
    /// which parses an RFC 2369 `mailto:` List-Unsubscribe URL and
    /// navigates to /mail/compose?to=...&subject=...&body=... instead
    /// of triggering the OS mailto handler (Mail.app on macOS).
    pub to: Option<String>,
    /// Ad-hoc prefill: Subject. Pairs with `to` above.
    pub subject: Option<String>,
    /// Ad-hoc prefill: Body. Pairs with `to` above.
    pub body: Option<String>,
    /// When the compose was opened from the unsubscribe-via-mailto flow,
    /// this carries the original sender's message ID so that on
    /// successful Send, the server can tag THAT message
    /// `+unsubscribed +trash -inbox` (mirroring what the one-click POST
    /// path already does for itself). Hidden form field threads this
    /// through to the SendForm.
    pub unsubscribe_for_id: Option<String>,
}

/// Form fields posted to `/api/send`.
///
/// `from_account` is the account slug (`personal` / `cohs`) — drives
/// send-backend selection. The actual `From:` header is filled from
/// the account's `identity` field server-side.
#[derive(Debug, Deserialize, Default)]
pub struct SendForm {
    pub from_account: String,
    pub to: String,
    pub cc: Option<String>,
    pub bcc: Option<String>,
    pub subject: String,
    pub body: String,
    /// In-Reply-To header, if this is a reply. Used to thread.
    pub in_reply_to: Option<String>,
    /// Optional draft id; if present, draft will be moved to sent/ on success.
    pub draft_id: Option<String>,
    /// When this Send is the unsubscribe-via-mailto flow, this carries
    /// the original sender's message ID. On success, the server tags
    /// THAT message `+unsubscribed +trash -inbox` so the inbox row
    /// that triggered the unsub disappears (matching the one-click
    /// POST path's behaviour).
    pub unsubscribe_for_id: Option<String>,
}

/// Draft autosave envelope. Matches `SendForm` plus a draft id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DraftBody {
    pub id: String,
    pub from_account: String,
    pub to: String,
    #[serde(default)]
    pub cc: Option<String>,
    #[serde(default)]
    pub bcc: Option<String>,
    pub subject: String,
    pub body: String,
    #[serde(default)]
    pub in_reply_to: Option<String>,
}

/// Response from POST `/api/draft`. JS reads this to know the canonical id.
#[derive(Debug, Serialize)]
pub struct DraftSaveResponse {
    pub ok: bool,
    pub id: String,
    /// Unix timestamp (seconds) of save.
    pub saved_at: u64,
    pub error: Option<String>,
}

/// Response from POST `/api/send`.
#[derive(Debug, Serialize)]
pub struct SendResponse {
    pub ok: bool,
    pub msg_id: Option<String>,
    pub error: Option<String>,
    /// True if the failure looks transient (network glitch, OAuth refresh
    /// timing). The client uses this to decide whether to keep the draft
    /// and offer a retry vs surface a hard failure.
    #[serde(default)]
    pub retry: bool,
}

/// Response from POST `/api/escalate-helix`.
#[derive(Debug, Serialize)]
pub struct EscalateResponse {
    pub ok: bool,
    pub tempfile_path: Option<String>,
    pub expected_re_post: bool,
    pub error: Option<String>,
}

/// Body of POST `/api/escalate-helix`.
#[derive(Debug, Deserialize)]
pub struct EscalateRequest {
    /// Current draft body to seed the tempfile with.
    pub body: String,
}

/// Response shape for GET `/api/escalate-helix/status`.
#[derive(Debug, Serialize)]
pub struct EscalateStatus {
    /// True once the user has saved a change in Helix (tempfile content
    /// differs from the seed). Once true, the JS poller stops and copies
    /// `body` back into the textarea.
    pub complete: bool,
    /// Current tempfile contents — only set when `complete` is true so the
    /// no-op poll path is cheap.
    pub body: Option<String>,
    /// Diagnostic. Set when no active session, or when reading the
    /// tempfile fails.
    pub error: Option<String>,
}

/// Response shape for POST `/api/escalate-helix/abort`.
#[derive(Debug, Serialize)]
pub struct EscalateAbort {
    pub ok: bool,
}

/// Active Helix escalation session. Single global slot — multiple compose
/// tabs would step on each other but the typical workflow has one
/// composer at a time. If that limitation bites, switch to a HashMap
/// keyed by an id returned in the initial response.
struct EscalateSession {
    tempfile_path: PathBuf,
    seed_content: String,
}

fn current_escalation() -> &'static std::sync::Mutex<Option<EscalateSession>> {
    use std::sync::{Mutex, OnceLock};
    static SLOT: OnceLock<Mutex<Option<EscalateSession>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

// ------------------------------------------------------------------
// Filesystem layout
// ------------------------------------------------------------------

/// `~/.cache/mailforge/drafts/`. Created on demand.
fn drafts_dir() -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    let base = crate::manifest::cache_root()?;
    let p = base.join("drafts");
    std::fs::create_dir_all(&p)
        .with_context(|| format!("mkdir {}", p.display()))?;
    Ok(p)
}

/// `~/.cache/mailforge/sent/`. Mirror of drafts_dir but for successful sends.
fn sent_dir() -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    let base = crate::manifest::cache_root()?;
    let p = base.join("sent");
    std::fs::create_dir_all(&p)
        .with_context(|| format!("mkdir {}", p.display()))?;
    Ok(p)
}

/// `~/.cache/mailforge/helix-tmp/`. Tempfiles for Helix escalation.
fn helix_tmp_dir() -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    let base = crate::manifest::cache_root()?;
    let p = base.join("helix-tmp");
    std::fs::create_dir_all(&p)
        .with_context(|| format!("mkdir {}", p.display()))?;
    Ok(p)
}

/// Sanity-check a draft id: must be non-empty, no path separators, no
/// shell metacharacters. Drafts are user-controlled by URL so we strictly
/// limit what's allowed before joining onto a filesystem path.
fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Atomic write: write to `<path>.tmp` then rename. Avoids the partial-read
/// race when a polling reader looks at the file mid-write.
fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
    use anyhow::Context;
    let parent = path.parent().context("path has no parent")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("mkdir {}", parent.display()))?;
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|s| s.to_str()).unwrap_or("")
    ));
    {
        let mut f = std::fs::File::create(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(bytes)
            .with_context(|| format!("writing {}", tmp.display()))?;
        f.sync_all().ok();
    }
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ------------------------------------------------------------------
// Reply quoting
// ------------------------------------------------------------------

/// Build a reply attribution line + quoted body from the parent message.
///
/// Returns `(to, cc, subject, quoted_body, in_reply_to)`.
///
/// Reply (default): To = original From, Cc = empty.
/// Reply-all: To = original From, Cc = original To+Cc minus our own identity.
///
/// Strips our own `identity` from to/cc to avoid replying to ourselves.
pub fn build_reply(
    parent_raw: &[u8],
    own_identity: &str,
    reply_all: bool,
) -> (String, String, String, String, Option<String>) {
    use mail_parser::MessageParser;

    let Some(msg) = MessageParser::default().parse(parent_raw) else {
        return (String::new(), String::new(), "Re: ".to_string(), String::new(), None);
    };

    // Determine To: original From (or Reply-To if present).
    let to = msg
        .reply_to()
        .and_then(|a| a.first())
        .and_then(|a| a.address())
        .or_else(|| msg.from().and_then(|a| a.first()).and_then(|a| a.address()))
        .map(|s| s.to_string())
        .unwrap_or_default();

    // Cc only filled for reply-all: union of original To + Cc, dropping
    // our own identity and the new To address.
    let mut cc_addrs: Vec<String> = Vec::new();
    if reply_all {
        let collect = |addrs: Option<&mail_parser::Address<'_>>, out: &mut Vec<String>| {
            if let Some(a) = addrs {
                for addr in a.iter() {
                    if let Some(s) = addr.address() {
                        out.push(s.to_string());
                    }
                }
            }
        };
        collect(msg.to(), &mut cc_addrs);
        collect(msg.cc(), &mut cc_addrs);
        // Filter out self + the new To.
        cc_addrs.retain(|a| !a.eq_ignore_ascii_case(own_identity) && !a.eq_ignore_ascii_case(&to));
        // De-duplicate while preserving order.
        let mut seen = std::collections::HashSet::new();
        cc_addrs.retain(|a| seen.insert(a.to_lowercase()));
    }
    let cc = cc_addrs.join(", ");

    // Subject: prepend "Re: " if not already there (case-insensitive).
    let subject_orig = msg.subject().unwrap_or("").trim();
    let subject = if subject_orig.to_ascii_lowercase().starts_with("re:") {
        subject_orig.to_string()
    } else if subject_orig.is_empty() {
        "Re: ".to_string()
    } else {
        format!("Re: {subject_orig}")
    };

    // Attribution + quoted body.
    let from_display = msg
        .from()
        .and_then(|a| a.first())
        .map(|addr| {
            let name = addr.name().unwrap_or("");
            let email = addr.address().unwrap_or("");
            if name.is_empty() {
                email.to_string()
            } else {
                format!("{name} <{email}>")
            }
        })
        .unwrap_or_default();
    let date_str = msg
        .date()
        .map(|d| d.to_rfc822())
        .unwrap_or_else(|| "an earlier date".to_string());
    let attribution = format!("On {date_str}, {from_display} wrote:\n");

    let body = msg.body_text(0).map(|c| c.to_string()).unwrap_or_default();
    let quoted = quote_text(&body);
    let quoted_body = format!("\n\n{attribution}{quoted}");

    let in_reply_to = msg.message_id().map(|s| s.to_string());

    (to, cc, subject, quoted_body, in_reply_to)
}

/// Build subject + inline-quoted body for a forward.
///
/// Returns `(subject, fwd_body)`. To/Cc are intentionally left blank; the
/// user fills them in. No In-Reply-To: a forward is not threaded against
/// the original (it's a new conversation aimed at a different audience).
pub fn build_forward(parent_raw: &[u8]) -> (String, String) {
    use mail_parser::MessageParser;

    let Some(msg) = MessageParser::default().parse(parent_raw) else {
        return ("Fwd: ".to_string(), String::new());
    };

    let subject_orig = msg.subject().unwrap_or("").trim();
    let subject = if subject_orig.to_ascii_lowercase().starts_with("fwd:")
        || subject_orig.to_ascii_lowercase().starts_with("fw:")
    {
        subject_orig.to_string()
    } else if subject_orig.is_empty() {
        "Fwd: ".to_string()
    } else {
        format!("Fwd: {subject_orig}")
    };

    let from_display = msg
        .from()
        .and_then(|a| a.first())
        .and_then(|addr| addr.address())
        .unwrap_or("")
        .to_string();
    let to_display = msg
        .to()
        .map(|a| {
            a.iter()
                .filter_map(|addr| addr.address())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let date_str = msg
        .date()
        .map(|d| d.to_rfc822())
        .unwrap_or_default();
    let subj_orig_str = msg.subject().unwrap_or("");

    let body = msg.body_text(0).map(|c| c.to_string()).unwrap_or_default();
    let separator = format!(
        "\n\n---------- Forwarded message ----------\n\
         From: {from_display}\n\
         Date: {date_str}\n\
         Subject: {subj_orig_str}\n\
         To: {to_display}\n\n",
    );
    let fwd_body = format!("{separator}{body}");

    (subject, fwd_body)
}

/// Prefix every line with "> ". Empty input → empty output.
fn quote_text(body: &str) -> String {
    if body.is_empty() {
        return String::new();
    }
    body.lines()
        .map(|l| if l.is_empty() { ">".to_string() } else { format!("> {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

// ------------------------------------------------------------------
// MIME building
// ------------------------------------------------------------------

/// Build the RFC822 bytes for a `SendForm`. Pure — no IO. Returns an error
/// if the form contains unparseable addresses.
///
/// Address fields with comma-separated entries are split before parsing.
/// Empty cc/bcc are no-ops.
pub fn build_mime(form: &SendForm, identity: &str) -> anyhow::Result<Vec<u8>> {
    use anyhow::{anyhow, Context};
    use lettre::message::header::ContentType;
    use lettre::message::{Mailbox as LettreMailbox, SinglePart};
    use lettre::Message;

    if form.to.trim().is_empty() {
        return Err(anyhow!("To: address is empty"));
    }

    let from_addr: LettreMailbox = identity.parse().context("parsing From address")?;
    let mut builder = Message::builder().from(from_addr);

    let parse_addrs = |raw: &str| -> anyhow::Result<Vec<LettreMailbox>> {
        raw.split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.parse::<LettreMailbox>().with_context(|| format!("parsing address: {s}")))
            .collect()
    };

    for to in parse_addrs(&form.to)? {
        builder = builder.to(to);
    }
    if let Some(cc) = form.cc.as_deref() {
        for c in parse_addrs(cc)? {
            builder = builder.cc(c);
        }
    }
    if let Some(bcc) = form.bcc.as_deref() {
        for b in parse_addrs(bcc)? {
            builder = builder.bcc(b);
        }
    }
    builder = builder.subject(form.subject.clone());

    // Always set a Message-ID so we can echo it back in the SendResponse for
    // diagnostics + future threading. Domain comes from the From-identity so
    // the id is realistic ("<uuid@willnapier.com>"), not lettre's
    // hostname-feature-fallback.
    let domain = identity
        .rsplit_once('@')
        .map(|(_, d)| d.to_string())
        .unwrap_or_else(|| "mailforge.local".to_string());
    let mid = format!("{}@{}", uuid::Uuid::new_v4(), domain);
    builder = builder.message_id(Some(mid));

    if let Some(irt) = form.in_reply_to.as_deref().filter(|s| !s.is_empty()) {
        // lettre wraps the raw id in <...> if not already; make sure the
        // header value has angle-bracket delimiters per RFC5322.
        let id = if irt.starts_with('<') {
            irt.to_string()
        } else {
            format!("<{irt}>")
        };
        builder = builder.in_reply_to(id.clone()).references(id);
    }

    let body_part = SinglePart::builder()
        .header(ContentType::TEXT_PLAIN)
        .body(form.body.clone());

    let msg = builder
        .singlepart(body_part)
        .context("building MIME message")?;

    Ok(msg.formatted())
}

/// Dispatch RFC822 bytes to the configured backend. Returns the backend's
/// output for diagnostics.
fn dispatch_send(backend: &SendBackend, mime: &[u8]) -> anyhow::Result<()> {
    use anyhow::{bail, Context};
    let (cmd, args): (&str, Vec<String>) = match backend {
        SendBackend::Msmtp { account } => (
            "msmtp",
            vec![
                format!("--account={account}"),
                "--read-recipients".to_string(),
                "--read-envelope-from".to_string(),
            ],
        ),
        SendBackend::GraphSend => ("graph-send", Vec::new()),
    };

    let mut child = Command::new(cmd)
        .args(&args)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "spawning {cmd} (is it installed and on PATH? msmtp: brew install msmtp; \
                 graph-send: practiceforge ships it to ~/.local/bin/)"
            )
        })?;

    {
        let stdin = child.stdin.as_mut().expect("piped stdin");
        stdin
            .write_all(mime)
            .with_context(|| format!("writing MIME to {cmd} stdin"))?;
    }

    let output = child.wait_with_output().with_context(|| format!("waiting for {cmd}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "{cmd} failed (exit {}): stderr={} stdout={}",
            output.status,
            stderr.trim(),
            stdout.trim()
        );
    }
    Ok(())
}

// ------------------------------------------------------------------
// HTML form
// ------------------------------------------------------------------

/// Render the compose form for an account + prefilled values.
fn render_form(
    account_slug: &str,
    to: &str,
    cc: &str,
    bcc: &str,
    subject: &str,
    body: &str,
    in_reply_to: Option<&str>,
    draft_id: Option<&str>,
    unsubscribe_for_id: Option<&str>,
) -> Markup {
    let title_label = if draft_id.is_some() {
        "Resume draft"
    } else if in_reply_to.is_some() {
        "Reply"
    } else {
        "Compose"
    };

    html! {
        section.compose-wrapper {
            header.compose-header {
                h1 { (title_label) }
                // Shortcut hint paragraph removed — same bindings are
                // already documented in the sticky helpbar at the bottom
                // of the page (Ctrl+Enter send, Ctrl+S save, Ctrl+E
                // Helix, Esc cancel). Duplicating them here is chrome.
            }

            form id="compose-form"
                 class="compose stack"
                 method="POST"
                 action="/api/send"
                 enctype="application/x-www-form-urlencoded"
                 data-draft-id=(draft_id.unwrap_or(""))
                 data-autosave-url="/api/draft"
                 data-escalate-url="/api/escalate-helix"
            {
                // Account selector (slug-only; identity rendered for context).
                label.field {
                    span.field-label { "From" }
                    select name="from_account" {
                        @for acc in accounts::ACCOUNTS {
                            @if acc.slug == account_slug {
                                option value=(acc.slug) selected {
                                    (acc.display_name) " <" (acc.identity) ">"
                                }
                            } @else {
                                option value=(acc.slug) {
                                    (acc.display_name) " <" (acc.identity) ">"
                                }
                            }
                        }
                    }
                }

                label.field {
                    span.field-label { "To" }
                    input type="text" name="to" value=(to)
                          placeholder="recipient@example.com (comma-separated)"
                          required;
                }

                label.field {
                    span.field-label { "Cc" }
                    input type="text" name="cc" value=(cc)
                          placeholder="(optional)";
                }

                label.field {
                    span.field-label { "Bcc" }
                    input type="text" name="bcc" value=(bcc)
                          placeholder="(optional)";
                }

                label.field {
                    span.field-label { "Subject" }
                    input type="text" name="subject" value=(subject)
                          placeholder="(no subject)";
                }

                label.field {
                    span.field-label { "Body" }
                    textarea name="body" rows="20"
                    {
                        (body)
                    }
                }

                @if let Some(irt) = in_reply_to {
                    input type="hidden" name="in_reply_to" value=(irt);
                }
                @if let Some(id) = draft_id {
                    input type="hidden" name="draft_id" value=(id);
                }
                // For mailto-unsubscribe sends: original sender's
                // message ID. Server tags THAT message (not the
                // outgoing one) on successful Send.
                @if let Some(id) = unsubscribe_for_id {
                    input type="hidden" name="unsubscribe_for_id" value=(id);
                }

                .cluster {
                    button type="submit" class="primary" { "Send" }
                    button type="button" id="save-draft-now" { "Save draft now" }
                    button type="button" id="open-helix" {
                        "Edit body in Helix (Ctrl+E)"
                    }
                }

                .compose-feedback {
                    span id="autosave-indicator" class="compose-status" { "" }
                    span id="send-feedback" { "" }
                }
            }
        }
    }
}

// ------------------------------------------------------------------
// Handlers
// ------------------------------------------------------------------

/// GET `/mail/compose` (and variants via query string).
pub async fn compose_form(Query(q): Query<ComposeQuery>) -> Response {
    // Branch on prefill source. Order: draft → reply_all → reply → fwd → blank.
    if let Some(id) = q.draft.as_deref() {
        if !is_safe_id(id) {
            return (StatusCode::BAD_REQUEST, "invalid draft id").into_response();
        }
        match load_draft(id) {
            Ok(d) => {
                let body = render_form(
                    &d.from_account,
                    &d.to,
                    d.cc.as_deref().unwrap_or(""),
                    d.bcc.as_deref().unwrap_or(""),
                    &d.subject,
                    &d.body,
                    d.in_reply_to.as_deref(),
                    Some(&d.id),
                    None,
                );
                let doc = templates::page(
                    "Compose - mailforge",
                    PageContext::Compose,
                    None,
                    None,
                    body,
                );
                return Html(doc).into_response();
            }
            Err(e) => {
                return (
                    StatusCode::NOT_FOUND,
                    format!("draft not found: {e}"),
                )
                    .into_response();
            }
        }
    }

    let from_slug = q
        .from
        .as_deref()
        .filter(|s| accounts::find(s).is_some())
        .map(|s| s.to_string())
        .unwrap_or_else(|| accounts::default_account().slug.to_string());

    let parent_id = q.reply.as_deref()
        .or(q.reply_all.as_deref())
        .or(q.fwd.as_deref())
        .or(q.forward.as_deref());

    if let Some(id) = parent_id {
        // Load the parent message bytes via notmuch_db.
        let parent_bytes = match load_parent_bytes(id) {
            Ok(b) => b,
            Err(e) => {
                return (
                    StatusCode::NOT_FOUND,
                    format!("parent message not found: {e}"),
                )
                    .into_response();
            }
        };

        let identity = accounts::find(&from_slug)
            .map(|a| a.identity)
            .unwrap_or("");

        let (to, cc, subject, body, in_reply_to);
        if q.fwd.is_some() || q.forward.is_some() {
            let (s, b) = build_forward(&parent_bytes);
            to = String::new();
            cc = String::new();
            subject = s;
            body = b;
            in_reply_to = None;
        } else {
            let reply_all = q.reply_all.is_some();
            let (t, c, s, b, irt) = build_reply(&parent_bytes, identity, reply_all);
            to = t;
            cc = c;
            subject = s;
            body = b;
            in_reply_to = irt;
        }

        let body_markup = render_form(
            &from_slug,
            &to,
            &cc,
            "",
            &subject,
            &body,
            in_reply_to.as_deref(),
            None,
            None,
        );
        let doc = templates::page(
            "Compose - mailforge",
            PageContext::Compose,
            None,
            None,
            body_markup,
        );
        return Html(doc).into_response();
    }

    // Blank composer (or ad-hoc prefilled via to/subject/body query params,
    // used by the unsubscribe-via-mailto flow).
    let to = q.to.as_deref().unwrap_or("");
    let subject = q.subject.as_deref().unwrap_or("");
    let prefill_body = q.body.as_deref().unwrap_or("");
    let body = render_form(&from_slug, to, "", "", subject, prefill_body, None, None, q.unsubscribe_for_id.as_deref());
    let doc = templates::page(
        "Compose - mailforge",
        PageContext::Compose,
        None,
        None,
        body,
    );
    Html(doc).into_response()
}

/// Load the raw RFC822 bytes for a notmuch message id.
fn load_parent_bytes(id: &str) -> anyhow::Result<Vec<u8>> {
    use anyhow::{anyhow, Context};
    let msg = notmuch_db::show(id).with_context(|| format!("notmuch show id:{id}"))?;
    let path = msg
        .filename
        .ok_or_else(|| anyhow!("message {id} has no filename in notmuch"))?;
    std::fs::read(&path).with_context(|| format!("reading {path}"))
}

/// POST `/api/send`. Form-encoded body; returns JSON on success or failure.
///
/// Move semantics: on success, draft_id (if present) is moved from
/// `drafts/<id>.json` to `sent/<id>.json` so the user has a sent record
/// without polluting the drafts folder. On failure, the draft stays put.
pub async fn send_post(Form(form): Form<SendForm>) -> Response {
    let Some(account) = accounts::find(&form.from_account) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(SendResponse {
                ok: false,
                msg_id: None,
                error: Some(format!("unknown account: {}", form.from_account)),
                retry: false,
            }),
        )
            .into_response();
    };

    // Build MIME.
    let mime = match build_mime(&form, account.identity) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SendResponse {
                    ok: false,
                    msg_id: None,
                    error: Some(format!("MIME build failed: {e:#}")),
                    retry: false,
                }),
            )
                .into_response();
        }
    };

    // Run the send synchronously on a blocking task (msmtp/graph-send are
    // blocking subprocesses; we don't want to stall the tokio runtime).
    let backend = account.send;
    let mime_clone = mime.clone();
    let send_result = tokio::task::spawn_blocking(move || dispatch_send(&backend, &mime_clone))
        .await
        .unwrap_or_else(|e| Err(anyhow::anyhow!("join error: {e}")));

    match send_result {
        Ok(()) => {
            // Try to extract the Message-ID we generated for diagnostics.
            let msg_id = extract_message_id(&mime);

            // Move draft to sent/ on success.
            if let Some(draft_id) = form.draft_id.as_deref() {
                if is_safe_id(draft_id) {
                    let _ = move_draft_to_sent(draft_id);
                }
            }

            // If this was the unsubscribe-via-mailto flow, tag the
            // ORIGINAL message (the inbox row that triggered the unsub)
            // as +unsubscribed +trash -inbox so it disappears from the
            // inbox view, mirroring the one-click POST path's behaviour.
            // Best-effort: log a warning on failure but still return Ok
            // (the user's email did go out — that's the user-facing
            // success criterion).
            if let Some(orig_id) = form.unsubscribe_for_id.as_deref() {
                let q = format!("id:{orig_id}");
                if let Err(e) = crate::mail::notmuch_db::apply_tag_changes(
                    &q,
                    &["unsubscribed", "trash"],
                    &["inbox"],
                ) {
                    tracing::warn!(
                        "send_post: tag-update for unsubscribe_for_id={orig_id} failed: {e}"
                    );
                }
            }

            (
                StatusCode::OK,
                Json(SendResponse {
                    ok: true,
                    msg_id,
                    error: None,
                    retry: false,
                }),
            )
                .into_response()
        }
        Err(e) => {
            // Heuristic: pizauth / network errors are likely transient.
            let err_str = format!("{e:#}");
            let retry = err_str.contains("pizauth")
                || err_str.contains("network")
                || err_str.contains("timeout")
                || err_str.contains("Connection")
                || err_str.contains("temporarily");
            (
                StatusCode::BAD_GATEWAY,
                Json(SendResponse {
                    ok: false,
                    msg_id: None,
                    error: Some(err_str),
                    retry,
                }),
            )
                .into_response()
        }
    }
}

/// Pluck the `Message-ID` header out of the formatted MIME bytes (lettre
/// auto-generates one when none is provided). Best-effort; returns None
/// if not found.
fn extract_message_id(mime: &[u8]) -> Option<String> {
    let head_end = mime.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(mime.len());
    let head = &mime[..head_end];
    let head_str = std::str::from_utf8(head).ok()?;
    for line in head_str.split("\r\n") {
        if let Some(rest) = line.strip_prefix("Message-ID:").or_else(|| line.strip_prefix("Message-Id:")) {
            return Some(rest.trim().trim_matches(|c| c == '<' || c == '>').to_string());
        }
    }
    None
}

fn move_draft_to_sent(id: &str) -> anyhow::Result<()> {
    let from = drafts_dir()?.join(format!("{id}.json"));
    if !from.exists() {
        return Ok(());
    }
    let to = sent_dir()?.join(format!("{id}.json"));
    std::fs::rename(&from, &to).map_err(|e| anyhow::anyhow!("rename draft: {e}"))?;
    Ok(())
}

/// POST `/api/draft`. JSON body. Persists draft to disk; returns the id.
pub async fn draft_save(Json(draft): Json<DraftBody>) -> Response {
    if !is_safe_id(&draft.id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(DraftSaveResponse {
                ok: false,
                id: draft.id.clone(),
                saved_at: 0,
                error: Some("invalid draft id".to_string()),
            }),
        )
            .into_response();
    }

    let Ok(dir) = drafts_dir() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(DraftSaveResponse {
                ok: false,
                id: draft.id.clone(),
                saved_at: 0,
                error: Some("cannot create drafts dir".to_string()),
            }),
        )
            .into_response();
    };
    let path = dir.join(format!("{}.json", draft.id));

    let bytes = match serde_json::to_vec_pretty(&draft) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(DraftSaveResponse {
                    ok: false,
                    id: draft.id.clone(),
                    saved_at: 0,
                    error: Some(format!("serialise draft: {e}")),
                }),
            )
                .into_response();
        }
    };

    if let Err(e) = atomic_write(&path, &bytes) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(DraftSaveResponse {
                ok: false,
                id: draft.id.clone(),
                saved_at: 0,
                error: Some(format!("write draft: {e:#}")),
            }),
        )
            .into_response();
    }

    Json(DraftSaveResponse {
        ok: true,
        id: draft.id,
        saved_at: now_unix_secs(),
        error: None,
    })
    .into_response()
}

/// GET `/api/draft/<id>`. Returns the JSON body that POST `/api/draft`
/// last wrote to disk. 404 on unknown id, 400 on bad id.
pub async fn draft_get_api(Path(id): Path<String>) -> Response {
    if !is_safe_id(&id) {
        return (StatusCode::BAD_REQUEST, "invalid draft id").into_response();
    }
    match load_draft(&id) {
        Ok(d) => Json(d).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "draft not found").into_response(),
    }
}

fn load_draft(id: &str) -> anyhow::Result<DraftBody> {
    use anyhow::Context;
    let path = drafts_dir()?.join(format!("{id}.json"));
    let bytes = std::fs::read(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let draft: DraftBody = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok(draft)
}

/// GET `/mail/draft/<id>`. Convenience URL for resuming drafts; redirects
/// to `/mail/compose?draft=<id>` so all the prefill logic lives in one place.
pub async fn draft_get(Path(id): Path<String>) -> Response {
    if !is_safe_id(&id) {
        return (StatusCode::BAD_REQUEST, "invalid draft id").into_response();
    }
    Redirect::to(&format!("/mail/compose?draft={id}")).into_response()
}

/// POST `/api/escalate-helix`. Writes the body to a tempfile, spawns
/// Helix via wezterm, returns the tempfile path so the JS can poll it.
///
/// Tempfile-race protection:
///   1. Tempfile is created with `atomic_write` (write to .tmp then rename),
///      so polls never see a partial file.
///   2. The path is returned to the client; the client is expected to
///      poll the file's mtime and read its contents AFTER detecting that
///      Helix has exited (via a separate signal — typically the user
///      clicks "I'm done" in the UI, or the JS detects window blur).
///   3. Helix's auto-save (250ms) flushes the buffer atomically via
///      Helix's own write logic before exit.
pub async fn escalate_helix(Json(req): Json<EscalateRequest>) -> Response {
    use anyhow::Context;

    let id = uuid::Uuid::new_v4().to_string();
    let dir = match helix_tmp_dir() {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(EscalateResponse {
                    ok: false,
                    tempfile_path: None,
                    expected_re_post: false,
                    error: Some(format!("helix-tmp dir: {e:#}")),
                }),
            )
                .into_response();
        }
    };
    let path = dir.join(format!("mailforge-draft-{id}.txt"));

    if let Err(e) = atomic_write(&path, req.body.as_bytes())
        .with_context(|| format!("seeding tempfile {}", path.display()))
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(EscalateResponse {
                ok: false,
                tempfile_path: None,
                expected_re_post: false,
                error: Some(format!("{e:#}")),
            }),
        )
            .into_response();
    }

    // Spawn Helix via wezterm. Use `wezterm cli spawn -- helix <path>`
    // when WezTerm is already running (preferred — opens in a new tab);
    // fall back to `wezterm start --always-new-process -- helix <path>`
    // which spawns a fresh window if the CLI socket isn't connected.
    let path_str = path.to_string_lossy().to_string();
    let path_for_spawn = path_str.clone();

    let spawn_result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        // First attempt: existing WezTerm session (new tab in current window).
        let attempt_cli = Command::new("wezterm")
            .args(["cli", "spawn", "--", "helix", &path_for_spawn])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .status();
        if let Ok(s) = attempt_cli {
            if s.success() {
                return Ok(());
            }
        }
        // Fallback: start a new WezTerm window.
        Command::new("wezterm")
            .args([
                "start",
                "--always-new-process",
                "--",
                "helix",
                &path_for_spawn,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("wezterm spawn failed: {e}"))
    })
    .await
    .unwrap_or_else(|e| Err(anyhow::anyhow!("join error: {e}")));

    if let Err(e) = spawn_result {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(EscalateResponse {
                ok: false,
                tempfile_path: Some(path_str),
                expected_re_post: false,
                error: Some(format!("{e:#}")),
            }),
        )
            .into_response();
    }

    // Record the session so /status can read the tempfile and detect
    // changes. Drop the previous session if any (single-slot semantics).
    *current_escalation().lock().unwrap() = Some(EscalateSession {
        tempfile_path: path.clone(),
        seed_content: req.body.clone(),
    });

    Json(EscalateResponse {
        ok: true,
        tempfile_path: Some(path_str),
        expected_re_post: true,
        error: None,
    })
    .into_response()
}

/// GET `/api/escalate-helix/status`. Polled by the composer JS after a
/// successful escalation. Returns `complete: true` and the new body once
/// the tempfile content differs from the seed (i.e. the user has saved
/// at least one change in Helix). Until then returns `complete: false`
/// so the JS keeps polling.
pub async fn escalate_helix_status() -> Json<EscalateStatus> {
    let guard = current_escalation().lock().unwrap();
    let Some(s) = guard.as_ref() else {
        return Json(EscalateStatus {
            complete: false,
            body: None,
            error: Some("no active session".into()),
        });
    };
    match std::fs::read_to_string(&s.tempfile_path) {
        Ok(current) if current != s.seed_content => Json(EscalateStatus {
            complete: true,
            body: Some(current),
            error: None,
        }),
        Ok(_) => Json(EscalateStatus { complete: false, body: None, error: None }),
        Err(e) => Json(EscalateStatus {
            complete: false,
            body: None,
            error: Some(format!("read tempfile: {e}")),
        }),
    }
}

/// POST `/api/escalate-helix/abort`. Clears the active session and
/// removes its tempfile. Doesn't kill Helix — `wezterm cli spawn` returns
/// once the tab is opened so we have no handle on the editor process.
/// The user closes the Helix tab manually.
pub async fn escalate_helix_abort() -> Json<EscalateAbort> {
    if let Some(s) = current_escalation().lock().unwrap().take() {
        let _ = std::fs::remove_file(&s.tempfile_path);
    }
    Json(EscalateAbort { ok: true })
}

// Marker so PreEscaped (re-exported by maud) doesn't get flagged as unused
// even if a future template stops needing it. PreEscaped is left available
// for impl agents who need to inject pre-escaped HTML into the form (e.g.
// rendering a server-side error banner with embedded markup).
#[allow(dead_code)]
fn _force_preescaped_in_scope() -> PreEscaped<&'static str> {
    PreEscaped("")
}

// ------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Realistic-looking inbound message used by reply / forward tests.
    /// Includes Date, From, To, Cc, Subject, Message-ID, and a multi-line
    /// plain body with a blank line in the middle.
    fn sample_parent() -> Vec<u8> {
        b"From: Alice Example <alice@example.com>\r\n\
          To: \"Will Napier\" <will@willnapier.com>, bob@example.com\r\n\
          Cc: carol@example.com\r\n\
          Date: Tue, 28 Apr 2026 14:30:00 +0100\r\n\
          Subject: Quarterly review notes\r\n\
          Message-ID: <abc123@example.com>\r\n\
          MIME-Version: 1.0\r\n\
          Content-Type: text/plain; charset=utf-8\r\n\
          \r\n\
          Hi Will,\r\n\
          \r\n\
          Could you take a look at the attached numbers?\r\n\
          Particularly figure 3.\r\n\
          \r\n\
          Cheers,\r\n\
          Alice\r\n"
            .to_vec()
    }

    #[test]
    fn quote_text_prefixes_each_line() {
        let input = "line one\n\nline three";
        let got = quote_text(input);
        assert_eq!(got, "> line one\n>\n> line three");
    }

    #[test]
    fn quote_text_empty_input_yields_empty() {
        assert_eq!(quote_text(""), "");
    }

    #[test]
    fn build_reply_basic() {
        let parent = sample_parent();
        let (to, cc, subject, body, irt) =
            build_reply(&parent, "will@willnapier.com", false);

        assert_eq!(to, "alice@example.com");
        assert_eq!(cc, "", "plain reply must have empty Cc");
        assert_eq!(subject, "Re: Quarterly review notes");
        assert!(
            body.contains("> Hi Will,"),
            "body should contain quoted greeting; got: {body}"
        );
        assert!(
            body.contains("Alice Example <alice@example.com> wrote:"),
            "attribution must include sender; got: {body}"
        );
        assert_eq!(irt.as_deref(), Some("abc123@example.com"));
    }

    #[test]
    fn build_reply_does_not_double_re_prefix() {
        let parent = b"From: a@x.com\r\n\
                       Subject: Re: Already a reply\r\n\
                       \r\n\
                       Body\r\n"
            .to_vec();
        let (_to, _cc, subject, _body, _irt) =
            build_reply(&parent, "me@x.com", false);
        // Must not become "Re: Re: ..."
        assert_eq!(subject, "Re: Already a reply");
    }

    #[test]
    fn build_reply_all_includes_other_recipients_and_excludes_self() {
        let parent = sample_parent();
        let (to, cc, _subject, _body, _irt) =
            build_reply(&parent, "will@willnapier.com", true);

        assert_eq!(to, "alice@example.com");
        // Cc should include bob (other To) and carol (original Cc),
        // and EXCLUDE will@willnapier.com.
        assert!(cc.contains("bob@example.com"), "cc missing bob: {cc}");
        assert!(cc.contains("carol@example.com"), "cc missing carol: {cc}");
        assert!(
            !cc.to_ascii_lowercase().contains("will@willnapier.com"),
            "cc must not echo self back: {cc}"
        );
    }

    #[test]
    fn build_forward_strips_in_reply_to_and_inlines_quote() {
        let parent = sample_parent();
        let (subject, body) = build_forward(&parent);
        assert_eq!(subject, "Fwd: Quarterly review notes");
        assert!(body.contains("---------- Forwarded message ----------"));
        assert!(body.contains("From: alice@example.com"));
        assert!(
            body.contains("Hi Will,"),
            "fwd body should inline the original; got: {body}"
        );
    }

    #[test]
    fn build_forward_handles_unparseable_input() {
        let (subject, body) = build_forward(b"this is not a valid email");
        // Mail-parser is permissive — it'll happily produce a Message even
        // for garbage input. The contract here is that we don't crash and
        // do prefix the subject with "Fwd: ".
        assert!(subject.starts_with("Fwd:"), "got subject={subject}");
        assert!(body.contains("Forwarded message"));
    }

    #[test]
    fn draft_round_trip_preserves_fields() {
        let original = DraftBody {
            id: "test-rt-001".to_string(),
            from_account: "personal".to_string(),
            to: "alice@example.com".to_string(),
            cc: Some("bob@example.com".to_string()),
            bcc: None,
            subject: "Round trip".to_string(),
            body: "Line 1\nLine 2\n".to_string(),
            in_reply_to: Some("xyz@example.com".to_string()),
        };
        let bytes = serde_json::to_vec(&original).unwrap();
        let decoded: DraftBody = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn build_mime_produces_valid_rfc822() {
        let form = SendForm {
            from_account: "personal".to_string(),
            to: "alice@example.com".to_string(),
            cc: Some("bob@example.com, carol@example.com".to_string()),
            bcc: None,
            subject: "Hello there".to_string(),
            body: "First line\nSecond line\n".to_string(),
            in_reply_to: Some("abc123@example.com".to_string()),
            draft_id: None,
            ..Default::default()
        };
        let bytes = build_mime(&form, "will@willnapier.com").expect("mime builds");
        let s = std::str::from_utf8(&bytes).expect("utf8");

        // Basic header sanity
        assert!(s.contains("From:"));
        assert!(s.contains("will@willnapier.com"));
        assert!(s.contains("To:"));
        assert!(s.contains("alice@example.com"));
        assert!(s.contains("Cc:"));
        assert!(s.contains("bob@example.com"));
        assert!(s.contains("carol@example.com"));
        assert!(s.contains("Subject:"));
        assert!(s.contains("Hello there"));
        assert!(s.contains("In-Reply-To:"));
        assert!(s.contains("<abc123@example.com>"));
        // Body starts after blank line.
        assert!(s.contains("\r\n\r\n"));
        assert!(s.contains("First line"));
        assert!(s.contains("Second line"));
    }

    #[test]
    fn build_mime_rejects_empty_to() {
        let form = SendForm {
            from_account: "personal".to_string(),
            to: "".to_string(),
            cc: None,
            bcc: None,
            subject: "x".to_string(),
            body: "".to_string(),
            in_reply_to: None,
            draft_id: None,
            ..Default::default()
        };
        let r = build_mime(&form, "will@willnapier.com");
        assert!(r.is_err());
    }

    #[test]
    fn build_mime_rejects_unparseable_address() {
        let form = SendForm {
            from_account: "personal".to_string(),
            to: "this is not an address".to_string(),
            cc: None,
            bcc: None,
            subject: "x".to_string(),
            body: "".to_string(),
            in_reply_to: None,
            draft_id: None,
            ..Default::default()
        };
        let r = build_mime(&form, "will@willnapier.com");
        assert!(r.is_err());
    }

    #[test]
    fn extract_message_id_finds_lettre_generated_id() {
        let form = SendForm {
            from_account: "personal".to_string(),
            to: "alice@example.com".to_string(),
            cc: None,
            bcc: None,
            subject: "id test".to_string(),
            body: "hello".to_string(),
            in_reply_to: None,
            draft_id: None,
            ..Default::default()
        };
        let bytes = build_mime(&form, "will@willnapier.com").unwrap();
        let id = extract_message_id(&bytes);
        assert!(id.is_some(), "lettre should auto-generate a Message-ID");
    }

    #[test]
    fn is_safe_id_accepts_alphanumeric_and_dashes() {
        assert!(is_safe_id("abc-123_xyz"));
        assert!(is_safe_id("550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn is_safe_id_rejects_path_traversal() {
        assert!(!is_safe_id("../etc/passwd"));
        assert!(!is_safe_id("a/b"));
        assert!(!is_safe_id(""));
        assert!(!is_safe_id("a b"));
        assert!(!is_safe_id("a;b"));
    }

    #[test]
    fn atomic_write_creates_file_with_correct_bytes() {
        let dir = tempdir_for_test();
        let path = dir.join("test.json");
        atomic_write(&path, b"hello world").unwrap();
        let read = std::fs::read(&path).unwrap();
        assert_eq!(read, b"hello world");
        // No leftover .tmp file
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn atomic_write_overwrites_existing() {
        let dir = tempdir_for_test();
        let path = dir.join("test.json");
        atomic_write(&path, b"first").unwrap();
        atomic_write(&path, b"second").unwrap();
        let read = std::fs::read(&path).unwrap();
        assert_eq!(read, b"second");
    }

    /// Per-test temporary directory under target/test-tmp/. Avoids tempfile
    /// crate dependency for now.
    fn tempdir_for_test() -> std::path::PathBuf {
        let tname = std::thread::current().name().unwrap_or("anon").to_string();
        let pid = std::process::id();
        let nonce = uuid::Uuid::new_v4();
        let p = std::env::temp_dir().join(format!("mailforge-test-{pid}-{tname}-{nonce}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
