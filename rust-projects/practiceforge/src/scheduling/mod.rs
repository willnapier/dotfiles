//! PracticeForge scheduling module — appointments, recurrence, ICS export.
//!
//! Architecture: follows the billing module pattern (trait-based providers,
//! config-driven, disabled by default).
//!
//! Key feature: infinite recurring sessions with block expiry warnings.
//! TM3 cannot do this — blocks expire silently causing human errors.

pub mod availability;
pub mod config;
pub mod ics;
pub mod models;
pub mod recurrence;

pub use config::SchedulingConfig;
pub use models::{
    Appointment, AppointmentSource, AppointmentStatus, AuthorisationBlock, BlockExpiryWarning,
    BlockStatus, Frequency, RecurrenceRule, RecurringSeries, SeriesStatus, SessionModality,
    Weekday,
};
