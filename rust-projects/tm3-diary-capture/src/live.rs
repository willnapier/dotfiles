//! Live diary scraping via headless Chrome + TM3 cookie auth.
//!
//! Navigates to the TM3 diary, extracts the rendered HTML, and passes
//! it to the same html::parse_diary() parser used for SingleFile exports.
//! Single parsing path — no JS DOM evaluation.

use anyhow::{bail, Context, Result};
use headless_chrome::protocol::cdp::Network;
use headless_chrome::{Browser, LaunchOptions};
use serde::Deserialize;
#[cfg(target_os = "macos")]
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
///
/// `weeks_back`: 0 = current week, 1 = previous week, etc. Clicks the
/// left-arrow navigation button N times before extracting.
pub fn scrape_diary(weeks_back: u32) -> Result<Vec<DaySchedule>> {
    let cookies = load_cookies()?;

    eprintln!("Launching headless Chrome...");
    let browser = Browser::new(
        LaunchOptions::default_builder()
            .headless(true)
            .window_size(Some((1280, 900)))
            .idle_browser_timeout(Duration::from_secs(120))
            .args(vec![
                std::ffi::OsStr::new("--password-store=basic"),
                std::ffi::OsStr::new("--use-mock-keychain"),
                // Suppress Chrome's MediaSession/hardware-media-key probes so
                // macOS doesn't prompt for Apple Music / MediaLibrary access
                // every time the binary's CDHash changes (i.e. every rebuild).
                std::ffi::OsStr::new("--disable-features=MediaSessionService,HardwareMediaKeyHandling"),
                std::ffi::OsStr::new("--mute-audio"),
            ])
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

    // Navigate to previous weeks if requested
    if weeks_back > 0 {
        eprintln!("Navigating back {} week(s)...", weeks_back);
        for i in 0..weeks_back {
            // Click the left-arrow button (previous week)
            // The button contains an SVG with data-icon="arrow-left"
            let clicked = tab.evaluate(
                r#"(function() {
                    var arrows = document.querySelectorAll('svg[data-icon="arrow-left"]');
                    for (var i = 0; i < arrows.length; i++) {
                        var btn = arrows[i].closest('button');
                        if (btn) { btn.click(); return true; }
                    }
                    return false;
                })()"#,
                false,
            );
            match clicked {
                Ok(r) if r.value.as_ref().and_then(|v| v.as_bool()) == Some(true) => {
                    eprintln!("  Week {} of {}...", i + 1, weeks_back);
                    // Wait for the diary to re-render with new data
                    std::thread::sleep(Duration::from_secs(3));
                }
                _ => bail!("Could not find the previous-week navigation button"),
            }
        }
        // Wait for final render to settle
        std::thread::sleep(Duration::from_secs(2));
    }

    eprintln!("Extracting HTML...");

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
    let mut schedules = html::parse_diary(page_html)?;

    // Resolve TM3 IDs for appointments missing them.
    // Click the appointment block in the diary to navigate to the client profile,
    // extract the TM3 ID from the URL, then navigate back.
    let mut resolved = 0;
    for schedule in &mut schedules {
        for appt in &mut schedule.appointments {
            if appt.tm3_id.is_some() { continue; }

            let name_escaped = appt.client_name.replace('\'', "\\'").replace('"', "\\\"");
            let click_js = format!(
                r#"(function() {{
                    var titles = document.querySelectorAll('div[title]');
                    for (var i = 0; i < titles.length; i++) {{
                        var t = titles[i].getAttribute('title') || '';
                        if (t.includes('{}')) {{
                            titles[i].click();
                            return true;
                        }}
                    }}
                    return false;
                }})()"#,
                name_escaped
            );

            match tab.evaluate(&click_js, false) {
                Ok(r) if r.value.as_ref().and_then(|v| v.as_bool()) == Some(true) => {
                    std::thread::sleep(Duration::from_secs(3));
                    let url = tab.get_url();
                    // Extract TM3 ID from URL like /contacts/clients/12345
                    if let Some(id) = url.split("/contacts/clients/").nth(1) {
                        let id = id.split(&['/', '?', '#'][..]).next().unwrap_or(id);
                        if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
                            eprintln!("  Resolved TM3 ID {} for \"{}\"", id, appt.client_name);
                            appt.tm3_id = Some(id.to_string());
                            resolved += 1;
                        }
                    }
                    // Navigate back to diary
                    let _ = tab.evaluate("window.history.back()", false);
                    std::thread::sleep(Duration::from_secs(3));
                }
                _ => {}
            }
        }
    }

    if resolved > 0 {
        eprintln!("Resolved {} TM3 ID(s) via appointment click.", resolved);
    }

    Ok(schedules)
}

fn load_cookies() -> Result<Vec<Cookie>> {
    // Two sources:
    //   1. OS keychain ("tm3-session" / "changeofharleystreet") — written
    //      by `tm3-upload login` on the local machine. Always fresh on
    //      whatever machine just logged in.
    //   2. Shared cookie file (~/Assistants/shared/.tm3-session-cookies.json) —
    //      written by tm3-cookie-sync on Mac and synced via Syncthing so
    //      Nimbini can read Mac's session. Only useful on a machine that
    //      doesn't have a local login.
    //
    // On macOS we prefer the keychain — if the user just ran `tm3-upload
    // login`, that's the fresh source of truth. The shared file may be
    // stale (3-day-old cookies synced back from Nimbini after a previous
    // login, which happened to bite us 2026-04-22 morning). On Linux
    // (Nimbini), the shared file is typically the only source.
    let cookie_file = dirs::home_dir()
        .unwrap_or_default()
        .join("Assistants/shared/.tm3-session-cookies.json");

    #[cfg(target_os = "macos")]
    {
        // Try keychain first on Mac
        if let Ok(cookies) = load_cookies_from_keychain(&cookie_file.display().to_string()) {
            return Ok(cookies);
        }
    }

    // Fall back to shared file (primary source on Linux, fallback on Mac)
    if cookie_file.exists() {
        let json = std::fs::read_to_string(&cookie_file)
            .with_context(|| format!("Failed to read cookie file: {}", cookie_file.display()))?;
        let json = json.trim();
        if !json.is_empty() {
            return serde_json::from_str(json)
                .with_context(|| "Failed to parse cookie file — may be corrupt or empty");
        }
    }

    load_cookies_from_keychain(&cookie_file.display().to_string())
}

#[cfg(target_os = "macos")]
fn load_cookies_from_keychain(_cookie_path: &str) -> Result<Vec<Cookie>> {
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

#[cfg(not(target_os = "macos"))]
fn load_cookies_from_keychain(cookie_path: &str) -> Result<Vec<Cookie>> {
    // Try Linux secret service (populated by `tm3-upload login` on this machine)
    let output = std::process::Command::new("secret-tool")
        .args(["lookup", "service", "tm3-session", "account", "changeofharleystreet"])
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            let json = String::from_utf8(out.stdout)?.trim().to_string();
            if !json.is_empty() {
                return serde_json::from_str(&json)
                    .context("Failed to parse cookies from secret-tool");
            }
        }
    }

    bail!(
        "No TM3 cookies found. Options:\n\
         - Run 'tm3-upload login' on this machine\n\
         - Or sync from Mac: ensure {} is populated via Syncthing",
        cookie_path
    )
}
