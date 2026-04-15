//! Core data types for the PracticeForge scheduling system.

use chrono::{NaiveDate, NaiveTime};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A single appointment instance — materialised from a series or created one-off.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Appointment {
    pub id: Uuid,
    /// Links to the RecurringSeries that generated this. None for one-offs.
    pub series_id: Option<Uuid>,
    pub practitioner: String,
    pub client_id: String,
    pub client_name: String,
    pub date: NaiveDate,
    pub start_time: NaiveTime,
    pub end_time: NaiveTime,
    pub status: AppointmentStatus,
    pub source: AppointmentSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_tag: Option<String>,
    pub location: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sms_confirmation: Option<SmsConfirmation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppointmentStatus {
    Tentative,
    Confirmed,
    Arrived,
    Completed,
    Cancelled,
    NoShow,
    LateCancellation,
}

impl std::fmt::Display for AppointmentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tentative => write!(f, "tentative"),
            Self::Confirmed => write!(f, "confirmed"),
            Self::Arrived => write!(f, "arrived"),
            Self::Completed => write!(f, "completed"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::NoShow => write!(f, "no-show"),
            Self::LateCancellation => write!(f, "late-cancellation"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppointmentSource {
    Practitioner,
    Admin,
    SelfBooked,
    Migration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmsConfirmation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replied_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
    pub confirmed: bool,
}

/// A recurring appointment series with RRULE semantics.
///
/// The series definition is the source of truth. Individual appointment
/// instances are materialised on-the-fly for a bounded date window —
/// infinite series are never fully expanded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecurringSeries {
    pub id: Uuid,
    pub practitioner: String,
    pub client_id: String,
    pub client_name: String,
    pub start_time: NaiveTime,
    pub end_time: NaiveTime,
    pub location: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_tag: Option<String>,
    pub recurrence: RecurrenceRule,
    /// Specific dates to skip (holidays, leave, one-off cancellations).
    #[serde(default)]
    pub exdates: Vec<NaiveDate>,
    pub status: SeriesStatus,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecurrenceRule {
    pub freq: Frequency,
    /// 1 = every occurrence, 2 = alternate, 3 = every third, etc.
    pub interval: u32,
    /// Constrain to specific weekdays (e.g. only Thursdays).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_day: Option<Vec<Weekday>>,
    /// First occurrence date. The series starts here.
    pub dtstart: NaiveDate,
    /// End date. None = infinite recurrence (the TM3 killer feature).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<NaiveDate>,
    /// Fixed number of occurrences. None = no limit.
    /// Mutually exclusive with `until` per RFC 5545.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Frequency {
    Weekly,
    Monthly,
}

impl std::fmt::Display for Frequency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Weekly => write!(f, "weekly"),
            Self::Monthly => write!(f, "monthly"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SeriesStatus {
    Active,
    Paused,
    Ended,
}

impl std::fmt::Display for SeriesStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Paused => write!(f, "paused"),
            Self::Ended => write!(f, "ended"),
        }
    }
}

/// Insurance authorisation block — tracks session limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorisationBlock {
    pub id: Uuid,
    pub client_id: String,
    pub insurer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_number: Option<String>,
    pub authorised_sessions: u32,
    pub used_sessions: u32,
    pub start_date: NaiveDate,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_date: Option<NaiveDate>,
    pub status: BlockStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorisation_ref: Option<String>,
}

impl AuthorisationBlock {
    pub fn remaining(&self) -> u32 {
        self.authorised_sessions.saturating_sub(self.used_sessions)
    }

    pub fn is_expiring(&self, threshold: u32) -> bool {
        self.status == BlockStatus::Active && self.remaining() <= threshold
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockStatus {
    Active,
    Expiring,
    Exhausted,
    Expired,
}

impl std::fmt::Display for BlockStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Expiring => write!(f, "expiring"),
            Self::Exhausted => write!(f, "exhausted"),
            Self::Expired => write!(f, "expired"),
        }
    }
}

/// Warning emitted when a block is approaching exhaustion.
#[derive(Debug, Clone)]
pub struct BlockExpiryWarning {
    pub client_id: String,
    pub insurer: String,
    pub remaining: u32,
    pub authorised: u32,
    pub message: String,
}
