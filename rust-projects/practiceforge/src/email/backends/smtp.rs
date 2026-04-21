//! SMTP backend — sends via `lettre` over STARTTLS (587) or implicit TLS (465).
//!
//! Auth modes supported:
//! - `AUTH PLAIN` / `AUTH LOGIN` with a password (legacy mode — Gmail with
//!   app-password, Exchange on-prem, any generic host).
//! - `AUTH XOAUTH2` with an OAuth2 access token (Gmail modern, any tenant
//!   that has enabled SMTP AUTH).
//!
//! The choice between auth modes is driven by the `AuthMode` variant in
//! [`SmtpConfig`]. Both take a [`TokenSource`] — what differs is how the
//! returned string is handed to `lettre` (as password vs as XOAUTH2 token).
//!
//! Ported from `email::legacy::send_email_with_password` in Phase 1.

use anyhow::{Context, Result};
use std::sync::Arc;

use lettre::message::{
    header::ContentType, Attachment as LettreAttachment, Mailbox as LettreMailbox,
    MultiPart, SinglePart,
};
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::SmtpTransport as LettreSmtpTransport;
use lettre::{Message, Transport};

use crate::email::{Body, Envelope, MailTransport, Mailbox, TokenSource};

/// Encryption posture for the SMTP connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encryption {
    /// Implicit TLS from connect (port 465).
    Tls,
    /// Plaintext then upgrade with STARTTLS (port 587).
    StartTls,
}

/// How to authenticate to the SMTP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    Password,
    XOAuth2,
}

/// Configuration for [`SmtpTransport`].
#[derive(Debug, Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub encryption: Encryption,
    pub username: String,
    pub auth_mode: AuthMode,
    // `TokenSource` carried at runtime; config itself is serializable, so
    // the live transport holds the TokenSource separately (see SmtpTransport::new).
}

pub struct SmtpTransport {
    config: SmtpConfig,
    token_source: Arc<dyn TokenSource>,
    display_name: String,
}

impl SmtpTransport {
    pub fn new(config: SmtpConfig, token_source: Arc<dyn TokenSource>) -> Self {
        let display_name = format!("SMTP ({}:{})", config.host, config.port);
        Self { config, token_source, display_name }
    }
}

/// Convert our vendor-neutral [`Mailbox`] into lettre's.
///
/// Goes through `str::parse` because lettre's `Mailbox::new` requires an
/// already-parsed `Address`, and parsing `"Name <addr>"` / bare `"addr"` is
/// exactly what lettre's `FromStr` impl does. This matches the legacy code.
fn to_lettre_mailbox(mb: &Mailbox) -> Result<LettreMailbox> {
    mb.display()
        .parse::<LettreMailbox>()
        .with_context(|| format!("Invalid email address: {}", mb.display()))
}

/// Build a `lettre::Message` from our vendor-neutral `Envelope`.
///
/// Factored out of `send()` so MIME construction is testable without opening
/// an SMTP connection.
pub(crate) fn envelope_to_message(env: &Envelope) -> Result<Message> {
    let from = to_lettre_mailbox(&env.from)?;

    let mut builder = Message::builder().from(from).subject(&env.subject);

    for to in &env.to {
        builder = builder.to(to_lettre_mailbox(to)?);
    }
    for cc in &env.cc {
        builder = builder.cc(to_lettre_mailbox(cc)?);
    }
    for bcc in &env.bcc {
        builder = builder.bcc(to_lettre_mailbox(bcc)?);
    }

    // ---- Body construction ----------------------------------------------
    //
    // Legacy shape:
    //   - plain text, no attachment        → Message::body(String)
    //   - plain text, with attachment(s)   → multipart/mixed [text, attach...]
    //   - html (no attachment path)        → singlepart TEXT_HTML
    //
    // New shape adds `Body::Multi { text, html }` → multipart/alternative,
    // and attachments always wrap whatever body we built in multipart/mixed.

    let body_part: MultiPart = match &env.body {
        Body::Text(s) => MultiPart::mixed().singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_PLAIN)
                .body(s.clone()),
        ),
        Body::Html(s) => MultiPart::mixed().singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_HTML)
                .body(s.clone()),
        ),
        Body::Multi { text, html } => {
            let alt = MultiPart::alternative()
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_PLAIN)
                        .body(text.clone()),
                )
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_HTML)
                        .body(html.clone()),
                );
            MultiPart::mixed().multipart(alt)
        }
    };

    // No attachments: prefer a non-multipart shape for text/html bodies so
    // small messages don't drag an unnecessary MIME envelope. This matches
    // legacy behaviour exactly.
    let message = if env.attachments.is_empty() {
        match &env.body {
            Body::Text(s) => builder
                .body(s.clone())
                .context("Failed to build text email message")?,
            Body::Html(s) => builder
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_HTML)
                        .body(s.clone()),
                )
                .context("Failed to build HTML email message")?,
            Body::Multi { text, html } => {
                let alt = MultiPart::alternative()
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(text.clone()),
                    )
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(html.clone()),
                    );
                builder
                    .multipart(alt)
                    .context("Failed to build multipart/alternative message")?
            }
        }
    } else {
        // With attachments: wrap body in multipart/mixed and append each
        // attachment as a singlepart.
        let mut mixed = body_part;
        for att in &env.attachments {
            let content_type = ContentType::parse(&att.content_type)
                .with_context(|| format!("Invalid content type: {}", att.content_type))?;
            mixed = mixed.singlepart(
                LettreAttachment::new(att.filename.clone()).body(att.data.clone(), content_type),
            );
        }
        builder
            .multipart(mixed)
            .context("Failed to build multipart email with attachments")?
    };

    Ok(message)
}

impl MailTransport for SmtpTransport {
    fn send(&self, envelope: &Envelope) -> Result<()> {
        let message = envelope_to_message(envelope)?;

        let secret = self
            .token_source
            .access_token()
            .context("Failed to obtain SMTP credential from TokenSource")?;

        // Credentials:
        //   - Password mode: (username, password)
        //   - XOAuth2 mode:  (email-address-as-user, access-token). lettre
        //     formats the SASL string internally — we just pick the
        //     mechanism. Verified against lettre 0.11 authentication.rs:
        //     XOAUTH2 produces `user=<user>\x01auth=Bearer <secret>\x01\x01`.
        let creds = Credentials::new(self.config.username.clone(), secret);

        let builder = match self.config.encryption {
            Encryption::Tls => LettreSmtpTransport::relay(&self.config.host)
                .context("Failed to create implicit-TLS SMTP transport")?,
            Encryption::StartTls => LettreSmtpTransport::starttls_relay(&self.config.host)
                .context("Failed to create STARTTLS SMTP transport")?,
        };

        let builder = builder.port(self.config.port).credentials(creds);

        let builder = match self.config.auth_mode {
            AuthMode::Password => builder, // lettre's default mechanism list is fine
            AuthMode::XOAuth2 => builder.authentication(vec![Mechanism::Xoauth2]),
        };

        let transport = builder.build();

        transport
            .send(&message)
            .with_context(|| format!("SMTP send failed via {}:{}", self.config.host, self.config.port))?;

        Ok(())
    }

    fn name(&self) -> &str {
        &self.display_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::email::{Attachment, Body, Envelope, Mailbox};

    #[test]
    fn mailbox_display_named_vs_bare() {
        let named = Mailbox::with_name("a@b.com", "Alice");
        assert_eq!(named.display(), "Alice <a@b.com>");

        let bare = Mailbox::new("a@b.com");
        assert_eq!(bare.display(), "a@b.com");
    }

    #[test]
    fn envelope_builder_sets_all_optional_fields() {
        let env = Envelope::builder(Mailbox::with_name("from@x.com", "From"))
            .to(Mailbox::with_name("to@y.com", "To"))
            .cc(Mailbox::new("cc@z.com"))
            .bcc(Mailbox::new("bcc@z.com"))
            .subject("Hello")
            .body(Body::Text("World".into()))
            .attachment(Attachment {
                filename: "f.pdf".into(),
                content_type: "application/pdf".into(),
                data: vec![1, 2, 3],
            })
            .build();

        assert_eq!(env.from.email, "from@x.com");
        assert_eq!(env.to.len(), 1);
        assert_eq!(env.to[0].name.as_deref(), Some("To"));
        assert_eq!(env.cc.len(), 1);
        assert_eq!(env.bcc.len(), 1);
        assert_eq!(env.subject, "Hello");
        assert!(matches!(env.body, Body::Text(ref s) if s == "World"));
        assert_eq!(env.attachments.len(), 1);
        assert_eq!(env.attachments[0].filename, "f.pdf");
    }

    fn render(message: &Message) -> String {
        String::from_utf8(message.formatted())
            .expect("lettre message should be valid UTF-8 for ASCII bodies")
    }

    #[test]
    fn text_body_no_attachment_renders_plain_headers() {
        let env = Envelope::builder(Mailbox::with_name("from@x.com", "Alice"))
            .to(Mailbox::with_name("to@y.com", "Bob"))
            .subject("Greetings")
            .body(Body::Text("hello world".into()))
            .build();

        let msg = envelope_to_message(&env).expect("build message");
        let rendered = render(&msg);

        // lettre may or may not quote display names depending on content;
        // just assert both the name and address land in a From/To header.
        assert!(
            rendered.contains("From:") && rendered.contains("Alice") && rendered.contains("<from@x.com>"),
            "From header missing expected pieces:\n{}",
            rendered
        );
        assert!(
            rendered.contains("To:") && rendered.contains("Bob") && rendered.contains("<to@y.com>"),
            "To header missing expected pieces:\n{}",
            rendered
        );
        assert!(rendered.contains("Subject: Greetings"));
        assert!(rendered.contains("hello world"));
    }

    #[test]
    fn html_body_sets_html_content_type() {
        let env = Envelope::builder(Mailbox::new("from@x.com"))
            .to(Mailbox::new("to@y.com"))
            .subject("HTML")
            .body(Body::Html("<b>hi</b>".into()))
            .build();

        let msg = envelope_to_message(&env).expect("build message");
        let rendered = render(&msg);

        assert!(
            rendered.contains("text/html"),
            "expected text/html in:\n{}",
            rendered
        );
        assert!(rendered.contains("<b>hi</b>"));
    }

    #[test]
    fn multi_body_produces_alternative() {
        let env = Envelope::builder(Mailbox::new("from@x.com"))
            .to(Mailbox::new("to@y.com"))
            .subject("Both")
            .body(Body::Multi {
                text: "plain".into(),
                html: "<b>rich</b>".into(),
            })
            .build();

        let msg = envelope_to_message(&env).expect("build message");
        let rendered = render(&msg);

        assert!(rendered.contains("multipart/alternative"));
        assert!(rendered.contains("plain"));
        assert!(rendered.contains("<b>rich</b>"));
    }

    #[test]
    fn attachment_produces_multipart_mixed() {
        let env = Envelope::builder(Mailbox::new("from@x.com"))
            .to(Mailbox::new("to@y.com"))
            .subject("File")
            .body(Body::Text("see attached".into()))
            .attachment(Attachment {
                filename: "report.pdf".into(),
                content_type: "application/pdf".into(),
                data: b"%PDF-1.4 fake".to_vec(),
            })
            .build();

        let msg = envelope_to_message(&env).expect("build message");
        let rendered = render(&msg);

        assert!(rendered.contains("multipart/mixed"));
        assert!(rendered.contains("application/pdf"));
        assert!(rendered.contains("report.pdf"));
        assert!(rendered.contains("see attached"));
    }

    #[test]
    fn cc_and_bcc_headers_are_emitted() {
        let env = Envelope::builder(Mailbox::new("from@x.com"))
            .to(Mailbox::new("to@y.com"))
            .cc(Mailbox::new("cc1@z.com"))
            .cc(Mailbox::new("cc2@z.com"))
            .bcc(Mailbox::new("bcc@z.com"))
            .subject("CC test")
            .body(Body::Text("body".into()))
            .build();

        let msg = envelope_to_message(&env).expect("build message");
        let rendered = render(&msg);

        assert!(rendered.contains("cc1@z.com"));
        assert!(rendered.contains("cc2@z.com"));
        // Bcc is an envelope-level header in lettre's formatted output:
        assert!(rendered.contains("bcc@z.com"));
    }

    #[test]
    fn invalid_address_surfaces_error() {
        let env = Envelope::builder(Mailbox::new("not-an-email"))
            .to(Mailbox::new("to@y.com"))
            .subject("x")
            .body(Body::Text("x".into()))
            .build();

        let result = envelope_to_message(&env);
        assert!(result.is_err());
    }
}
