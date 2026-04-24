//! send — construct RFC 5322 MIME and pipe to msmtp.
//!
//! Replaces the earlier `himalaya message send` subprocess path (himalaya
//! retired 2026-04-24). The new stack is:
//!
//! ```text
//! bequest (this crate)  →  lettre (MIME)  →  msmtp --account=gmail
//!                                                        │
//!                                                        ▼
//!                                              pizauth show gmail  (XOAUTH2)
//! ```
//!
//! msmtp is configured at `~/.config/msmtp/config` (dotfile-managed) with
//! `passwordeval = pizauth show gmail` on its Gmail account. pizauth is
//! the OAuth broker daemon; tokens auto-refresh. See
//! `~/Assistants/shared/CLI-EMAIL-SYSTEM.md` for the full architecture.
//!
//! bequest sends only from the personal Gmail identity — the dead-man's
//! switch is William's, not COHS's — so the --account=gmail choice is
//! hard-coded. If a future caller needs COHS send, use a different path
//! (Microsoft tenant blocks SMTP AUTH; needs `practiceforge email graph-send`).

use anyhow::{bail, Context, Result};
use lettre::message::{header::ContentType, Attachment, MultiPart, SinglePart};
use lettre::Message;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Send a plain-text email, optionally with one or more file attachments.
///
/// - `from`: RFC 5322 From address (must be will@willnapier.com — msmtp's
///   gmail account authenticates as that user). Bequest's config
///   validates non-emptiness at the call site.
/// - `to`: single recipient.
/// - `subject`: plain text subject line.
/// - `body`: plain-text body (UTF-8).
/// - `attachments`: list of filesystem paths to attach. Each becomes a
///   MIME part with `application/octet-stream` content-type and the
///   file's basename as the filename parameter.
///
/// Returns Err if MIME construction fails, msmtp cannot be spawned, or
/// msmtp exits non-zero. msmtp's stderr is included in the error.
pub fn send_mail(
    from: &str,
    to: &str,
    subject: &str,
    body: &str,
    attachments: &[&Path],
) -> Result<()> {
    if from.is_empty() {
        bail!("from address is empty");
    }
    if to.is_empty() {
        bail!("to address is empty");
    }

    let from_addr = from.parse().context("parsing From address")?;
    let to_addr = to.parse().context("parsing To address")?;

    let builder = Message::builder()
        .from(from_addr)
        .to(to_addr)
        .subject(subject);

    let body_part = SinglePart::builder()
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_owned());

    let email = if attachments.is_empty() {
        builder.singlepart(body_part)
    } else {
        let mut multipart = MultiPart::mixed().singlepart(body_part);
        for path in attachments {
            let contents = std::fs::read(path)
                .with_context(|| format!("reading attachment {}", path.display()))?;
            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("attachment")
                .to_owned();
            multipart = multipart.singlepart(
                Attachment::new(filename)
                    .body(contents, ContentType::parse("application/octet-stream").unwrap()),
            );
        }
        builder.multipart(multipart)
    }
    .context("building MIME message")?;

    let mime_bytes = email.formatted();

    let mut child = Command::new("msmtp")
        .args(["--account=gmail", "--read-recipients", "--read-envelope-from"])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning msmtp (is it installed? `brew install msmtp` on macOS, `pacman -S msmtp` on Arch)")?;

    child
        .stdin
        .as_mut()
        .expect("piped stdin")
        .write_all(&mime_bytes)
        .context("writing MIME to msmtp stdin")?;

    let output = child.wait_with_output().context("waiting for msmtp")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("msmtp failed (exit {}): {}", output.status, stderr.trim());
    }
    Ok(())
}
