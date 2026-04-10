//! TM3 headless browser spike — proof of concept.
//!
//! Phase A (first run / session expired): Log in manually, capture session cookies.
//!   tm3-spike login
//!
//! Phase B (subsequent runs): Load cookies, inspect documents page headlessly.
//!   tm3-spike inspect <tm3_id>
//!
//! Cookies stored at ~/.config/clinical-product/tm3-cookies.json

use anyhow::{bail, Context, Result};
use headless_chrome::browser::tab::Tab;
use headless_chrome::protocol::cdp::Network;
use headless_chrome::{Browser, LaunchOptions};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const TM3_BASE: &str = "https://changeofharleystreet.tm3app.com";

#[derive(Serialize, Deserialize, Debug, Clone)]
struct SavedCookie {
    name: String,
    value: String,
    domain: String,
    path: String,
    secure: bool,
    http_only: bool,
    expires: Option<f64>,
}

fn config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let dir = PathBuf::from(home)
        .join(".config")
        .join("clinical-product");
    std::fs::create_dir_all(&dir).ok();
    dir
}

fn cookie_path() -> PathBuf {
    config_dir().join("tm3-cookies.json")
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage:");
        eprintln!("  tm3-spike login              — log in manually, save session cookies");
        eprintln!("  tm3-spike inspect <tm3_id>   — auto-login via cookies, inspect documents page");
        std::process::exit(1);
    }

    match args[1].as_str() {
        "login" => do_login(),
        "inspect" => {
            let tm3_id = args.get(2).context("Usage: tm3-spike inspect <tm3_id>")?;
            do_inspect(tm3_id)
        }
        other => bail!("Unknown command '{}'. Use 'login' or 'inspect'.", other),
    }
}

// --- Phase A: Manual login + cookie capture ---
fn do_login() -> Result<()> {
    eprintln!("[login] Launching Chrome (visible)...");
    let browser = launch_browser(false)?;
    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(30));

    eprintln!("[login] Navigating to TM3...");
    tab.navigate_to(TM3_BASE)?;
    tab.wait_until_navigated()?;
    eprintln!("[login] Landed at: {}", tab.get_url());

    eprintln!();
    eprintln!("[login] ==========================================");
    eprintln!("[login]  Log in with your passkey (Touch ID).    ");
    eprintln!("[login]  I'll capture cookies once you're in.    ");
    eprintln!("[login] ==========================================");
    eprintln!();

    // Wait for authentication
    wait_for_auth(&tab)?;
    eprintln!("[login] Authenticated! URL: {}", tab.get_url());

    // Let the app fully settle
    std::thread::sleep(Duration::from_secs(2));

    // Capture cookies
    save_cookies(&tab)?;

    eprintln!();
    eprintln!("[login] Done! You can now use: tm3-spike inspect <tm3_id>");

    // Keep browser open briefly so user can see it worked
    eprintln!("[login] Press Enter to close browser...");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    Ok(())
}

// --- Phase B: Cookie-based auto-login + inspect ---
fn do_inspect(tm3_id: &str) -> Result<()> {
    let path = cookie_path();
    let json = std::fs::read_to_string(&path).context(format!(
        "No saved cookies at {}. Run 'tm3-spike login' first.",
        path.display()
    ))?;
    let cookies: Vec<SavedCookie> = serde_json::from_str(&json)?;
    eprintln!("[inspect] Loaded {} cookies from {}", cookies.len(), path.display());

    eprintln!("[inspect] Launching Chrome (headless)...");
    let browser = launch_browser(true)?;
    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(30));

    // Navigate to TM3 first (need domain context for cookies)
    eprintln!("[inspect] Navigating to TM3 to set cookie domain...");
    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(3));

    // Inject saved cookies
    eprintln!("[inspect] Injecting saved cookies...");
    for cookie in &cookies {
        let _ = tab.call_method(Network::SetCookie {
            name: cookie.name.clone(),
            value: cookie.value.clone(),
            url: None,
            domain: Some(cookie.domain.clone()),
            path: Some(cookie.path.clone()),
            secure: Some(cookie.secure),
            http_only: Some(cookie.http_only),
            same_site: None,
            expires: cookie.expires,
            priority: None,
            same_party: None,
            source_scheme: None,
            source_port: None,
            partition_key: None,
        });
    }

    // Reload page with cookies in place
    eprintln!("[inspect] Reloading with cookies...");
    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(5));

    let url = tab.get_url();
    eprintln!("[inspect] Post-cookie URL: {}", url);

    if url.contains("login") || url.contains("Login") {
        eprintln!("[inspect] Still on login page — cookies may have expired.");
        eprintln!("[inspect] Run 'tm3-spike login' to refresh cookies.");
        bail!("Session cookies expired. Re-run 'tm3-spike login'.");
    }

    eprintln!("[inspect] Authenticated via cookies!");

    // Direct URL navigation doesn't work — React SPA renders the shell but not the content.
    // Navigate like a human: use TM3's Quick Search to find the patient, then click Documents.
    eprintln!("[inspect] Using TM3 Quick Search to navigate to patient {}...", tm3_id);

    // Click the Quick Search button (⌘K)
    let search_clicked = tab.evaluate(
        r#"
        (function() {
            var buttons = document.querySelectorAll('button');
            for (var i = 0; i < buttons.length; i++) {
                if (buttons[i].textContent.includes('Quick search')) {
                    buttons[i].click();
                    return "clicked";
                }
            }
            // Fallback: trigger Cmd+K
            document.dispatchEvent(new KeyboardEvent('keydown', {key: 'k', metaKey: true, bubbles: true}));
            return "dispatched_key";
        })()
        "#,
        false,
    )?;
    if let Some(val) = &search_clicked.value {
        eprintln!("[inspect] Search trigger: {}", val);
    }
    std::thread::sleep(Duration::from_secs(2));

    // Type the patient ID into the search box
    let search_js = format!(
        r#"
        (function() {{
            // Find the search input that appeared
            var inputs = document.querySelectorAll('input[type="text"], input[type="search"], input:not([type])');
            var searchInput = null;
            for (var i = 0; i < inputs.length; i++) {{
                var el = inputs[i];
                var ph = (el.placeholder || '').toLowerCase();
                var cl = (el.className || '').toLowerCase();
                // The search input is likely the one that's visible and focused, or has search-related attributes
                if (ph.includes('search') || cl.includes('search') || el === document.activeElement) {{
                    searchInput = el;
                    break;
                }}
            }}
            if (!searchInput) {{
                // Try the last focused input
                searchInput = document.activeElement;
            }}
            if (searchInput && searchInput.tagName === 'INPUT') {{
                searchInput.value = '{}';
                searchInput.dispatchEvent(new Event('input', {{bubbles: true}}));
                searchInput.dispatchEvent(new Event('change', {{bubbles: true}}));
                return JSON.stringify({{found: true, placeholder: searchInput.placeholder, class: searchInput.className}});
            }}
            return JSON.stringify({{found: false, activeTag: document.activeElement ? document.activeElement.tagName : 'none'}});
        }})()
        "#,
        tm3_id
    );
    let search_result = tab.evaluate(&search_js, false)?;
    if let Some(val) = &search_result.value {
        eprintln!("[inspect] Search input: {}", val);
    }
    std::thread::sleep(Duration::from_secs(3));

    // Take a screenshot to see what the search shows
    let ss_path = config_dir().join("tm3-search.png");
    if let Ok(bytes) = tab.capture_screenshot(
        headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
        None, None, true,
    ) {
        std::fs::write(&ss_path, &bytes)?;
        eprintln!("[inspect] Search screenshot: {}", ss_path.display());
    }

    // Dump what's visible after search
    let search_results = tab.evaluate(
        r#"
        (function() {
            // Look for search results — could be a dropdown, list, or overlay
            var allText = document.body.innerText;
            var links = document.querySelectorAll('a[href*="Patient"], a[href*="patient"]');
            var results = Array.from(links).map(function(a) {
                return {href: a.href, text: a.textContent.trim().substring(0, 80)};
            });

            // Also check for any new overlays/modals/dropdowns
            var overlays = document.querySelectorAll('[class*="modal" i], [class*="overlay" i], [class*="dropdown" i], [class*="popover" i], [class*="search" i][class*="result" i], [role="listbox"], [role="dialog"]');
            var overlayInfo = Array.from(overlays).map(function(el) {
                return {
                    tag: el.tagName, class: el.className.substring(0, 80),
                    text: el.innerText.trim().substring(0, 200),
                    visible: el.offsetParent !== null || getComputedStyle(el).display !== 'none'
                };
            });

            return JSON.stringify({
                patientLinks: results,
                overlays: overlayInfo,
                bodySnippet: allText.substring(0, 400)
            }, null, 2);
        })()
        "#,
        false,
    )?;
    eprintln!("[inspect] Search results:");
    if let Some(val) = search_results.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("{}", s);
    }

    // Give user a chance to see the search screenshot and understand the state
    eprintln!("[inspect] Check tm3-search.png for visual state.");

    // Take a screenshot to see what we're looking at
    let screenshot_path = config_dir().join("tm3-screenshot.png");
    match tab.capture_screenshot(
        headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
        None, None, true,
    ) {
        Ok(bytes) => {
            std::fs::write(&screenshot_path, &bytes)?;
            eprintln!("[inspect] Screenshot saved: {}", screenshot_path.display());
        }
        Err(e) => eprintln!("[inspect] Screenshot failed: {}", e),
    }

    // Capture console errors
    let errors = tab.evaluate(
        r#"
        (function() {
            // Check if there's a visible error message on the page
            var body = document.body ? document.body.innerText.trim().substring(0, 500) : "(no body)";
            var reactRoot = document.getElementById('root') || document.getElementById('app')
                || document.querySelector('[data-reactroot]') || document.querySelector('#__next');
            var rootInfo = reactRoot ? {
                tag: reactRoot.tagName, id: reactRoot.id,
                childCount: reactRoot.childElementCount,
                text: reactRoot.innerText.trim().substring(0, 200)
            } : null;

            return JSON.stringify({
                bodyLength: body.length,
                bodyPreview: body.substring(0, 300),
                reactRoot: rootInfo,
                readyState: document.readyState,
                title: document.title
            }, null, 2);
        })()
        "#,
        false,
    )?;

    eprintln!("[inspect] Page content inspection:");
    if let Some(val) = errors.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("{}", s);
    }

    // Inspect upload widget
    inspect_upload_widget(&tab)?;

    // Final state
    eprintln!("\n[inspect] === FINAL PAGE STATE ===");
    dump_final_state(&tab)?;

    eprintln!("\n[inspect] Done.");
    Ok(())
}

// --- Cookie helpers ---

fn save_cookies(tab: &Tab) -> Result<()> {
    eprintln!("[login] Capturing cookies...");

    let cookie_data = tab.evaluate(
        r#"
        (function() {
            // document.cookie only gives us non-httpOnly cookies
            // We'll get what we can — the CDP method below gets the full set
            return document.cookie;
        })()
        "#,
        false,
    )?;

    if let Some(val) = &cookie_data.value {
        let cookie_str = val.as_str().unwrap_or("");
        eprintln!("[login] document.cookie length: {} chars", cookie_str.len());
    }

    // Use CDP to get ALL cookies (including httpOnly)
    let cdp_cookies = tab.call_method(Network::GetCookies {
        urls: Some(vec![
            TM3_BASE.to_string(),
            format!("{}/", TM3_BASE),
        ]),
    })?;

    let saved: Vec<SavedCookie> = cdp_cookies
        .cookies
        .iter()
        .map(|c| SavedCookie {
            name: c.name.clone(),
            value: c.value.clone(),
            domain: c.domain.clone(),
            path: c.path.clone(),
            secure: c.secure,
            http_only: c.http_only,
            expires: if c.expires > 0.0 {
                Some(c.expires)
            } else {
                None
            },
        })
        .collect();

    eprintln!("[login] Captured {} cookies:", saved.len());
    for c in &saved {
        eprintln!(
            "  {} (domain: {}, httpOnly: {}, secure: {}, expires: {})",
            c.name,
            c.domain,
            c.http_only,
            c.secure,
            c.expires
                .map(|e| {
                    let secs = e as u64;
                    let dt = std::time::UNIX_EPOCH + Duration::from_secs(secs);
                    format!("{:?}", dt)
                })
                .unwrap_or_else(|| "session".to_string())
        );
    }

    let path = cookie_path();
    let json = serde_json::to_string_pretty(&saved)?;
    std::fs::write(&path, &json)?;
    eprintln!("[login] Cookies saved to: {}", path.display());

    Ok(())
}

// --- Shared helpers ---

fn launch_browser(headless: bool) -> Result<Browser> {
    Browser::new(
        LaunchOptions::default_builder()
            .headless(headless)
            .window_size(Some((1280, 900)))
            .idle_browser_timeout(Duration::from_secs(120))
            .build()
            .context("Failed to build launch options")?,
    )
    .context("Failed to launch Chrome")
}

fn wait_for_auth(tab: &Tab) -> Result<()> {
    for attempt in 0..60 {
        std::thread::sleep(Duration::from_secs(2));
        let current_url = tab.get_url();

        if !current_url.contains("login") && !current_url.contains("Login") {
            return Ok(());
        }

        if attempt % 5 == 4 {
            eprintln!(
                "[auth] Waiting for login... ({}s)",
                (attempt + 1) * 2
            );
        }
    }
    bail!("Timed out waiting for authentication (120s).")
}

fn wait_for_page_load(tab: &Tab) -> Result<()> {
    for i in 0..15 {
        std::thread::sleep(Duration::from_secs(2));
        let title = tab
            .evaluate("document.title", false)
            .ok()
            .and_then(|r| r.value)
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();

        if !title.contains("Loading") && !title.is_empty() {
            eprintln!("[inspect] Page loaded ({}s). Title: {}", (i + 1) * 2, title);
            return Ok(());
        }
    }
    eprintln!("[inspect] Page may still be loading after 30s — proceeding anyway.");
    Ok(())
}

fn inspect_upload_widget(tab: &Arc<Tab>) -> Result<()> {
    eprintln!("[inspect] Inspecting upload widget DOM...");

    let widget_info = tab.evaluate(
        r#"
        (function() {
            var info = {};

            var iframes = document.querySelectorAll('iframe');
            info.iframes = Array.from(iframes).map(function(el) {
                return { src: el.src, id: el.id, name: el.name };
            });

            var fileInputs = document.querySelectorAll('input[type="file"]');
            info.fileInputs = Array.from(fileInputs).map(function(el) {
                var rect = el.getBoundingClientRect();
                return {
                    name: el.name, id: el.id, accept: el.accept,
                    multiple: el.multiple, class: el.className,
                    visible: el.offsetParent !== null,
                    width: rect.width, height: rect.height
                };
            });

            // CSS selector search
            var uploadTriggers = document.querySelectorAll(
                'button[class*="upload" i], button[class*="attach" i], ' +
                'a[class*="upload" i], a[class*="attach" i], ' +
                '[role="button"][class*="upload" i], ' +
                '.k-upload, .k-dropzone, .kendo-upload, ' +
                '.dz-clickable, .dropzone, ' +
                '.fine-uploader, .qq-upload-button, ' +
                '[class*="file" i][class*="select" i], ' +
                '[class*="browse" i]'
            );
            info.uploadTriggers = Array.from(uploadTriggers).map(function(el) {
                return {
                    tag: el.tagName, id: el.id, class: el.className,
                    text: el.textContent.trim().substring(0, 80),
                    ariaLabel: el.getAttribute('aria-label')
                };
            });

            // Text-content search for upload-related buttons
            var allButtons = document.querySelectorAll('button, a');
            info.uploadButtons = Array.from(allButtons)
                .filter(function(el) {
                    var t = el.textContent.trim().toLowerCase();
                    return t.includes('upload') || t.includes('attach') || t.includes('browse')
                        || t.includes('choose file') || t.includes('add document')
                        || t.includes('new document') || t.includes('add file');
                })
                .map(function(el) {
                    return {
                        tag: el.tagName, id: el.id, class: el.className,
                        text: el.textContent.trim().substring(0, 80)
                    };
                });

            info.libraries = {
                kendo: typeof kendo !== 'undefined',
                kendoVersion: typeof kendo !== 'undefined' ? kendo.version : null,
                Dropzone: typeof Dropzone !== 'undefined',
                jQuery: typeof jQuery !== 'undefined',
                jQueryVersion: typeof jQuery !== 'undefined' ? jQuery.fn.jquery : null,
                angular: typeof angular !== 'undefined',
                React: typeof React !== 'undefined' || !!document.querySelector('[data-reactroot]')
            };

            return JSON.stringify(info, null, 2);
        })()
        "#,
        false,
    )?;

    eprintln!("[inspect] Upload widget DOM:");
    if let Some(val) = widget_info.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("{}", s);
    }

    Ok(())
}

fn dump_final_state(tab: &Tab) -> Result<()> {
    let info = tab.evaluate(
        r#"
        (function() {
            var info = {};
            info.url = window.location.href;
            info.title = document.title;

            var fileInputs = document.querySelectorAll('input[type="file"]');
            info.fileInputs = Array.from(fileInputs).map(function(el) {
                return {
                    name: el.name, id: el.id, accept: el.accept,
                    multiple: el.multiple, class: el.className,
                    visible: el.offsetParent !== null
                };
            });

            var uploadElements = document.querySelectorAll(
                '[class*="upload" i], [id*="upload" i], [class*="drop" i]'
            );
            info.uploadElements = Array.from(uploadElements).map(function(el) {
                return {
                    tag: el.tagName, id: el.id, class: el.className,
                    text: el.textContent.trim().substring(0, 100)
                };
            });

            // Get ALL buttons on the page for full picture
            var buttons = document.querySelectorAll('button');
            info.allButtons = Array.from(buttons).map(function(el) {
                return {
                    text: el.textContent.trim().substring(0, 60),
                    class: el.className.substring(0, 80)
                };
            });

            return JSON.stringify(info, null, 2);
        })()
        "#,
        false,
    )?;

    if let Some(val) = info.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("{}", s);
    }

    Ok(())
}
