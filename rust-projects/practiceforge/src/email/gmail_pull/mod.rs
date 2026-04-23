//! Gmail pull — pure-Rust replacement for `lieer`'s `gmi pull`.
//!
//! This module family replaces the Python lieer dependency for the
//! pull half of our Gmail sync stack (the push half is
//! [`crate::email::gmail_push_tags`]).
//!
//! ## Build order
//!
//! - **Leg 1 (this file + [`api`])**: raw-HTTP Gmail API client. No
//!   filesystem writes, no state. Proves the wire protocol works.
//! - **Leg 2** (planned): `maildir` + `state` + `pull` modules —
//!   initial full-mirror sync. One file per message, lieer-compatible
//!   naming, resume on interrupt.
//! - **Leg 3** (planned): `history` module — delta sync via Gmail's
//!   history API, with full-resync fallback on stale historyId.
//! - **Leg 4** (planned): `tags` module — apply Gmail labels as
//!   notmuch tags on pull, mirroring the tag→label push path.
//! - **Leg 5** (cutover): user-facing `gmail-pull` subcommand,
//!   validation parallel-run against lieer, then retire lieer.
//!
//! Full plan: `~/Assistants/shared/practiceforge/gmail-rust-replacement-plan-2026-04-22.md`.

pub mod api;

pub use api::{
    BatchMessage, GmailApi, GmailApiError, HistoryResponse, Profile,
};
