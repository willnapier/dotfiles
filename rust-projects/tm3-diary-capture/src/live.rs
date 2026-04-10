//! Live diary scraping via headless Chrome + TM3 cookie auth.

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use headless_chrome::protocol::cdp::Network;
use headless_chrome::{Browser, LaunchOptions};
use serde::Deserialize;
use std::process::Command;
use std::time::Duration;

use crate::html::{Appointment, DaySchedule, Status};

const TM3_BASE: &str = "https://changeofharleystreet.tm3app.com";

#[derive(Deserialize)]
struct Cookie {
    name: String,
    value: String,
    domain: String,
    path: String,
    secure: bool,
    http_only: bool,
    expires: Option<f64>,
}

/// Scrape the TM3 diary via headless Chrome, returning the same DaySchedule
/// format as the HTML parser.
pub fn scrape_diary() -> Result<Vec<DaySchedule>> {
    let cookies = load_cookies()?;

    eprintln!("Launching headless Chrome...");
    let browser = Browser::new(
        LaunchOptions::default_builder()
            .headless(true)
            .window_size(Some((1280, 900)))
            .idle_browser_timeout(Duration::from_secs(120))
            .build()
            .context("Failed to build launch options")?,
    )
    .context("Failed to launch Chrome")?;

    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(30));

    // Inject cookies
    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(3));

    for c in &cookies {
        let _ = tab.call_method(Network::SetCookie {
            name: c.name.clone(),
            value: c.value.clone(),
            url: None,
            domain: Some(c.domain.clone()),
            path: Some(c.path.clone()),
            secure: Some(c.secure),
            http_only: Some(c.http_only),
            same_site: None,
            expires: c.expires,
            priority: None,
            same_party: None,
            source_scheme: None,
            source_port: None,
            partition_key: None,
        });
    }

    // Navigate to diary
    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(5));

    let url = tab.get_url();
    if url.contains("login") {
        bail!("Session expired. Run 'tm3-upload login' to re-authenticate.");
    }

    eprintln!("Authenticated. Scraping diary...");

    // Extract appointments from the DOM
    let json = tab
        .evaluate(
            r#"
            (function() {
                // Get month and year
                var bodyText = document.body.innerText;
                var monthMatch = bodyText.match(/(January|February|March|April|May|June|July|August|September|October|November|December)\s+(\d{4})/);
                if (!monthMatch) return JSON.stringify({error: "Could not find month/year"});
                var monthName = monthMatch[1];
                var year = parseInt(monthMatch[2]);

                var months = {January:1,February:2,March:3,April:4,May:5,June:6,
                              July:7,August:8,September:9,October:10,November:11,December:12};
                var monthNum = months[monthName];

                // Get day headers
                var dayPattern = /(Mon|Tue|Wed|Thu|Fri|Sat|Sun)\s+(\d{1,2})(?:st|nd|rd|th)/g;
                var dayHeaders = [];
                var m;
                while ((m = dayPattern.exec(bodyText)) !== null) {
                    var dayNum = parseInt(m[2]);
                    var dateStr = year + '-' + String(monthNum).padStart(2,'0') + '-' + String(dayNum).padStart(2,'0');
                    if (dayHeaders.findIndex(function(d) { return d.date === dateStr; }) === -1) {
                        dayHeaders.push({dayName: m[1], dayNum: dayNum, date: dateStr});
                    }
                }

                // Get all schedule columns
                var columns = document.querySelectorAll('.schedule-layer');

                // Build per-day appointments
                var days = [];
                for (var c = 0; c < columns.length; c++) {
                    var header = c < dayHeaders.length ? dayHeaders[c] : null;
                    if (!header) continue;

                    var apptDivs = columns[c].querySelectorAll(
                        'div[style*="background"].cursor-pointer'
                    );

                    var appointments = [];
                    for (var a = 0; a < apptDivs.length; a++) {
                        var text = apptDivs[a].innerText.trim();
                        if (!text) continue;

                        var lines = text.split('\n');
                        var firstLine = lines[0].trim();

                        // Parse "HH:MM-HH:MM Name" pattern
                        var timeMatch = firstLine.match(/^(\d{1,2}:\d{2})\s*-\s*(\d{1,2}:\d{2})\s+(.*)/);
                        if (!timeMatch) continue;

                        var startTime = timeMatch[1];
                        var clientName = timeMatch[3].trim();
                        var rateTag = lines.length > 1 ? lines[1].trim() : null;

                        // Skip administration blocks
                        if (clientName.toLowerCase() === 'administration') continue;

                        // Check for cancelled status (red/strikethrough styling)
                        var isCancelled = apptDivs[a].classList.contains('line-through') ||
                            apptDivs[a].querySelector('.line-through') !== null ||
                            apptDivs[a].style.textDecoration === 'line-through';

                        appointments.push({
                            start_time: startTime,
                            client_name: clientName,
                            rate_tag: rateTag,
                            status: isCancelled ? 'cancelled' : 'booked'
                        });
                    }

                    days.push({
                        date: header.date,
                        appointments: appointments
                    });
                }

                return JSON.stringify({days: days});
            })()
            "#,
            false,
        )
        .context("Failed to evaluate diary scraper")?;

    let json_str = json
        .value
        .as_ref()
        .and_then(|v| v.as_str())
        .context("Diary scraper returned no data")?;

    // Parse the JSON into DaySchedules
    let parsed: serde_json::Value =
        serde_json::from_str(json_str).context("Failed to parse diary JSON")?;

    if let Some(err) = parsed.get("error") {
        bail!("Diary scraper error: {}", err);
    }

    let days = parsed["days"]
        .as_array()
        .context("Expected 'days' array")?;

    let mut schedules = Vec::new();
    for day in days {
        let date_str = day["date"].as_str().unwrap_or("");
        let date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
            .with_context(|| format!("Invalid date: {}", date_str))?;

        let appts = day["appointments"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| {
                        Some(Appointment {
                            start_time: a["start_time"].as_str()?.to_string(),
                            client_name: a["client_name"].as_str()?.to_string(),
                            rate_tag: a["rate_tag"].as_str().map(|s| s.to_string()),
                            status: if a["status"].as_str() == Some("cancelled") {
                                Status::Cancelled
                            } else {
                                Status::Booked
                            },
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        schedules.push(DaySchedule {
            date,
            appointments: appts,
        });
    }

    Ok(schedules)
}

fn load_cookies() -> Result<Vec<Cookie>> {
    let output = Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "tm3-session",
            "-a",
            "changeofharleystreet",
            "-w",
        ])
        .output()
        .context("Failed to read keychain")?;

    if !output.status.success() {
        bail!("No TM3 session in keychain. Run 'tm3-upload login' first.");
    }

    let json = String::from_utf8(output.stdout)?.trim().to_string();
    Ok(serde_json::from_str(&json)?)
}
