//! Recurrence engine — wraps the `rrule` crate for RFC 5545 RRULE support.
//!
//! Key principle: never materialise an infinite series fully.
//! Always materialise into a bounded [from, to] window.

use anyhow::{Context, Result};
use chrono::{NaiveDate, NaiveTime, TimeZone};
use chrono_tz::Europe::London;
use rrule::{Frequency as RFreq, NWeekday, RRule, RRuleSet, Tz, Unvalidated};

use super::models::{
    AuthorisationBlock, BlockExpiryWarning, Frequency, RecurringSeries, Weekday,
};

/// Materialise occurrences of a series within [from, to].
///
/// Combines the series RRULE with practitioner holidays as EXDATE.
/// For infinite series, only returns dates within the window.
pub fn materialise(
    series: &RecurringSeries,
    from: NaiveDate,
    to: NaiveDate,
    holidays: &[NaiveDate],
) -> Result<Vec<NaiveDate>> {
    let rrule_set = build_rrule_set(series, holidays)?;

    let from_dt = naive_to_tz(from, NaiveTime::from_hms_opt(0, 0, 0).unwrap());
    let to_dt = naive_to_tz(to, NaiveTime::from_hms_opt(23, 59, 59).unwrap());

    let dates: Vec<NaiveDate> = rrule_set
        .after(from_dt)
        .before(to_dt)
        .all(500) // safety limit per expansion
        .dates
        .into_iter()
        .map(|dt| dt.date_naive())
        .collect();

    Ok(dates)
}

/// Check if a block is approaching exhaustion.
/// Returns a warning if remaining sessions <= threshold.
pub fn check_block_expiry(
    block: &AuthorisationBlock,
    threshold: u32,
) -> Option<BlockExpiryWarning> {
    let remaining = block.remaining();
    if remaining > threshold {
        return None;
    }

    let message = if remaining == 0 {
        format!(
            "{} has exhausted all {} sessions authorised by {} — re-authorisation required",
            block.client_id, block.authorised_sessions, block.insurer
        )
    } else {
        format!(
            "{} has {} session{} remaining of {} authorised by {} — time to request re-authorisation?",
            block.client_id,
            remaining,
            if remaining == 1 { "" } else { "s" },
            block.authorised_sessions,
            block.insurer
        )
    };

    Some(BlockExpiryWarning {
        client_id: block.client_id.clone(),
        insurer: block.insurer.clone(),
        remaining,
        authorised: block.authorised_sessions,
        message,
    })
}

fn build_rrule_set(series: &RecurringSeries, holidays: &[NaiveDate]) -> Result<RRuleSet> {
    let dtstart = naive_to_tz(series.recurrence.dtstart, series.start_time);

    let freq = match series.recurrence.freq {
        Frequency::Weekly => RFreq::Weekly,
        Frequency::Monthly => RFreq::Monthly,
    };

    let mut rrule = RRule::<Unvalidated>::new(freq).interval(series.recurrence.interval as u16);

    if let Some(count) = series.recurrence.count {
        rrule = rrule.count(count);
    }

    if let Some(until) = series.recurrence.until {
        let until_dt = naive_to_tz(until, NaiveTime::from_hms_opt(23, 59, 59).unwrap());
        rrule = rrule.until(until_dt);
    }

    if let Some(ref days) = series.recurrence.by_day {
        let nweekdays: Vec<NWeekday> = days.iter().map(weekday_to_nweekday).collect();
        rrule = rrule.by_weekday(nweekdays);
    }

    let validated_rrule = rrule
        .validate(dtstart)
        .context("Invalid recurrence rule")?;

    let mut set = RRuleSet::new(dtstart).rrule(validated_rrule);

    // Add per-series exception dates
    for exdate in &series.exdates {
        let exdate_dt = naive_to_tz(*exdate, series.start_time);
        set = set.exdate(exdate_dt);
    }

    // Add practitioner holidays as exception dates
    for holiday in holidays {
        let holiday_dt = naive_to_tz(*holiday, series.start_time);
        set = set.exdate(holiday_dt);
    }

    Ok(set)
}

fn naive_to_tz(date: NaiveDate, time: NaiveTime) -> chrono::DateTime<Tz> {
    let naive_dt = date.and_time(time);
    London
        .from_local_datetime(&naive_dt)
        .single()
        .unwrap_or_else(|| {
            // DST ambiguity — pick the earlier one
            London
                .from_local_datetime(&naive_dt)
                .earliest()
                .expect("date should be representable in Europe/London")
        })
        .with_timezone(&Tz::Europe__London)
}

fn weekday_to_nweekday(day: &Weekday) -> NWeekday {
    match day {
        Weekday::Mon => NWeekday::Every(rrule::Weekday::Mon),
        Weekday::Tue => NWeekday::Every(rrule::Weekday::Tue),
        Weekday::Wed => NWeekday::Every(rrule::Weekday::Wed),
        Weekday::Thu => NWeekday::Every(rrule::Weekday::Thu),
        Weekday::Fri => NWeekday::Every(rrule::Weekday::Fri),
        Weekday::Sat => NWeekday::Every(rrule::Weekday::Sat),
        Weekday::Sun => NWeekday::Every(rrule::Weekday::Sun),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduling::models::*;

    fn test_series(freq: Frequency, interval: u32, dtstart: NaiveDate) -> RecurringSeries {
        RecurringSeries {
            id: Uuid::new_v4(),
            practitioner: "test".to_string(),
            client_id: "EB76".to_string(),
            client_name: "Elizabeth Briscoe".to_string(),
            start_time: NaiveTime::from_hms_opt(10, 0, 0).unwrap(),
            end_time: NaiveTime::from_hms_opt(10, 50, 0).unwrap(),
            location: "37 Gloucester Place".to_string(),
            rate_tag: None,
            recurrence: RecurrenceRule {
                freq,
                interval,
                by_day: None,
                dtstart,
                until: None,
                count: None,
            },
            exdates: vec![],
            status: SeriesStatus::Active,
            created_at: "2026-04-16T00:00:00Z".to_string(),
            notes: None,
        }
    }

    #[test]
    fn weekly_recurrence() {
        let start = NaiveDate::from_ymd_opt(2026, 4, 16).unwrap(); // Thursday
        let series = test_series(Frequency::Weekly, 1, start);

        let from = NaiveDate::from_ymd_opt(2026, 4, 16).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 5, 14).unwrap();

        let dates = materialise(&series, from, to, &[]).unwrap();
        assert_eq!(dates.len(), 4); // 4 Thursdays: Apr 16, 23, 30, May 7
        assert_eq!(dates[0], NaiveDate::from_ymd_opt(2026, 4, 16).unwrap());
        assert_eq!(dates[1], NaiveDate::from_ymd_opt(2026, 4, 23).unwrap());
        assert_eq!(dates[2], NaiveDate::from_ymd_opt(2026, 4, 30).unwrap());
        assert_eq!(dates[3], NaiveDate::from_ymd_opt(2026, 5, 7).unwrap());
    }

    #[test]
    fn fortnightly_recurrence() {
        let start = NaiveDate::from_ymd_opt(2026, 4, 16).unwrap();
        let series = test_series(Frequency::Weekly, 2, start);

        let from = NaiveDate::from_ymd_opt(2026, 4, 16).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 6, 11).unwrap(); // ~8 weeks

        let dates = materialise(&series, from, to, &[]).unwrap();
        assert_eq!(dates.len(), 4); // Apr 16, 30, May 14, 28
        assert_eq!(dates[0], NaiveDate::from_ymd_opt(2026, 4, 16).unwrap());
        assert_eq!(dates[1], NaiveDate::from_ymd_opt(2026, 4, 30).unwrap());
        assert_eq!(dates[2], NaiveDate::from_ymd_opt(2026, 5, 14).unwrap());
        assert_eq!(dates[3], NaiveDate::from_ymd_opt(2026, 5, 28).unwrap());
    }

    #[test]
    fn every_three_weeks() {
        let start = NaiveDate::from_ymd_opt(2026, 4, 16).unwrap();
        let series = test_series(Frequency::Weekly, 3, start);

        let from = NaiveDate::from_ymd_opt(2026, 4, 16).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(); // ~12 weeks

        let dates = materialise(&series, from, to, &[]).unwrap();
        assert_eq!(dates.len(), 4); // Apr 16, May 7, May 28, Jun 18
        assert_eq!(dates[0], NaiveDate::from_ymd_opt(2026, 4, 16).unwrap());
        assert_eq!(dates[1], NaiveDate::from_ymd_opt(2026, 5, 7).unwrap());
        assert_eq!(dates[2], NaiveDate::from_ymd_opt(2026, 5, 28).unwrap());
        assert_eq!(dates[3], NaiveDate::from_ymd_opt(2026, 6, 18).unwrap());
    }

    #[test]
    fn infinite_series_bounded_by_window() {
        // No count, no until — infinite recurrence
        let start = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let series = test_series(Frequency::Weekly, 1, start);

        // Ask for just 4 weeks
        let from = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 4, 28).unwrap();

        let dates = materialise(&series, from, to, &[]).unwrap();
        assert_eq!(dates.len(), 4);
        // All dates should be Thursdays in April (series starts on Thu Jan 1 2026)
        for d in &dates {
            assert!(d >= &from);
            assert!(d <= &to);
        }
    }

    #[test]
    fn exdate_skips_holiday() {
        let start = NaiveDate::from_ymd_opt(2026, 4, 16).unwrap();
        let mut series = test_series(Frequency::Weekly, 1, start);

        // Skip Apr 23 (per-series exception)
        series.exdates.push(NaiveDate::from_ymd_opt(2026, 4, 23).unwrap());

        let from = NaiveDate::from_ymd_opt(2026, 4, 16).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 5, 14).unwrap();

        let dates = materialise(&series, from, to, &[]).unwrap();
        assert_eq!(dates.len(), 3); // Apr 16, 30, May 7 (Apr 23 skipped)
        assert!(!dates.contains(&NaiveDate::from_ymd_opt(2026, 4, 23).unwrap()));
    }

    #[test]
    fn practitioner_holiday_skips_date() {
        let start = NaiveDate::from_ymd_opt(2026, 4, 16).unwrap();
        let series = test_series(Frequency::Weekly, 1, start);

        let holidays = vec![NaiveDate::from_ymd_opt(2026, 4, 30).unwrap()];

        let from = NaiveDate::from_ymd_opt(2026, 4, 16).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 5, 14).unwrap();

        let dates = materialise(&series, from, to, &holidays).unwrap();
        assert_eq!(dates.len(), 3); // Apr 16, 23, May 7 (Apr 30 = holiday)
        assert!(!dates.contains(&NaiveDate::from_ymd_opt(2026, 4, 30).unwrap()));
    }

    #[test]
    fn count_limited_series() {
        let start = NaiveDate::from_ymd_opt(2026, 4, 16).unwrap();
        let mut series = test_series(Frequency::Weekly, 1, start);
        series.recurrence.count = Some(3); // Only 3 sessions

        let from = NaiveDate::from_ymd_opt(2026, 4, 16).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap(); // Large window

        let dates = materialise(&series, from, to, &[]).unwrap();
        assert_eq!(dates.len(), 3); // Only 3, despite large window
    }

    #[test]
    fn block_expiry_warning_at_threshold() {
        let block = AuthorisationBlock {
            id: Uuid::new_v4(),
            client_id: "EB76".to_string(),
            insurer: "AXA".to_string(),
            policy_number: None,
            authorised_sessions: 10,
            used_sessions: 8,
            start_date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            end_date: None,
            status: BlockStatus::Active,
            authorisation_ref: None,
        };

        let warning = check_block_expiry(&block, 2);
        assert!(warning.is_some());
        let w = warning.unwrap();
        assert_eq!(w.remaining, 2);
        assert!(w.message.contains("2 sessions remaining"));
        assert!(w.message.contains("AXA"));
    }

    #[test]
    fn block_no_warning_when_plenty_remain() {
        let block = AuthorisationBlock {
            id: Uuid::new_v4(),
            client_id: "EB76".to_string(),
            insurer: "BUPA".to_string(),
            policy_number: None,
            authorised_sessions: 10,
            used_sessions: 3,
            start_date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            end_date: None,
            status: BlockStatus::Active,
            authorisation_ref: None,
        };

        let warning = check_block_expiry(&block, 2);
        assert!(warning.is_none());
    }

    #[test]
    fn block_exhausted_warning() {
        let block = AuthorisationBlock {
            id: Uuid::new_v4(),
            client_id: "JL07".to_string(),
            insurer: "Vitality".to_string(),
            policy_number: None,
            authorised_sessions: 6,
            used_sessions: 6,
            start_date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            end_date: None,
            status: BlockStatus::Active,
            authorisation_ref: None,
        };

        let warning = check_block_expiry(&block, 2);
        assert!(warning.is_some());
        let w = warning.unwrap();
        assert_eq!(w.remaining, 0);
        assert!(w.message.contains("exhausted"));
    }

    #[test]
    fn monthly_recurrence() {
        let start = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let series = test_series(Frequency::Monthly, 1, start);

        let from = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();

        let dates = materialise(&series, from, to, &[]).unwrap();
        assert!(dates.len() >= 5); // Jan-May at least
        assert_eq!(dates[0], NaiveDate::from_ymd_opt(2026, 1, 15).unwrap());
        assert_eq!(dates[1], NaiveDate::from_ymd_opt(2026, 2, 15).unwrap());
    }
}
