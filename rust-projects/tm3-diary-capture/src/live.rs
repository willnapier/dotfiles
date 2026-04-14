//! Live diary scraping via headless Chrome + TM3 cookie auth.
//!
//! Navigates to the TM3 diary, extracts the rendered HTML, and passes
//! it to the same html::parse_diary() parser used for SingleFile exports.
//! Single parsing path — no JS DOM evaluation.

use anyhow::{bail, Context, Result};
use headless_chrome::protocol::cdp::Network;
use headless_chrome::{Browser, LaunchOptions};
use serde::Deserialize;
use std::process::Command;
use std::time::Duration;

use crate::html::{self, DaySchedule};

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
///
/// Strategy: navigate to the diary page, wait for the scheduler grid to render,
/// grab the full outerHTML, and pass it to html::parse_diary(). This ensures
/// both live and file paths use identical parsing logic.
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

    // Navigate to diary (cookies now set)
    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(5));

    let url = tab.get_url();
    if url.contains("login") {
        bail!("Session expired. Run 'tm3-upload login' to re-authenticate.");
    }

    eprintln!("Authenticated. Waiting for diary to render...");

    // Wait for appointment elements to appear in the DOM.
    // We look for div[title] elements whose title matches the time pattern
    // (e.g. "10:00-10:50 - Client Name - ..."), OR the grid container with
    // the 2880px height. Either indicates the diary has rendered.
    let mut diary_ready = false;
    for _ in 0..20 {
        let check = tab.evaluate(
            r#"(function() {
                // Check for appointment title elements
                var titles = document.querySelectorAll('div[title]');
                for (var i = 0; i < titles.length; i++) {
                    if (/^\d{2}:\d{2}-\d{2}:\d{2} - /.test(titles[i].title)) return true;
                }
                // Check for the grid container (SingleFile-style inline styles)
                if (document.querySelector('div[style*="height:2880px"]')) return true;
                // Check for day headers
                var text = document.body.innerText;
                if (/Mon \d{1,2}(st|nd|rd|th)/.test(text) && /(January|February|March|April|May|June|July|August|September|October|November|December)\s+\d{4}/.test(text)) return true;
                return false;
            })()"#,
            false,
        );
        if let Ok(result) = check {
            if result.value.as_ref().and_then(|v| v.as_bool()) == Some(true) {
                diary_ready = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }

    if !diary_ready {
        bail!("Diary did not render within 20 seconds. TM3 may have changed layout or cookies may be expired.");
    }

    // Give an extra moment for any remaining async rendering
    std::thread::sleep(Duration::from_secs(2));

    eprintln!("Diary rendered. Extracting HTML...");

    // Get the full page HTML
    let html_result = tab.evaluate(
        "document.documentElement.outerHTML",
        false,
    ).context("Failed to extract page HTML")?;

    let page_html = html_result
        .value
        .as_ref()
        .and_then(|v| v.as_str())
        .context("Page HTML was empty")?;

    // Save HTML for debugging if DUMP_HTML env var is set
    if std::env::var("DUMP_HTML").is_ok() {
        let dump_path = "/tmp/tm3-live-dump.html";
        let _ = std::fs::write(dump_path, page_html);
        eprintln!("HTML dumped to {dump_path}");
    }

    // Parse using the same parser as the file path
    html::parse_diary(page_html)
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
