//! Microsoft Graph backend — sends via `POST /v1.0/me/sendMail`.
//!
//! For M365 tenants where `SmtpClientAuthenticationDisabled = $true`
//! blocks SMTP submission entirely, Graph is the alternative: same OAuth
//! token (with `Mail.Send` scope added), HTTP POST of a JSON message
//! object containing the MIME body, and Exchange handles submission.
//!
//! The COHS (Change of Harley Street) tenant is our first such case.
//! See `~/.claude/projects/.../project_cohs_m365_auth.md` for background.
//!
//! Phase 0 stub.

use anyhow::Result;
use std::sync::Arc;

use crate::email::{Envelope, MailTransport, TokenSource};

/// Configuration for [`GraphTransport`].
///
/// Uses the `/common` endpoint by default so any M365 tenant works with
/// the same binary. Override via `tenant` for single-tenant apps if needed.
#[derive(Debug, Clone)]
pub struct GraphConfig {
    /// Base URL — `https://graph.microsoft.com/v1.0` by default.
    pub base_url: String,
    /// Save a copy to Sent Items (usually yes).
    pub save_to_sent_items: bool,
}

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            base_url: "https://graph.microsoft.com/v1.0".to_string(),
            save_to_sent_items: true,
        }
    }
}

pub struct GraphTransport {
    config: GraphConfig,
    token_source: Arc<dyn TokenSource>,
}

impl GraphTransport {
    pub fn new(config: GraphConfig, token_source: Arc<dyn TokenSource>) -> Self {
        Self { config, token_source }
    }
}

impl MailTransport for GraphTransport {
    fn send(&self, _envelope: &Envelope) -> Result<()> {
        // Phase 2: implement.
        //
        // 1. Build MIME body from Envelope (reuse legacy MIME builder or pull from lettre).
        // 2. Base64-encode MIME.
        // 3. POST to {base_url}/me/sendMail with:
        //    Authorization: Bearer <token_source.access_token()>
        //    Content-Type: application/json
        //    Body: {"message": {...MIME as JSON...}, "saveToSentItems": bool}
        //    OR: send raw MIME via the "Outlook sendMail" variant with Content-Type text/plain.
        // 4. Expect 202 Accepted. Any other status → error with body context.
        let _ = (&self.config, &self.token_source);
        todo!("Phase 2: implement Graph /me/sendMail POST")
    }

    fn name(&self) -> &str {
        "Microsoft Graph"
    }
}
