//! Static account configuration.
//!
//! Two accounts hard-coded for William's stack: `personal` (Gmail / msmtp)
//! and `cohs` (Microsoft 365 / graph-send). Adding a third account = three
//! lines (entry in [`ACCOUNTS`], possibly a new [`SendBackend`] variant if
//! the transport differs).
//!
//! This module deliberately doesn't read from `~/.config/meli/config.toml`
//! or any other live config file: mailpost's account list is small, stable,
//! and changes infrequently enough that compile-time encoding beats a config
//! parse. If/when this grows beyond ~5 accounts or the install is
//! distributed to colleagues, lift to a config-driven approach (TOML next
//! to mailpost's other state).
//!
//! Cross-references:
//! - notmuch tag prefix: each account's messages carry a `tag:<prefix>` so
//!   queries can be partitioned cleanly. `personal` uses absence of `tag:cohs`
//!   (consistent with the existing meli config's workspace mailboxes).
//! - identity: the `From:` address used when composing. Picked up by msmtp's
//!   `--read-envelope-from` flag for Gmail; passed verbatim into MIME for
//!   graph-send (which uses the M365-tenant token's authenticated identity
//!   regardless of the From header value).
//! - send backend: which subprocess receives the lettre-built MIME bytes
//!   on stdin. See [`SendBackend`].

/// Send backend per account. Each variant maps to a subprocess command in
/// `compose::send_post`. The MIME bytes are piped to stdin.
///
/// `Msmtp { account: "gmail" }` runs:
///   `msmtp --account=gmail --read-recipients --read-envelope-from`
///
/// `GraphSend` runs:
///   `graph-send`  (no args; reads MIME from stdin, posts to Graph
///   `/me/sendMail`, auth via pizauth `cohs-graph`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendBackend {
    /// Pipe MIME to msmtp with the named account. Pizauth manages OAuth.
    Msmtp { account: &'static str },
    /// Pipe MIME to graph-send (Microsoft Graph /me/sendMail). Pizauth
    /// manages OAuth via the `cohs-graph` account.
    GraphSend,
}

/// One configured account.
#[derive(Debug, Clone)]
pub struct Account {
    /// URL-safe slug. Appears in `/mail/<slug>/<mailbox>` paths and in
    /// the sidebar.
    pub slug: &'static str,
    /// Display name in the sidebar header.
    pub display_name: &'static str,
    /// RFC5322 From address used when composing from this account.
    pub identity: &'static str,
    /// notmuch tag that gates this account's messages. Empty string
    /// means "no tag gate" (the catch-all for the personal Gmail
    /// account that pre-dates the cohs introduction).
    pub tag_gate: &'static str,
    /// How to send mail from this account.
    pub send: SendBackend,
}

/// All configured accounts. Lookup by slug via [`find`].
pub const ACCOUNTS: &[Account] = &[
    Account {
        slug: "personal",
        display_name: "William Napier",
        identity: "will@willnapier.com",
        // No tag_gate: the personal Gmail messages don't carry a `personal`
        // tag — they're identified by the absence of `tag:cohs`. Listing
        // queries handle this asymmetry in `notmuch_db::mailbox_query`.
        tag_gate: "",
        send: SendBackend::Msmtp { account: "gmail" },
    },
    Account {
        slug: "cohs",
        display_name: "Will Napier (COHS)",
        identity: "will.napier@changeofharleystreet.com",
        tag_gate: "cohs",
        send: SendBackend::GraphSend,
    },
];

/// Look up an account by slug. Returns None for unknown slugs.
///
/// Handlers should 404 (or redirect to a default mailbox) on None rather
/// than panicking — the URL might be a stale bookmark.
pub fn find(slug: &str) -> Option<&'static Account> {
    ACCOUNTS.iter().find(|a| a.slug == slug)
}

/// Default account if the URL omits one. The first entry in [`ACCOUNTS`].
pub fn default_account() -> &'static Account {
    &ACCOUNTS[0]
}
