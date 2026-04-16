//! Practitioner availability — per-day working hours, breaks, and slot-finding.
//!
//! Each practitioner has an `availability.yaml` in their schedules directory.
//! The slot-finder reads this + existing appointments to produce ranked
//! available slots for reschedules or new bookings.

use anyhow::{Context, Result};
use chrono::{Datelike, NaiveDate, NaiveTime};
use serde::{Deserialize, Serialize};
use std::path::Path;

use super::models::{Appointment, RecurringSeries, SessionModality, SeriesStatus};
use super::recurrence;

// ---------------------------------------------------------------------------
// Data model — stored in availability.yaml per practitioner
// ---------------------------------------------------------------------------

/// Complete practitioner availability configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PractitionerAvailability {
    pub practitioner: String,
    pub days: DaySchedules,
    #[serde(default)]
    pub reschedule: RescheduleConfig,
    #[serde(default)]
    pub preferences: SlotPreferences,
}

/// Per-day schedule definitions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaySchedules {
    #[serde(default)]
    pub monday: Option<DayConfig>,
    #[serde(default)]
    pub tuesday: Option<DayConfig>,
    #[serde(default)]
    pub wednesday: Option<DayConfig>,
    #[serde(default)]
    pub thursday: Option<DayConfig>,
    #[serde(default)]
    pub friday: Option<DayConfig>,
}

/// Configuration for a single day.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayConfig {
    /// Default modality for this day.
    pub default_modality: Modality,
    /// Earliest session start time.
    pub start: String,
    /// Latest session *end* time (sessions must finish by this, not start).
    pub end: String,
    /// Whether this day is available for reschedules.
    #[serde(default = "default_true")]
    pub reschedule_eligible: bool,
    /// Different end time for reschedules (e.g., Tuesday regulars till 20:30
    /// but reschedules only till 18:00).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reschedule_end: Option<String>,
    /// Soft-stop time — prefer to finish by this, offer later only as fallback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soft_stop: Option<String>,
    /// Non-negotiable breaks.
    #[serde(default)]
    pub breaks: Vec<TimeBlock>,
    /// Whether to protect the travel gap after initial remote sessions.
    #[serde(default)]
    pub protect_travel_gap: bool,
    /// Minimum travel gap duration in minutes.
    #[serde(default = "default_travel_gap")]
    pub travel_gap_minutes: u32,
}

/// A time block (break, protected period).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeBlock {
    pub start: String,
    pub end: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Modality {
    Remote,
    InPerson,
}

impl std::fmt::Display for Modality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Modality::Remote => write!(f, "remote"),
            Modality::InPerson => write!(f, "in-person"),
        }
    }
}

/// Reschedule-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RescheduleConfig {
    /// Hours from original appointment time to search for slots.
    #[serde(default = "default_48")]
    pub window_hours: u32,
    /// Day preference order (first = most preferred).
    #[serde(default)]
    pub preferred_days: Vec<String>,
}

impl Default for RescheduleConfig {
    fn default() -> Self {
        Self {
            window_hours: 48,
            preferred_days: vec![
                "tuesday".into(),
                "wednesday".into(),
                "thursday".into(),
                "friday".into(),
            ],
        }
    }
}

/// Slot ranking preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlotPreferences {
    /// Prefer contiguous placement (adjacent to existing sessions).
    #[serde(default = "default_true")]
    pub prefer_contiguous: bool,
    /// Prefer slots within the existing day span over extending the day.
    #[serde(default = "default_true")]
    pub minimise_day_span: bool,
}

impl Default for SlotPreferences {
    fn default() -> Self {
        Self {
            prefer_contiguous: true,
            minimise_day_span: true,
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_48() -> u32 {
    48
}
fn default_travel_gap() -> u32 {
    45
}

// ---------------------------------------------------------------------------
// Slot finding
// ---------------------------------------------------------------------------

/// A candidate slot returned by the slot-finder.
#[derive(Debug, Clone)]
pub struct AvailableSlot {
    pub date: NaiveDate,
    pub start_time: NaiveTime,
    pub end_time: NaiveTime,
    pub modality: Modality,
    pub day_name: String,
    /// Ranking score (lower = better). Used for sorting.
    pub score: SlotScore,
}

/// Breakdown of the ranking score for a slot.
#[derive(Debug, Clone)]
pub struct SlotScore {
    /// Penalty for extending the working day span (minutes).
    pub span_extension: i64,
    /// Day preference rank (0 = most preferred).
    pub day_rank: u32,
    /// Distance from nearest existing appointment (minutes, lower = more contiguous).
    pub contiguity_gap: i64,
    /// Whether this slot is past a soft-stop time.
    pub past_soft_stop: bool,
    /// Composite score for sorting.
    pub total: i64,
}

/// An occupied time range on a specific date.
#[derive(Debug, Clone)]
struct OccupiedSlot {
    start: NaiveTime,
    end: NaiveTime,
    modality: Option<SessionModality>,
}

/// Load a practitioner's availability from their schedules directory.
pub fn load_availability(practitioner_dir: &Path) -> Result<PractitionerAvailability> {
    let path = practitioner_dir.join("availability.yaml");
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Cannot read {}", path.display()))?;
    let avail: PractitionerAvailability =
        serde_yaml::from_str(&content).context("Failed to parse availability.yaml")?;
    Ok(avail)
}

/// Find available slots for a reschedule.
///
/// Searches from `from` to `from + window_hours` for gaps that fit
/// `duration_minutes`. Returns slots ranked by the preference cascade.
pub fn find_reschedule_slots(
    avail: &PractitionerAvailability,
    series: &[RecurringSeries],
    one_offs: &[Appointment],
    from: chrono::NaiveDateTime,
    duration_minutes: u32,
    holidays: &[NaiveDate],
) -> Vec<AvailableSlot> {
    let window_end = from + chrono::Duration::hours(avail.reschedule.window_hours as i64);
    let start_date = from.date();
    let end_date = window_end.date();

    let mut slots = Vec::new();

    let mut date = start_date;
    while date <= end_date {
        if holidays.contains(&date) {
            date = date.succ_opt().unwrap_or(date);
            continue;
        }

        let day_config = match day_config_for_date(&avail.days, date) {
            Some(dc) => dc,
            None => {
                date = date.succ_opt().unwrap_or(date);
                continue;
            }
        };

        if !day_config.reschedule_eligible {
            date = date.succ_opt().unwrap_or(date);
            continue;
        }

        // Determine the effective end time for reschedules
        let day_start = parse_time(&day_config.start);
        let day_end = day_config
            .reschedule_end
            .as_deref()
            .map(parse_time)
            .unwrap_or_else(|| parse_time(&day_config.end));

        // Collect occupied slots for this date
        let occupied = occupied_slots_for_date(date, series, one_offs, holidays);

        // Collect breaks
        let mut blocked: Vec<(NaiveTime, NaiveTime)> = day_config
            .breaks
            .iter()
            .map(|b| (parse_time(&b.start), parse_time(&b.end)))
            .collect();

        // Travel-gap protection
        if day_config.protect_travel_gap {
            if let Some(gap) =
                detect_travel_gap(&occupied, day_config.travel_gap_minutes)
            {
                blocked.push(gap);
            }
        }

        // Add occupied appointments as blocked
        for occ in &occupied {
            blocked.push((occ.start, occ.end));
        }

        // Sort blocked ranges and find gaps
        blocked.sort_by_key(|&(s, _)| s);

        let gaps = find_gaps(day_start, day_end, &blocked);

        // Filter by start time (must be after `from` on the first day)
        let earliest_start = if date == start_date {
            from.time()
        } else {
            day_start
        };

        // Latest start time (must be before window end on the last day)
        let latest_end = if date == end_date {
            window_end.time()
        } else {
            day_end
        };

        let duration = chrono::Duration::minutes(duration_minutes as i64);

        for (gap_start, gap_end) in &gaps {
            let effective_start = (*gap_start).max(earliest_start);
            let effective_end = (*gap_end).min(latest_end);

            if effective_end - effective_start >= duration {
                // Generate slots aligned to the start of the gap (contiguous preference)
                // and at the end of the gap (also contiguous)
                let mut candidates = Vec::new();

                // Slot at start of gap
                let slot_end_at_start = effective_start + duration;
                if slot_end_at_start <= effective_end {
                    candidates.push((effective_start, slot_end_at_start));
                }

                // Slot at end of gap (adjacent to next appointment)
                let slot_start_at_end = effective_end - duration;
                if slot_start_at_end >= effective_start && slot_start_at_end != effective_start {
                    candidates.push((slot_start_at_end, effective_end));
                }

                for (s_start, s_end) in candidates {
                    let score = score_slot(
                        avail,
                        day_config,
                        date,
                        s_start,
                        s_end,
                        &occupied,
                    );

                    slots.push(AvailableSlot {
                        date,
                        start_time: s_start,
                        end_time: s_end,
                        modality: day_config.default_modality.clone(),
                        day_name: weekday_name(date),
                        score,
                    });
                }
            }
        }

        date = date.succ_opt().unwrap_or(date);
    }

    // Sort by composite score
    slots.sort_by_key(|s| s.score.total);

    // Deduplicate slots with identical start times
    slots.dedup_by(|a, b| a.date == b.date && a.start_time == b.start_time);

    slots
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

fn score_slot(
    avail: &PractitionerAvailability,
    day_config: &DayConfig,
    date: NaiveDate,
    start: NaiveTime,
    end: NaiveTime,
    occupied: &[OccupiedSlot],
) -> SlotScore {
    // 1. Span extension penalty
    let span_extension = if occupied.is_empty() {
        0i64
    } else {
        let existing_start = occupied.iter().map(|o| o.start).min().unwrap();
        let existing_end = occupied.iter().map(|o| o.end).max().unwrap();

        let earlier = if start < existing_start {
            (existing_start - start).num_minutes()
        } else {
            0
        };
        let later = if end > existing_end {
            (end - existing_end).num_minutes()
        } else {
            0
        };
        earlier + later
    };

    // 2. Day preference rank
    let day_name = weekday_name(date).to_lowercase();
    let day_rank = avail
        .reschedule
        .preferred_days
        .iter()
        .position(|d| d.to_lowercase() == day_name)
        .map(|p| p as u32)
        .unwrap_or(99);

    // 3. Contiguity — distance to nearest occupied slot
    let contiguity_gap = if occupied.is_empty() {
        999i64
    } else {
        occupied
            .iter()
            .map(|o| {
                let gap_before = if start >= o.end {
                    (start - o.end).num_minutes()
                } else {
                    i64::MAX
                };
                let gap_after = if o.start >= end {
                    (o.start - end).num_minutes()
                } else {
                    i64::MAX
                };
                gap_before.min(gap_after)
            })
            .min()
            .unwrap_or(999)
    };

    // 4. Past soft stop
    let past_soft_stop = day_config
        .soft_stop
        .as_deref()
        .map(|ss| end > parse_time(ss))
        .unwrap_or(false);

    // Composite: weighted sum
    // span_extension * 100 dominates, then day_rank * 50, then contiguity,
    // then soft_stop penalty, then chronological (date)
    let soft_stop_penalty: i64 = if past_soft_stop { 10000 } else { 0 };
    let date_offset = (date - NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()).num_days();

    let total = span_extension * 100
        + day_rank as i64 * 50
        + contiguity_gap
        + soft_stop_penalty
        + date_offset;

    SlotScore {
        span_extension,
        day_rank,
        contiguity_gap,
        past_soft_stop,
        total,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn day_config_for_date<'a>(days: &'a DaySchedules, date: NaiveDate) -> Option<&'a DayConfig> {
    match date.weekday() {
        chrono::Weekday::Mon => days.monday.as_ref(),
        chrono::Weekday::Tue => days.tuesday.as_ref(),
        chrono::Weekday::Wed => days.wednesday.as_ref(),
        chrono::Weekday::Thu => days.thursday.as_ref(),
        chrono::Weekday::Fri => days.friday.as_ref(),
        _ => None, // weekends
    }
}

fn weekday_name(date: NaiveDate) -> String {
    match date.weekday() {
        chrono::Weekday::Mon => "Monday".to_string(),
        chrono::Weekday::Tue => "Tuesday".to_string(),
        chrono::Weekday::Wed => "Wednesday".to_string(),
        chrono::Weekday::Thu => "Thursday".to_string(),
        chrono::Weekday::Fri => "Friday".to_string(),
        chrono::Weekday::Sat => "Saturday".to_string(),
        chrono::Weekday::Sun => "Sunday".to_string(),
    }
}

fn parse_time(s: &str) -> NaiveTime {
    NaiveTime::parse_from_str(s, "%H:%M").unwrap_or_else(|_| {
        NaiveTime::parse_from_str(s, "%H:%M:%S").unwrap_or(NaiveTime::from_hms_opt(9, 0, 0).unwrap())
    })
}

/// Collect all occupied time ranges for a specific date.
fn occupied_slots_for_date(
    date: NaiveDate,
    series: &[RecurringSeries],
    one_offs: &[Appointment],
    holidays: &[NaiveDate],
) -> Vec<OccupiedSlot> {
    let mut occupied = Vec::new();

    // Materialise recurring series for this single date
    for s in series {
        if s.status != SeriesStatus::Active {
            continue;
        }
        let dates = recurrence::materialise(s, date, date, holidays).unwrap_or_default();
        if dates.contains(&date) {
            occupied.push(OccupiedSlot {
                start: s.start_time,
                end: s.end_time,
                modality: s.modality.clone(),
            });
        }
    }

    // One-off appointments on this date
    for appt in one_offs {
        if appt.date == date
            && appt.status != super::AppointmentStatus::Cancelled
            && appt.status != super::AppointmentStatus::NoShow
        {
            occupied.push(OccupiedSlot {
                start: appt.start_time,
                end: appt.end_time,
                modality: appt.modality.clone(),
            });
        }
    }

    occupied.sort_by_key(|o| o.start);
    occupied
}

/// Detect the travel gap after initial remote sessions.
///
/// If the day starts with one or more remote sessions followed by a gap
/// before the first in-person session, return that gap as a protected range.
fn detect_travel_gap(
    occupied: &[OccupiedSlot],
    min_gap_minutes: u32,
) -> Option<(NaiveTime, NaiveTime)> {
    if occupied.is_empty() {
        return None;
    }

    // Find where remote sessions end and in-person begins.
    // For now, with modality not yet on appointments, this is a no-op.
    // When modality is added, this logic activates.
    let mut last_remote_end: Option<NaiveTime> = None;

    for occ in occupied {
        match &occ.modality {
            Some(SessionModality::Remote) => {
                last_remote_end = Some(occ.end);
            }
            Some(SessionModality::InPerson) => {
                // Found first in-person after remotes
                if let Some(remote_end) = last_remote_end {
                    let gap = (occ.start - remote_end).num_minutes();
                    if gap >= min_gap_minutes as i64 {
                        return Some((remote_end, occ.start));
                    }
                }
                return None; // in-person without preceding remote
            }
            None => {
                // Unknown modality — can't determine, don't protect
                return None;
            }
        }
    }

    None
}

/// Find gaps between blocked ranges within a day window.
fn find_gaps(
    day_start: NaiveTime,
    day_end: NaiveTime,
    blocked: &[(NaiveTime, NaiveTime)],
) -> Vec<(NaiveTime, NaiveTime)> {
    let mut gaps = Vec::new();
    let mut cursor = day_start;

    for &(block_start, block_end) in blocked {
        if block_start > cursor {
            gaps.push((cursor, block_start));
        }
        if block_end > cursor {
            cursor = block_end;
        }
    }

    if cursor < day_end {
        gaps.push((cursor, day_end));
    }

    gaps
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn william_availability() -> PractitionerAvailability {
        PractitionerAvailability {
            practitioner: "will-napier".to_string(),
            days: DaySchedules {
                monday: Some(DayConfig {
                    default_modality: Modality::Remote,
                    start: "09:15".to_string(),
                    end: "17:00".to_string(),
                    reschedule_eligible: false,
                    reschedule_end: None,
                    soft_stop: None,
                    breaks: vec![],
                    protect_travel_gap: false,
                    travel_gap_minutes: 45,
                }),
                tuesday: Some(DayConfig {
                    default_modality: Modality::InPerson,
                    start: "07:45".to_string(),
                    end: "20:30".to_string(),
                    reschedule_eligible: true,
                    reschedule_end: Some("18:00".to_string()),
                    soft_stop: None,
                    breaks: vec![
                        TimeBlock {
                            start: "10:55".to_string(),
                            end: "11:25".to_string(),
                            label: Some("Brunch".to_string()),
                        },
                        TimeBlock {
                            start: "13:50".to_string(),
                            end: "15:50".to_string(),
                            label: Some("Gym".to_string()),
                        },
                    ],
                    protect_travel_gap: true,
                    travel_gap_minutes: 45,
                }),
                wednesday: Some(DayConfig {
                    default_modality: Modality::InPerson,
                    start: "07:45".to_string(),
                    end: "18:00".to_string(),
                    reschedule_eligible: true,
                    reschedule_end: None,
                    soft_stop: None,
                    breaks: vec![],
                    protect_travel_gap: true,
                    travel_gap_minutes: 45,
                }),
                thursday: Some(DayConfig {
                    default_modality: Modality::InPerson,
                    start: "07:45".to_string(),
                    end: "16:45".to_string(),
                    reschedule_eligible: true,
                    reschedule_end: None,
                    soft_stop: None,
                    breaks: vec![],
                    protect_travel_gap: true,
                    travel_gap_minutes: 45,
                }),
                friday: Some(DayConfig {
                    default_modality: Modality::Remote,
                    start: "07:45".to_string(),
                    end: "18:00".to_string(),
                    reschedule_eligible: true,
                    reschedule_end: None,
                    soft_stop: Some("16:45".to_string()),
                    breaks: vec![],
                    protect_travel_gap: false,
                    travel_gap_minutes: 45,
                }),
            },
            reschedule: RescheduleConfig {
                window_hours: 48,
                preferred_days: vec![
                    "tuesday".into(),
                    "wednesday".into(),
                    "thursday".into(),
                    "friday".into(),
                ],
            },
            preferences: SlotPreferences {
                prefer_contiguous: true,
                minimise_day_span: true,
            },
        }
    }

    #[test]
    fn monday_excluded_from_reschedules() {
        let avail = william_availability();
        let monday = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap(); // Monday
        let from = monday.and_hms_opt(9, 0, 0).unwrap();

        let slots = find_reschedule_slots(&avail, &[], &[], from, 45, &[]);

        // No Monday slots should appear
        assert!(
            slots.iter().all(|s| s.day_name != "Monday"),
            "Monday slots should not be offered"
        );
    }

    #[test]
    fn tuesday_breaks_respected() {
        let avail = william_availability();
        let tuesday = NaiveDate::from_ymd_opt(2026, 4, 21).unwrap(); // Tuesday
        let from = tuesday.and_hms_opt(7, 0, 0).unwrap();

        let slots = find_reschedule_slots(&avail, &[], &[], from, 45, &[]);
        let tue_slots: Vec<_> = slots.iter().filter(|s| s.day_name == "Tuesday").collect();

        // No slot should overlap with brunch (10:55-11:25) or gym (13:50-15:50)
        for slot in &tue_slots {
            let brunch_start = parse_time("10:55");
            let brunch_end = parse_time("11:25");
            let gym_start = parse_time("13:50");
            let gym_end = parse_time("15:50");

            let overlaps_brunch =
                slot.start_time < brunch_end && slot.end_time > brunch_start;
            let overlaps_gym = slot.start_time < gym_end && slot.end_time > gym_start;

            assert!(
                !overlaps_brunch,
                "Slot {:?}-{:?} overlaps brunch",
                slot.start_time, slot.end_time
            );
            assert!(
                !overlaps_gym,
                "Slot {:?}-{:?} overlaps gym",
                slot.start_time, slot.end_time
            );
        }
    }

    #[test]
    fn tuesday_reschedule_capped_at_18() {
        let avail = william_availability();
        let tuesday = NaiveDate::from_ymd_opt(2026, 4, 21).unwrap();
        let from = tuesday.and_hms_opt(7, 0, 0).unwrap();

        let slots = find_reschedule_slots(&avail, &[], &[], from, 45, &[]);
        let tue_slots: Vec<_> = slots.iter().filter(|s| s.day_name == "Tuesday").collect();

        for slot in &tue_slots {
            assert!(
                slot.end_time <= parse_time("18:00"),
                "Tuesday reschedule slot ends at {:?}, should be <= 18:00",
                slot.end_time
            );
        }
    }

    #[test]
    fn thursday_hard_stop_respected() {
        let avail = william_availability();
        let thursday = NaiveDate::from_ymd_opt(2026, 4, 23).unwrap();
        let from = thursday.and_hms_opt(7, 0, 0).unwrap();

        let slots = find_reschedule_slots(&avail, &[], &[], from, 45, &[]);
        let thu_slots: Vec<_> = slots.iter().filter(|s| s.day_name == "Thursday").collect();

        for slot in &thu_slots {
            assert!(
                slot.end_time <= parse_time("16:45"),
                "Thursday slot ends at {:?}, must be <= 16:45",
                slot.end_time
            );
        }
    }

    #[test]
    fn friday_soft_stop_penalised() {
        let avail = william_availability();
        let friday = NaiveDate::from_ymd_opt(2026, 4, 24).unwrap();
        let from = friday.and_hms_opt(7, 0, 0).unwrap();

        let slots = find_reschedule_slots(&avail, &[], &[], from, 45, &[]);
        let fri_slots: Vec<_> = slots.iter().filter(|s| s.day_name == "Friday").collect();

        let before_soft: Vec<_> = fri_slots
            .iter()
            .filter(|s| s.end_time <= parse_time("16:45"))
            .collect();
        let after_soft: Vec<_> = fri_slots
            .iter()
            .filter(|s| s.end_time > parse_time("16:45"))
            .collect();

        if !before_soft.is_empty() && !after_soft.is_empty() {
            assert!(
                before_soft[0].score.total < after_soft[0].score.total,
                "Slots before soft stop should score better than after"
            );
        }
    }

    #[test]
    fn preferred_days_ranked_higher() {
        let avail = william_availability();
        // Start on a Monday so Tue, Wed, Thu, Fri are all in range
        let monday = NaiveDate::from_ymd_opt(2026, 4, 20).unwrap();
        let from = monday.and_hms_opt(9, 0, 0).unwrap();

        let slots = find_reschedule_slots(&avail, &[], &[], from, 45, &[]);

        // Find the best Tuesday and best Friday slot
        let best_tue = slots.iter().find(|s| s.day_name == "Tuesday");
        let best_fri = slots.iter().find(|s| s.day_name == "Friday");

        if let (Some(tue), Some(fri)) = (best_tue, best_fri) {
            assert!(
                tue.score.day_rank < fri.score.day_rank,
                "Tuesday (rank {}) should rank higher than Friday (rank {})",
                tue.score.day_rank,
                fri.score.day_rank
            );
        }
    }

    #[test]
    fn find_gaps_basic() {
        let start = parse_time("07:45");
        let end = parse_time("18:00");
        let blocked = vec![
            (parse_time("09:00"), parse_time("10:00")),
            (parse_time("14:00"), parse_time("15:00")),
        ];

        let gaps = find_gaps(start, end, &blocked);
        assert_eq!(gaps.len(), 3);
        assert_eq!(gaps[0], (parse_time("07:45"), parse_time("09:00")));
        assert_eq!(gaps[1], (parse_time("10:00"), parse_time("14:00")));
        assert_eq!(gaps[2], (parse_time("15:00"), parse_time("18:00")));
    }

    #[test]
    fn find_gaps_no_blocked() {
        let start = parse_time("07:45");
        let end = parse_time("18:00");

        let gaps = find_gaps(start, end, &[]);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0], (start, end));
    }

    #[test]
    fn span_minimisation_scoring() {
        let avail = william_availability();
        let wed_config = avail.days.wednesday.as_ref().unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 4, 22).unwrap();

        // Existing appointment 10:00-10:45
        let occupied = vec![OccupiedSlot {
            start: parse_time("10:00"),
            end: parse_time("10:45"),
            modality: Some(SessionModality::InPerson),
        }];

        // Slot within span (10:45-11:30) vs extending earlier (08:00-08:45)
        let within = score_slot(
            &avail,
            wed_config,
            date,
            parse_time("10:45"),
            parse_time("11:30"),
            &occupied,
        );
        let extending = score_slot(
            &avail,
            wed_config,
            date,
            parse_time("08:00"),
            parse_time("08:45"),
            &occupied,
        );

        assert!(
            within.span_extension < extending.span_extension,
            "Within-span slot should have lower span_extension ({}) than extending ({})",
            within.span_extension,
            extending.span_extension
        );
        assert!(
            within.total < extending.total,
            "Within-span slot should score better"
        );
    }

    #[test]
    fn holidays_excluded() {
        let avail = william_availability();
        let tuesday = NaiveDate::from_ymd_opt(2026, 4, 21).unwrap();
        let from = tuesday.and_hms_opt(7, 0, 0).unwrap();

        let holidays = vec![tuesday];
        let slots = find_reschedule_slots(&avail, &[], &[], from, 45, &holidays);

        assert!(
            slots.iter().all(|s| s.date != tuesday),
            "Holiday dates should have no slots"
        );
    }

    #[test]
    fn ninety_minute_fallback_to_45() {
        let avail = william_availability();
        let thursday = NaiveDate::from_ymd_opt(2026, 4, 23).unwrap();
        let from = thursday.and_hms_opt(7, 0, 0).unwrap();

        // Fill Thursday almost completely: 07:45-16:00 occupied
        let occupied_appt = Appointment {
            id: uuid::Uuid::new_v4(),
            series_id: None,
            practitioner: "will-napier".to_string(),
            client_id: "XX99".to_string(),
            client_name: "Filler".to_string(),
            date: thursday,
            start_time: parse_time("07:45"),
            end_time: parse_time("16:00"),
            status: super::super::AppointmentStatus::Confirmed,
            source: super::super::AppointmentSource::Practitioner,
            modality: None,
            rate_tag: None,
            location: "37 Gloucester Place".to_string(),
            reschedule_for: None,
            sms_confirmation: None,
            notes: None,
            created_at: "2026-04-23T00:00:00Z".to_string(),
        };

        // 90-min won't fit (only 45 min gap: 16:00-16:45)
        let slots_90 = find_reschedule_slots(&avail, &[], &[occupied_appt.clone()], from, 90, &[]);
        let thu_90: Vec<_> = slots_90
            .iter()
            .filter(|s| s.date == thursday)
            .collect();
        assert!(thu_90.is_empty(), "No 90-min slot should fit on Thursday");

        // 45-min fits
        let slots_45 = find_reschedule_slots(&avail, &[], &[occupied_appt], from, 45, &[]);
        let thu_45: Vec<_> = slots_45
            .iter()
            .filter(|s| s.date == thursday)
            .collect();
        assert!(!thu_45.is_empty(), "45-min slot should fit on Thursday");
    }
}
