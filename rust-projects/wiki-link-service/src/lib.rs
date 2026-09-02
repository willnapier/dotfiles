//! wiki-link-service — Rust port of the two Nushell wiki-link watchers and
//! their `link-service` supervisor:
//!
//! * `scripts/wiki-backlinks`    → [`backlinks`]  (maintains `## Backlinks` sections)
//! * `scripts/wiki-resolve-mark` → [`resolve`]    (marks `?[[target]]` for missing targets)
//! * `scripts/link-service`      → `start`/`status`/`stop`/`audit`/`reconcile` in `main.rs`
//!
//! 0.1.0 was a byte-for-byte replica of the oracles including nine of their
//! bugs. 0.2.0 (2026-09-02) fixes those nine per Will's spec — see the
//! `SPEC` notes in each module — and keeps byte-identical parity with the
//! oracles for everything else (`tests/parity.rs`). [`audit`] reports what
//! the corrected rules would change without writing; [`reconcile`] applies
//! exactly that write set with same-directory atomic replacements.

pub mod audit;
pub mod backlinks;
pub mod heartbeat;
pub mod logger;
pub mod reconcile;
pub mod resolve;
pub mod watch;
pub mod wiki;
