//! Transport contract — every backend (SMTP, Graph, future) implements
//! [`MailTransport`]. The types in this file form the vendor-neutral
//! message shape.

use anyhow::Result;

/// Sender or recipient. `name` is optional (bare address if `None`).
#[derive(Debug, Clone)]
pub struct Mailbox {
    pub email: String,
    pub name: Option<String>,
}

impl Mailbox {
    pub fn new(email: impl Into<String>) -> Self {
        Self { email: email.into(), name: None }
    }

    pub fn with_name(email: impl Into<String>, name: impl Into<String>) -> Self {
        Self { email: email.into(), name: Some(name.into()) }
    }

    /// RFC 5322 display form: `Name <addr>` if named, else bare `addr`.
    pub fn display(&self) -> String {
        match &self.name {
            Some(n) => format!("{} <{}>", n, self.email),
            None => self.email.clone(),
        }
    }
}

/// Attachment content held in memory. Fine for clinical-scale messages
/// (letters, invoices); a streaming variant can be added later if bulk
/// attachment sends emerge as a use case.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

/// Body shape — text only, HTML only, or both (for clients that prefer
/// one or the other). Invoice emails want HTML; attendance reports want text.
#[derive(Debug, Clone)]
pub enum Body {
    Text(String),
    Html(String),
    Multi { text: String, html: String },
}

/// Message ready to send. Backends translate this into whatever wire
/// format they need (MIME for SMTP, JSON for Graph `/me/sendMail`, etc.).
#[derive(Debug, Clone)]
pub struct Envelope {
    pub from: Mailbox,
    pub to: Vec<Mailbox>,
    pub cc: Vec<Mailbox>,
    pub bcc: Vec<Mailbox>,
    pub subject: String,
    pub body: Body,
    pub attachments: Vec<Attachment>,
}

impl Envelope {
    pub fn builder(from: Mailbox) -> EnvelopeBuilder {
        EnvelopeBuilder {
            envelope: Envelope {
                from,
                to: Vec::new(),
                cc: Vec::new(),
                bcc: Vec::new(),
                subject: String::new(),
                body: Body::Text(String::new()),
                attachments: Vec::new(),
            },
        }
    }
}

/// Fluent builder for `Envelope`. Keeps call sites readable when a message
/// has many optional parts.
pub struct EnvelopeBuilder {
    envelope: Envelope,
}

impl EnvelopeBuilder {
    pub fn to(mut self, m: Mailbox) -> Self { self.envelope.to.push(m); self }
    pub fn cc(mut self, m: Mailbox) -> Self { self.envelope.cc.push(m); self }
    pub fn bcc(mut self, m: Mailbox) -> Self { self.envelope.bcc.push(m); self }
    pub fn subject(mut self, s: impl Into<String>) -> Self { self.envelope.subject = s.into(); self }
    pub fn body(mut self, b: Body) -> Self { self.envelope.body = b; self }
    pub fn attachment(mut self, a: Attachment) -> Self { self.envelope.attachments.push(a); self }
    pub fn build(self) -> Envelope { self.envelope }
}

/// Every mail backend implements this. Keep the surface minimal: the trait
/// should not leak backend-specific concepts (SMTP response codes, Graph
/// JSON errors, etc.). Map those into `anyhow::Error` with clear context.
pub trait MailTransport: Send + Sync {
    /// Send a single message. Returns `Ok(())` on successful submission to
    /// the upstream (SMTP `250 OK`, Graph `202 Accepted`, etc.). Returns
    /// `Err` with context for any failure — auth, network, malformed input.
    fn send(&self, envelope: &Envelope) -> Result<()>;

    /// Short human-readable name for diagnostics and wizard UX
    /// (e.g. `"SMTP (smtp.gmail.com:465)"`, `"Microsoft Graph"`).
    fn name(&self) -> &str;

    /// Optional proof-of-life check without sending mail. Backends that can
    /// cheaply verify (IMAP `NOOP`, Graph `GET /me`) should override.
    /// Default is `Ok(())` — we assume healthy unless proven otherwise.
    fn health_check(&self) -> Result<()> {
        Ok(())
    }
}
