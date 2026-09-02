//! wiki-link-service — Rust port (2026-09-02) of the two Nushell wiki-link
//! watchers and their `link-service` supervisor:
//!
//! * `scripts/wiki-backlinks`    → [`backlinks`]  (maintains `## Backlinks` sections)
//! * `scripts/wiki-resolve-mark` → [`resolve`]    (marks `?[[target]]` for missing targets)
//! * `scripts/link-service`      → `start`/`status`/`stop` in `main.rs`
//!
//! The handlers are ports of the oracles' `handle_change` functions and are
//! held to byte-for-byte parity with them by `tests/parity.rs`. Where the
//! oracle's behaviour is a bug it is replicated on purpose and documented at
//! the point of replication (search for `ORACLE BUG`).
//!
//! The Nushell scripts shell out to `rg`, `fd` and `sd`; [`wiki`] emulates the
//! exact semantics of those calls (regex crate, `ignore`-crate walk with
//! hidden/gitignore handling, fd smart-case) instead of spawning them.

pub mod backlinks;
pub mod heartbeat;
pub mod logger;
pub mod resolve;
pub mod watch;
pub mod wiki;
