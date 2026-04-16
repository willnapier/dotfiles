//! TM3 Migration — export data from TM3 into PracticeForge.
//!
//! Orchestrates the full migration: clients, calendar, documents,
//! and validation. Each sub-module can be run independently or
//! as part of a full migration run.

pub mod calendar;
pub mod clients;
pub mod documents;
pub mod validate;

#[cfg(test)]
mod tests;

pub use calendar::CalendarReport;
pub use clients::MigrationReport;
pub use documents::DocReport;
pub use validate::ValidationReport;
