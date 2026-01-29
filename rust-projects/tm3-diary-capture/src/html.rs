use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use regex::Regex;
use scraper::{Html, Selector};

#[derive(Debug, Clone)]
pub struct Appointment {
    pub start_time: String,
    pub client_name: String,
    pub rate_tag: Option<String>,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Booked,
    Cancelled,
}

#[derive(Debug)]
pub struct DaySchedule {
    pub date: NaiveDate,
    pub appointments: Vec<Appointment>,
}

/// Parse a TM3 diary HTML snapshot into day schedules.
pub fn parse_diary(html: &str) -> Result<Vec<DaySchedule>> {
    let month_year = extract_month_year(html)?;
    let day_headers = extract_day_headers(html)?;
    let dates = resolve_dates(&day_headers, &month_year)?;
    let columns = extract_day_columns(html)?;

    if dates.len() != columns.len() {
        bail!(
            "Date count ({}) doesn't match column count ({})",
            dates.len(),
            columns.len()
        );
    }

    let mut schedules = Vec::new();
    for (date, titles) in dates.into_iter().zip(columns) {
        let appointments: Vec<Appointment> = titles
            .iter()
            .filter_map(|t| parse_title(t).ok())
            .collect();
        schedules.push(DaySchedule {
            date,
            appointments,
        });
    }

    Ok(schedules)
}

/// Extract "January 2026" from the HTML.
fn extract_month_year(html: &str) -> Result<String> {
    let re = Regex::new(r#"class="text-2xl bold">([A-Z][a-z]+ \d{4})<"#)?;
    let cap = re
        .captures(html)
        .context("Could not find month/year header (e.g. 'January 2026')")?;
    Ok(cap[1].to_string())
}

/// Extract day headers like ["Mon 26th", "Tue 27th", ...].
fn extract_day_headers(html: &str) -> Result<Vec<String>> {
    let re = Regex::new(
        r#"grid-column:span 1">(Mon|Tue|Wed|Thu|Fri|Sat|Sun) (\d{1,2}(?:st|nd|rd|th))</div>"#,
    )?;
    let headers: Vec<String> = re
        .captures_iter(html)
        .map(|cap| format!("{} {}", &cap[1], &cap[2]))
        .collect();
    if headers.is_empty() {
        bail!("No day headers found");
    }
    Ok(headers)
}

/// Convert "Mon 26th" + "January 2026" into NaiveDate.
fn resolve_dates(headers: &[String], month_year: &str) -> Result<Vec<NaiveDate>> {
    let re = Regex::new(r"(?:Mon|Tue|Wed|Thu|Fri|Sat|Sun) (\d{1,2})(?:st|nd|rd|th)")?;
    let my_re = Regex::new(r"([A-Z][a-z]+) (\d{4})")?;
    let my_cap = my_re
        .captures(month_year)
        .context("Invalid month/year format")?;
    let month_name = &my_cap[1];
    let year: i32 = my_cap[2].parse()?;
    let month = parse_month(month_name)?;

    let mut dates = Vec::new();
    for header in headers {
        let cap = re
            .captures(header)
            .with_context(|| format!("Invalid day header: {}", header))?;
        let day: u32 = cap[1].parse()?;
        let date = NaiveDate::from_ymd_opt(year, month, day)
            .with_context(|| format!("Invalid date: {} {} {}", year, month, day))?;
        dates.push(date);
    }
    Ok(dates)
}

fn parse_month(name: &str) -> Result<u32> {
    match name {
        "January" => Ok(1),
        "February" => Ok(2),
        "March" => Ok(3),
        "April" => Ok(4),
        "May" => Ok(5),
        "June" => Ok(6),
        "July" => Ok(7),
        "August" => Ok(8),
        "September" => Ok(9),
        "October" => Ok(10),
        "November" => Ok(11),
        "December" => Ok(12),
        _ => bail!("Unknown month: {}", name),
    }
}

/// Extract appointment titles grouped by day column.
///
/// The HTML has a 6-column grid (2880px height). Each child is a day column.
/// Within each column, appointments have a div[title] matching the time pattern.
fn extract_day_columns(html: &str) -> Result<Vec<Vec<String>>> {
    let doc = Html::parse_document(html);

    // Find the main grid container (2880px height, 6 columns)
    let div_sel = Selector::parse("div").unwrap();
    let title_sel = Selector::parse("div[title]").unwrap();
    let time_re = Regex::new(r"^\d{2}:\d{2}-\d{2}:\d{2} - ")?;

    let mut grid_container = None;
    for el in doc.select(&div_sel) {
        if let Some(style) = el.value().attr("style") {
            if style.contains("height:2880px")
                && style.contains("grid-template-columns")
                && style.contains("301px")
            {
                grid_container = Some(el);
                break;
            }
        }
    }

    let grid = grid_container.context("Could not find appointment grid container")?;
    let mut columns = Vec::new();

    // Each direct child of the grid is a day column
    for child in grid.children() {
        if let Some(child_el) = child.value().as_element() {
            let child_ref = scraper::ElementRef::wrap(child).unwrap();
            let _ = child_el; // used for the is_element check
            let mut titles = Vec::new();
            for title_el in child_ref.select(&title_sel) {
                if let Some(title) = title_el.value().attr("title") {
                    if time_re.is_match(title) {
                        titles.push(title.to_string());
                    }
                }
            }
            columns.push(titles);
        }
    }

    if columns.is_empty() {
        bail!("No day columns found in grid");
    }

    Ok(columns)
}

/// Parse a title attribute into an Appointment.
///
/// Format: "HH:MM-HH:MM - ClientName - [RateInfo...] - Location - Status"
///
/// Strategy: parse from ends inward.
/// - First segment: time range
/// - Last segment: Booked/Cancelled
/// - Second-to-last: location (contains "Gloucester Place" or similar)
/// - Second segment: client name
/// - Middle segments: rate info (may include "In Debt" prefix, notes)
fn parse_title(title: &str) -> Result<Appointment> {
    let parts: Vec<&str> = title.split(" - ").collect();
    if parts.len() < 4 {
        bail!("Title has too few segments: {}", title);
    }

    // First segment: time range "HH:MM-HH:MM"
    let time_part = parts[0];
    let time_re = Regex::new(r"^(\d{2}:\d{2})-(\d{2}:\d{2})$")?;
    let time_cap = time_re
        .captures(time_part)
        .with_context(|| format!("Invalid time format: {}", time_part))?;
    let start_time = time_cap[1].to_string();

    // Last segment: status
    let status_str = parts.last().unwrap().trim();
    let status = match status_str {
        "Booked" => Status::Booked,
        "Cancelled" => Status::Cancelled,
        _ => bail!("Unknown status: {}", status_str),
    };

    // Second segment: client name
    let client_name = parts[1].trim().to_string();

    // Middle segments (between client name and location/status): rate info
    // Location is second-to-last and typically contains "Place" or a street address
    // Rate info is everything between client name and location
    let rate_segments = &parts[2..parts.len() - 2]; // skip time, client, location, status
    let rate_tag = classify_rate(rate_segments);

    Ok(Appointment {
        start_time,
        client_name,
        rate_tag,
        status,
    })
}

/// Classify rate segments into a note mode tag.
///
/// Returns None for self-pay (default), Some("insurer") for insurance,
/// Some("couples") for couples rate.
fn classify_rate(segments: &[&str]) -> Option<String> {
    let combined = segments.join(" - ");

    if combined.contains("Couples Rate") {
        return Some("couples".to_string());
    }

    // Strip "In Debt - " prefix if present for classification
    let rate_str = if combined.starts_with("In Debt - ") {
        &combined["In Debt - ".len()..]
    } else {
        &combined
    };

    if rate_str.starts_with("AXA")
        || rate_str.starts_with("BUPA")
        || rate_str.starts_with("Insurance Rate")
        || rate_str.starts_with("Insurance Reduced")
        || rate_str.starts_with("PWC")
        || rate_str.starts_with("Taylor Wessing")
    {
        return Some("insurer".to_string());
    }

    // Standard Rate / Self Paid = default, no tag
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_title_standard() {
        let title =
            "10:00-11:00 - Laurillard, Jasmina - Standard Rate_Self Paid_19 - 37 Gloucester Place  - Booked";
        let appt = parse_title(title).unwrap();
        assert_eq!(appt.start_time, "10:00");
        assert_eq!(appt.client_name, "Laurillard, Jasmina");
        assert_eq!(appt.rate_tag, None);
        assert_eq!(appt.status, Status::Booked);
    }

    #[test]
    fn test_parse_title_cancelled() {
        let title =
            "12:30-13:15 - Parker, Emily - Standard Rate_Self Paid_19 - 37 Gloucester Place  - Cancelled";
        let appt = parse_title(title).unwrap();
        assert_eq!(appt.status, Status::Cancelled);
    }

    #[test]
    fn test_parse_title_axa() {
        let title =
            "13:20-14:05 - Andrade, Bruna - AXA_Victoria Jenkins - 37 Gloucester Place  - Booked";
        let appt = parse_title(title).unwrap();
        assert_eq!(appt.rate_tag, Some("insurer".to_string()));
    }

    #[test]
    fn test_parse_title_bupa() {
        let title =
            "14:10-14:55 - Pugh-Smith, Marcus - Insurance Rate - BUPA - 37 Gloucester Place  - Booked";
        let appt = parse_title(title).unwrap();
        assert_eq!(appt.rate_tag, Some("insurer".to_string()));
    }

    #[test]
    fn test_parse_title_couples() {
        let title =
            "19:05-19:50 - Perry, Navdeep and Ashley - Couples Rate_23 - 37 Gloucester Place  - Booked";
        let appt = parse_title(title).unwrap();
        assert_eq!(appt.rate_tag, Some("couples".to_string()));
    }

    #[test]
    fn test_parse_title_in_debt_axa() {
        let title =
            "08:35-09:20 - Dodoc, Joana - In Debt - AXA_Victoria Jenkins - verify AXA count - 37 Gloucester Place  - Cancelled";
        let appt = parse_title(title).unwrap();
        assert_eq!(appt.client_name, "Dodoc, Joana");
        assert_eq!(appt.rate_tag, Some("insurer".to_string()));
        assert_eq!(appt.status, Status::Cancelled);
    }

    #[test]
    fn test_parse_title_in_debt_standard() {
        let title =
            "10:55-12:25 - Thomas, Anisha - In Debt - Standard Rate_Self Paid Double_19 - 37 Gloucester Place  - Booked";
        let appt = parse_title(title).unwrap();
        assert_eq!(appt.rate_tag, None);
    }

    #[test]
    fn test_parse_title_pwc() {
        let title =
            "17:30-18:15 - Thomas, Catrin - PWC_PARTNER_ALL - 37 Gloucester Place  - Booked";
        let appt = parse_title(title).unwrap();
        assert_eq!(appt.rate_tag, Some("insurer".to_string()));
    }

    #[test]
    fn test_parse_title_insurance_reduced_with_notes() {
        let title = "16:30-17:15 - Takchi, Caroline - Insurance Reduced Rate - 4 - Will be on Zoom today - 37 Gloucester Place  - Booked";
        let appt = parse_title(title).unwrap();
        assert_eq!(appt.rate_tag, Some("insurer".to_string()));
    }

    #[test]
    fn test_parse_title_taylor_wessing_with_notes() {
        let title = "16:05-16:50 - Cowan, Phoebe - Taylor Wessing - Rate 1  - confirm she is continuing / draft report - 37 Gloucester Place  - Booked";
        let appt = parse_title(title).unwrap();
        assert_eq!(appt.rate_tag, Some("insurer".to_string()));
    }

    #[test]
    fn test_parse_title_in_debt_pwc() {
        let title =
            "09:30-10:15 - Jain, Abhijay - In Debt - PWC_PARTNER_ALL - 37 Gloucester Place  - Booked";
        let appt = parse_title(title).unwrap();
        assert_eq!(appt.rate_tag, Some("insurer".to_string()));
    }

    #[test]
    fn test_parse_title_in_debt_bupa() {
        let title = "15:15-16:00 - Bunyard, Ben - In Debt - Insurance Rate - BUPA - 37 Gloucester Place  - Booked";
        let appt = parse_title(title).unwrap();
        assert_eq!(appt.rate_tag, Some("insurer".to_string()));
    }

    #[test]
    fn test_parse_title_standard_with_notes() {
        let title = "18:15-19:00 - Villani, Dominic - Standard Rate_Self Paid_19 - verify that he is continuing - 37 Gloucester Place  - Booked";
        let appt = parse_title(title).unwrap();
        assert_eq!(appt.rate_tag, None);
    }

    #[test]
    fn test_classify_rate_standard() {
        assert_eq!(classify_rate(&["Standard Rate_Self Paid_19"]), None);
        assert_eq!(
            classify_rate(&["Standard Rate_Self Paid Double_19"]),
            None
        );
    }

    #[test]
    fn test_classify_rate_insurer() {
        assert_eq!(
            classify_rate(&["AXA_Victoria Jenkins"]),
            Some("insurer".to_string())
        );
        assert_eq!(
            classify_rate(&["Insurance Rate", "BUPA"]),
            Some("insurer".to_string())
        );
        assert_eq!(
            classify_rate(&["Insurance Reduced Rate", "4"]),
            Some("insurer".to_string())
        );
        assert_eq!(
            classify_rate(&["PWC_PARTNER_ALL"]),
            Some("insurer".to_string())
        );
        assert_eq!(
            classify_rate(&["Taylor Wessing", "Rate 1 "]),
            Some("insurer".to_string())
        );
    }

    #[test]
    fn test_classify_rate_in_debt() {
        assert_eq!(
            classify_rate(&["In Debt - AXA_Victoria Jenkins"]),
            Some("insurer".to_string())
        );
        assert_eq!(
            classify_rate(&["In Debt - Standard Rate_Self Paid Double_19"]),
            None
        );
    }

    #[test]
    fn test_extract_month_year() {
        let html = r#"<span class="text-2xl bold">January 2026</span>"#;
        assert_eq!(extract_month_year(html).unwrap(), "January 2026");
    }

    #[test]
    fn test_extract_day_headers() {
        let html = r#"<div style="grid-column:span 1">Mon 26th</div><div style="grid-column:span 1">Tue 27th</div>"#;
        let headers = extract_day_headers(html).unwrap();
        assert_eq!(headers, vec!["Mon 26th", "Tue 27th"]);
    }

    #[test]
    fn test_resolve_dates() {
        let headers = vec!["Mon 26th".to_string(), "Tue 27th".to_string()];
        let dates = resolve_dates(&headers, "January 2026").unwrap();
        assert_eq!(dates[0], NaiveDate::from_ymd_opt(2026, 1, 26).unwrap());
        assert_eq!(dates[1], NaiveDate::from_ymd_opt(2026, 1, 27).unwrap());
    }
}
