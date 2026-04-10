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

    // Simple broad DOM scrape — find ALL elements with appointment-like content
    let result = tab.evaluate(
        r#"
        (function() {
            var info = {};

            // Get the month/year header
            var monthHeader = document.body.innerText.match(/(January|February|March|April|May|June|July|August|September|October|November|December)\s+\d{4}/);
            info.monthYear = monthHeader ? monthHeader[0] : null;

            // Get day column headers
            var allText = document.body.innerText;
            var dayPattern = /(Mon|Tue|Wed|Thu|Fri|Sat|Sun)\s+\d{1,2}(st|nd|rd|th)/g;
            var days = [];
            var m;
            while ((m = dayPattern.exec(allText)) !== null) {
                if (days.indexOf(m[0]) === -1) days.push(m[0]);
            }
            info.dayHeaders = days;

            // Find elements that look like appointments (blue blocks with text)
            // Try multiple selector strategies
            var selectors = [
                '[class*="event"]', '[class*="Event"]',
                '[class*="appointment"]', '[class*="Appointment"]',
                '[class*="booking"]', '[class*="Booking"]',
                '[data-uid]',
                '.schedule-event', '.calendar-event',
                '[style*="background"]' // appointment blocks often have inline bg color
            ];

            var found = {};
            for (var i = 0; i < selectors.length; i++) {
                var els = document.querySelectorAll(selectors[i]);
                if (els.length > 0 && els.length < 200) {
                    found[selectors[i]] = Array.from(els).slice(0, 10).map(function(el) {
                        return {
                            tag: el.tagName,
                            class: (el.className || '').toString().substring(0, 120),
                            text: el.innerText ? el.innerText.trim().substring(0, 200) : '',
                            style: (el.getAttribute('style') || '').substring(0, 100),
                            dataAttrs: Array.from(el.attributes).filter(function(a) {
                                return a.name.startsWith('data-');
                            }).map(function(a) { return a.name + '=' + a.value.substring(0, 40); })
                        };
                    });
                }
            }
            info.selectorResults = found;

            // Try to find any element containing appointment text patterns (time-time Name)
            var allDivs = document.querySelectorAll('div, td, span');
            var apptPattern = /\d{1,2}:\d{2}\s*-\s*\d{1,2}:\d{2}/;
            var apptDivs = [];
            for (var j = 0; j < allDivs.length && apptDivs.length < 20; j++) {
                var txt = allDivs[j].innerText || '';
                if (apptPattern.test(txt) && txt.length < 300 && txt.length > 10) {
                    apptDivs.push({
                        tag: allDivs[j].tagName,
                        class: (allDivs[j].className || '').toString().substring(0, 120),
                        text: txt.trim().substring(0, 200),
                        parentClass: (allDivs[j].parentElement ? allDivs[j].parentElement.className || '' : '').toString().substring(0, 80)
                    });
                }
            }
            info.timePatternElements = apptDivs;

            return JSON.stringify(info, null, 2);
        })()
        "#,
        false,
    )?;

    if let Some(val) = result.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("{}", s);
    } else {
        eprintln!("evaluate returned no value");
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
