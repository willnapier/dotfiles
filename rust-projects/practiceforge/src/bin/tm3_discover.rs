//! TM3 diary API discovery — intercepts fetch/XHR while you manually create an
//! appointment in the visible browser, then saves all captured calls to JSON.
//!
//! This is the recon step required before implementing write-back to TM3.
//! Run this on Mac where TM3 session cookies are available.
//!
//! Usage:
//!   tm3-discover
//!
//! Output:
//!   ~/.config/practiceforge/tm3-diary-api.json

use anyhow::{bail, Context, Result};
use headless_chrome::protocol::cdp::Network;
use headless_chrome::{Browser, LaunchOptions};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const TM3_BASE: &str = "https://changeofharleystreet.tm3app.com";
const KEYCHAIN_SERVICE: &str = "tm3-session";
const KEYCHAIN_ACCOUNT: &str = "changeofharleystreet";
const OUTPUT_FILE: &str = "tm3-diary-api.json";

#[derive(Serialize, Deserialize, Debug, Clone)]
struct StoredCookie {
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
    let dir = PathBuf::from(home).join(".config").join("practiceforge");
    std::fs::create_dir_all(&dir).ok();
    dir
}

fn load_cookies() -> Result<Vec<StoredCookie>> {
    // 1. Local keychain (macOS)
    if cfg!(target_os = "macos") {
        let out = Command::new("security")
            .args([
                "find-generic-password",
                "-s", KEYCHAIN_SERVICE,
                "-a", KEYCHAIN_ACCOUNT,
                "-w",
            ])
            .output();
        if let Ok(o) = out {
            if o.status.success() {
                let json = String::from_utf8(o.stdout)?.trim().to_string();
                if let Ok(c) = serde_json::from_str::<Vec<StoredCookie>>(&json) {
                    eprintln!("[cookies] Loaded {} cookies from macOS keychain.", c.len());
                    return Ok(c);
                }
            }
        }
    } else {
        // Linux: secret-tool
        let out = Command::new("secret-tool")
            .args(["lookup", "service", KEYCHAIN_SERVICE, "account", KEYCHAIN_ACCOUNT])
            .output();
        if let Ok(o) = out {
            if o.status.success() {
                let json = String::from_utf8(o.stdout)?.trim().to_string();
                if let Ok(c) = serde_json::from_str::<Vec<StoredCookie>>(&json) {
                    eprintln!("[cookies] Loaded {} cookies from secret-service.", c.len());
                    return Ok(c);
                }
            }
        }
    }

    // 2. Syncthing shared file
    let shared = dirs::home_dir()
        .unwrap_or_default()
        .join("Assistants")
        .join("shared")
        .join(".tm3-session-cookies.json");
    if shared.exists() {
        let json = std::fs::read_to_string(&shared)?;
        let c: Vec<StoredCookie> = serde_json::from_str(json.trim())?;
        eprintln!("[cookies] Loaded {} cookies from shared file.", c.len());
        return Ok(c);
    }

    // 3. SSH to Mac
    let out = Command::new("ssh")
        .args([
            "-o", "ConnectTimeout=5",
            "mac",
            &format!(
                "security find-generic-password -s '{}' -a '{}' -w",
                KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT
            ),
        ])
        .output();
    if let Ok(o) = out {
        if o.status.success() {
            let json = String::from_utf8(o.stdout)?.trim().to_string();
            if let Ok(c) = serde_json::from_str::<Vec<StoredCookie>>(&json) {
                eprintln!("[cookies] Loaded {} cookies via SSH from Mac keychain.", c.len());
                return Ok(c);
            }
        }
    }

    bail!(
        "No TM3 session cookies found.\n\
         Options:\n\
         - On Mac: run 'tm3-spike login' to authenticate and store cookies\n\
         - Ensure SSH to Mac is working: ssh mac 'echo ok'"
    )
}

fn inject_cookies(
    tab: &headless_chrome::browser::tab::Tab,
    cookies: &[StoredCookie],
) -> Result<()> {
    for c in cookies {
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
    Ok(())
}

/// JavaScript injected into the page to intercept all fetch + XHR calls.
const INTERCEPT_JS: &str = r#"
(function() {
    if (window.__tm3DiscoverActive) { return 'already_active'; }
    window.__tm3DiscoverActive = true;
    window.__tm3Requests = [];

    // --- fetch ---
    const _fetch = window.fetch;
    window.fetch = async function(input, init) {
        const url = (typeof input === 'string') ? input
                  : (input && input.url) ? input.url : String(input);
        const method = (init && init.method) ? init.method.toUpperCase() : 'GET';
        let body = null;
        if (init && init.body !== undefined && init.body !== null) {
            try { body = String(init.body); } catch(e) { body = '[unreadable]'; }
        }
        let headers = {};
        if (init && init.headers) {
            try {
                if (typeof init.headers.forEach === 'function') {
                    init.headers.forEach((v, k) => { headers[k] = v; });
                } else {
                    Object.assign(headers, init.headers);
                }
            } catch(e) {}
        }
        window.__tm3Requests.push({ type:'fetch', method, url, body, headers, ts: Date.now() });
        return _fetch.apply(this, arguments);
    };

    // --- XMLHttpRequest ---
    const _open = XMLHttpRequest.prototype.open;
    const _setHeader = XMLHttpRequest.prototype.setRequestHeader;
    const _send = XMLHttpRequest.prototype.send;

    XMLHttpRequest.prototype.open = function(method, url) {
        this.__m = method ? method.toUpperCase() : 'GET';
        this.__u = url || '';
        this.__h = {};
        return _open.apply(this, arguments);
    };
    XMLHttpRequest.prototype.setRequestHeader = function(k, v) {
        if (!this.__h) this.__h = {};
        this.__h[k] = v;
        return _setHeader.apply(this, arguments);
    };
    XMLHttpRequest.prototype.send = function(body) {
        let b = null;
        if (body !== null && body !== undefined) {
            try { b = String(body); } catch(e) { b = '[unreadable]'; }
        }
        window.__tm3Requests.push({
            type: 'xhr',
            method: this.__m || 'GET',
            url: this.__u || '',
            body: b,
            headers: this.__h || {},
            ts: Date.now()
        });
        return _send.apply(this, arguments);
    };

    return 'intercept_active';
})()
"#;

fn main() -> Result<()> {
    eprintln!("╔══════════════════════════════════════════════╗");
    eprintln!("║     TM3 Diary API Discovery Tool             ║");
    eprintln!("╚══════════════════════════════════════════════╝");
    eprintln!();

    let cookies = load_cookies()?;

    eprintln!("[discover] Launching Chrome (visible)...");
    let browser = Browser::new(
        LaunchOptions::default_builder()
            .headless(false)
            .window_size(Some((1400, 900)))
            .idle_browser_timeout(Duration::from_secs(600))
            .build()
            .context("Failed to build Chrome options")?,
    )
    .context("Failed to launch Chrome — is Chrome/Chromium installed?")?;

    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(30));

    // Navigate to TM3 to establish domain context before injecting cookies
    eprintln!("[discover] Navigating to TM3...");
    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(3));

    eprintln!("[discover] Injecting session cookies...");
    inject_cookies(&tab, &cookies)?;

    // Reload so cookies take effect
    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(5));

    let current_url = tab.get_url();
    if current_url.contains("login") || current_url.contains("Login") {
        bail!(
            "Still on login page — cookies have expired.\n\
             Run: tm3-spike login"
        );
    }
    eprintln!("[discover] Authenticated. URL: {}", current_url);

    // Inject intercept script
    eprintln!("[discover] Injecting API intercept...");
    let intercept_result = tab.evaluate(INTERCEPT_JS, false)?;
    eprintln!(
        "[discover] Intercept status: {}",
        intercept_result
            .value
            .as_ref()
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
    );

    // Try to navigate to the diary view
    eprintln!("[discover] Attempting to navigate to diary...");
    let nav_js = r#"
        (function() {
            var tried = [];
            var paths = ['/Diary', '/diary', '/Calendar', '/Appointments', '/Schedule'];
            // Push to first candidate and let React Router handle it
            window.history.pushState({}, '', paths[0]);
            window.dispatchEvent(new PopStateEvent('popstate', {state: {}}));
            return window.location.pathname;
        })()
    "#;
    tab.evaluate(nav_js, false).ok();
    std::thread::sleep(Duration::from_secs(3));

    eprintln!();
    eprintln!("┌──────────────────────────────────────────────────────────┐");
    eprintln!("│  INSTRUCTIONS                                            │");
    eprintln!("│                                                          │");
    eprintln!("│  1. You should see TM3 in the browser window.           │");
    eprintln!("│     If the diary isn't visible, navigate there now.     │");
    eprintln!("│                                                          │");
    eprintln!("│  2. CREATE a test appointment:                          │");
    eprintln!("│     - Click an empty time slot in the diary             │");
    eprintln!("│     - Fill in client, time, appointment type            │");
    eprintln!("│     - Click Save / Confirm                              │");
    eprintln!("│     (You can delete it afterwards)                      │");
    eprintln!("│                                                          │");
    eprintln!("│  3. OPTIONALLY also try:                                │");
    eprintln!("│     - Editing an appointment (reschedule)               │");
    eprintln!("│     - Marking attendance (Attended / DNA / Cancelled)   │");
    eprintln!("│                                                          │");
    eprintln!("│  4. Come back here and press Enter when done.           │");
    eprintln!("└──────────────────────────────────────────────────────────┘");
    eprintln!();

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    // Collect captured requests
    eprintln!("[discover] Collecting captured requests...");
    let raw = tab
        .evaluate("JSON.stringify(window.__tm3Requests || [])", false)
        .context("Failed to read captured requests from browser")?;

    let json_str = raw
        .value
        .as_ref()
        .and_then(|v| v.as_str())
        .unwrap_or("[]");

    let all: Vec<serde_json::Value> = serde_json::from_str(json_str).unwrap_or_default();
    eprintln!("[discover] Total captured: {}", all.len());

    // Filter to write operations and anything hitting an API path
    let interesting: Vec<&serde_json::Value> = all
        .iter()
        .filter(|r| {
            let method = r.get("method").and_then(|v| v.as_str()).unwrap_or("GET");
            let url = r.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let is_mutation = matches!(method, "POST" | "PUT" | "PATCH" | "DELETE");
            let is_api = url.contains("/api/")
                || url.contains("/Api/")
                || url.contains("appointment")
                || url.contains("Appointment")
                || url.contains("diary")
                || url.contains("Diary")
                || url.contains("calendar")
                || url.contains("Calendar")
                || url.contains("event")
                || url.contains("Event");
            is_mutation || is_api
        })
        .collect();

    eprintln!("[discover] Interesting (write ops / diary API): {}", interesting.len());
    eprintln!();

    if interesting.is_empty() {
        eprintln!("WARNING: No interesting requests captured.");
        eprintln!("  - Make sure you actually saved/submitted an appointment");
        eprintln!("  - The intercept was injected after page load, so calls made");
        eprintln!("    before that point were not captured");
        eprintln!("  - Try re-running and performing actions after the browser opens");
    } else {
        eprintln!("Captured API calls:");
        for (i, req) in interesting.iter().enumerate() {
            let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("?");
            let url = req.get("url").and_then(|v| v.as_str()).unwrap_or("?");
            let body_preview = req
                .get("body")
                .and_then(|v| v.as_str())
                .map(|s| {
                    if s.len() > 120 {
                        format!("{}...", &s[..120])
                    } else {
                        s.to_string()
                    }
                })
                .unwrap_or_else(|| "(no body)".to_string());
            eprintln!("  [{:2}] {} {}", i + 1, method, url);
            eprintln!("       body: {}", body_preview);
            eprintln!();
        }
    }

    // Save to file
    let output_path = config_dir().join(OUTPUT_FILE);
    let output = serde_json::json!({
        "captured_at": chrono::Local::now().to_rfc3339(),
        "total_requests": all.len(),
        "interesting_count": interesting.len(),
        "all_requests": all,
        "interesting_requests": interesting,
    });
    std::fs::write(&output_path, serde_json::to_string_pretty(&output)?)?;

    eprintln!("[discover] Full capture saved to:");
    eprintln!("  {}", output_path.display());
    eprintln!();
    eprintln!("Next step: share the JSON with Nimbini or review it to identify");
    eprintln!("the appointment creation/update endpoints.");

    Ok(())
}
