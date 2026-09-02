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
//! the corrected rules would change without writing, as the watchers would
//! leave it (their size/link-count skips honoured); [`reconcile`] (0.2.3)
//! applies that set under the watched root only, with same-directory atomic
//! replacements, and refuses `--apply` while the service lock is live.
//! 0.2.4: one graph on every host — Syncthing conflict copies are not notes
//! (each host keeps its own, unsynced), and link names resolve NFC-normalised
//! (macOS hands back NFD file names, Linux NFC).

pub mod audit;
pub mod backlinks;
pub mod heartbeat;
pub mod logger;
pub mod reconcile;
pub mod resolve;
pub mod watch;
pub mod wiki;
