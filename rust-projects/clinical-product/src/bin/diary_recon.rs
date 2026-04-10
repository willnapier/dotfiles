//! One-shot diary DOM reconnaissance — discover appointment selectors.

use anyhow::{Context, Result};
use headless_chrome::protocol::cdp::Network;
use headless_chrome::{Browser, LaunchOptions};
use serde::Deserialize;
use std::process::Command;
use std::time::Duration;

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

fn main() -> Result<()> {
    let output = Command::new("security")
        .args(["find-generic-password", "-s", "tm3-session", "-a", "changeofharleystreet", "-w"])
        .output()?;
    let json = String::from_utf8(output.stdout)?.trim().to_string();
    let cookies: Vec<Cookie> = serde_json::from_str(&json)?;

    let browser = Browser::new(
        LaunchOptions::default_builder()
            .headless(true)
            .window_size(Some((1280, 900)))
            .idle_browser_timeout(Duration::from_secs(120))
            .build()?,
    )?;

    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(30));

    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(3));

    for c in &cookies {
        let _ = tab.call_method(Network::SetCookie {
            name: c.name.clone(), value: c.value.clone(), url: None,
            domain: Some(c.domain.clone()), path: Some(c.path.clone()),
            secure: Some(c.secure), http_only: Some(c.http_only),
            same_site: None, expires: c.expires, priority: None,
            same_party: None, source_scheme: None, source_port: None,
            partition_key: None,
        });
    }

    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(5));

    eprintln!("URL: {}", tab.get_url());

    // Deep inspection of the diary/scheduler DOM
    let result = tab.evaluate(
        r#"
        (function() {
            var info = {};
            info.url = window.location.href;
            info.title = document.title;

            // Look for Kendo scheduler
            if (typeof kendo !== 'undefined' && typeof jQuery !== 'undefined') {
                var scheduler = jQuery('.k-scheduler').data('kendoScheduler');
                if (scheduler) {
                    info.kendoScheduler = {
                        found: true,
                        viewName: scheduler.viewName(),
                        date: scheduler.date() ? scheduler.date().toISOString() : null,
                        dataSourceLength: scheduler.dataSource.total()
                    };

                    // Get events from the data source
                    var events = scheduler.dataSource.data();
                    info.events = [];
                    for (var i = 0; i < Math.min(events.length, 50); i++) {
                        var ev = events[i];
                        info.events.push({
                            title: ev.title || ev.Title || '',
                            start: ev.start ? ev.start.toISOString() : (ev.Start || ''),
                            end: ev.end ? ev.end.toISOString() : (ev.End || ''),
                            description: ev.description || ev.Description || '',
                            id: ev.id || ev.Id || '',
                            // Dump all field names to discover the schema
                            fields: Object.keys(ev).filter(function(k) {
                                return typeof ev[k] !== 'function' && k !== '_events';
                            })
                        });
                    }
                } else {
                    info.kendoScheduler = {found: false, reason: 'no kendoScheduler widget'};
                }
            }

            // Also look for appointment elements in the DOM
            var apptElements = document.querySelectorAll(
                '.k-event, [class*="appointment" i], [class*="booking" i], ' +
                '[class*="schedule" i][class*="item" i], [data-uid]'
            );
            info.appointmentElements = Array.from(apptElements).slice(0, 30).map(function(el) {
                return {
                    tag: el.tagName,
                    class: el.className.substring(0, 100),
                    text: el.innerText.trim().substring(0, 150),
                    dataUid: el.getAttribute('data-uid') || '',
                    title: el.getAttribute('title') || '',
                    ariaLabel: el.getAttribute('aria-label') || ''
                };
            });

            // Check for the React scheduler (TM3 might use React for the diary)
            var reactScheduler = document.querySelectorAll(
                '[class*="calendar" i], [class*="scheduler" i], [class*="diary" i], ' +
                '[class*="event" i][class*="card" i], [class*="appointment" i][class*="card" i]'
            );
            info.reactSchedulerElements = Array.from(reactScheduler).slice(0, 20).map(function(el) {
                return {
                    tag: el.tagName,
                    class: el.className.substring(0, 100),
                    text: el.innerText.trim().substring(0, 200),
                    childCount: el.childElementCount
                };
            });

            // Look at the column headers (day names/dates)
            var headers = document.querySelectorAll(
                'th, [class*="header" i][class*="day" i], [class*="column" i][class*="header" i]'
            );
            info.dayHeaders = Array.from(headers).slice(0, 20).map(function(el) {
                return {
                    tag: el.tagName,
                    class: el.className.substring(0, 80),
                    text: el.innerText.trim().substring(0, 60)
                };
            });

            return JSON.stringify(info, null, 2);
        })()
        "#,
        false,
    )?;

    if let Some(val) = result.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("{}", s);
    }

    // Screenshot
    let ss_path = std::path::PathBuf::from(std::env::var("HOME").unwrap())
        .join(".config/clinical-product/diary-recon.png");
    if let Ok(bytes) = tab.capture_screenshot(
        headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
        None, None, true,
    ) {
        std::fs::write(&ss_path, &bytes)?;
        eprintln!("Screenshot: {}", ss_path.display());
    }

    Ok(())
}
