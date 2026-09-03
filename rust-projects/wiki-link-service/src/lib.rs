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
//! (macOS hands back NFD file names, Linux NFC); 0.2.5 renders section entries
//! NFC so both hosts write identical bytes. 0.2.6 moves that normalisation to
//! the boundary where a path becomes a name (`wiki::note_name`, `wiki::rel_key`)
//! so every derived key is NFC by construction, matches link *text* in either
//! spelling (`wiki::name_pattern`), resolves the watched root once and fails
//! when it is missing (`reconcile::watched_root`), and reports canonically
//! equivalent duplicate file names in `audit`. 0.2.7 moves those helpers to
//! the shared `forge-names` crate (the roll-out map `meta-nfc-boundary-rollout`)
//! and adds `resolve <name>` / `link-for <path>` so scripts stop resolving
//! names by byte-matching `fd` output. 0.2.8 adds `backlinks`, `rename`, `new`
//! and `promote` ([`ops`]) so the last Nushell note commands that byte-matched
//! file names become thin wrappers over the service.

pub mod audit;
pub mod backlinks;
pub mod heartbeat;
pub mod logger;
pub mod ops;
pub mod reconcile;
pub mod resolve;
pub mod watch;
pub mod wiki;
