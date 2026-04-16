//! Search module — Tantivy full-text search across client files.
//!
//! Indexes identity.yaml, notes.md, and correspondence/ for every client
//! directory. Uses the registry when available, falls back to scanning
//! `~/Clinical/clients/` directly.

pub mod config;
pub mod index;
pub mod query;
#[cfg(test)]
mod tests;

pub use config::SearchConfig;
pub use query::SearchResult;
