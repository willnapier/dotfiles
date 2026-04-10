//! Healthcode login page reconnaissance.
//!
//! Inspects the login page at auth.healthcode.co.uk to discover:
//! - Login form structure (fields, selectors)
//! - Auth method (username/password, SSO, passkey?)
//! - Post-login navigation patterns
//!
//! Usage:
//!   healthcode-spike recon              — inspect login page (no credentials needed)
//!   healthcode-spike login              — manual login + cookie capture to keychain
//!   healthcode-spike inspect            — load cookies, inspect the portal

use anyhow::{bail, Context, Result};
use headless_chrome::browser::tab::Tab;
use headless_chrome::protocol::cdp::Network;
use headless_chrome::{Browser, LaunchOptions};
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::time::Duration;

const HC_LOGIN: &str = "https://auth.healthcode.co.uk/login";
const HC_BASE: &str = "https://auth.healthcode.co.uk";
const KEYCHAIN_SERVICE: &str = "healthcode-session";
const KEYCHAIN_ACCOUNT: &str = "healthcode";

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

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(|s| s.as_str()).unwrap_or("recon");

    match mode {
        "recon" => cmd_recon(),
        "login" => cmd_login(),
        "inspect" => cmd_inspect(),
        other => bail!("Unknown mode '{}'. Use recon, login, or inspect.", other),
    }
}

fn cmd_recon() -> Result<()> {
    eprintln!("[recon] Launching Chrome (headless)...");
    let browser = launch_browser(true)?;
    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(30));

    eprintln!("[recon] Navigating to Healthcode login...");
    tab.navigate_to(HC_LOGIN)?;
    std::thread::sleep(Duration::from_secs(5));

    let url = tab.get_url();
    eprintln!("[recon] Landed at: {}", url);

    // Dump full page structure
    let info = tab.evaluate(
        r#"
        (function() {
            var info = {};
            info.url = window.location.href;
            info.title = document.title;

            var inputs = document.querySelectorAll('input, select, textarea');
            info.inputs = Array.from(inputs).map(function(el) {
                return {
                    tag: el.tagName, type: el.type, name: el.name, id: el.id,
                    placeholder: el.placeholder || '', class: el.className,
                    required: el.required, autocomplete: el.autocomplete || ''
                };
            });

            var forms = document.querySelectorAll('form');
            info.forms = Array.from(forms).map(function(el) {
                return {action: el.action, method: el.method, id: el.id, class: el.className};
            });

            var buttons = document.querySelectorAll('button, input[type="submit"], a.btn, [role="button"]');
            info.buttons = Array.from(buttons).map(function(el) {
                return {
                    tag: el.tagName, type: el.type || '',
                    text: el.textContent.trim().substring(0, 80),
                    id: el.id, class: el.className, href: el.href || ''
                };
            });

            var links = document.querySelectorAll('a[href]');
            info.links = Array.from(links).map(function(a) {
                return {href: a.href, text: a.textContent.trim().substring(0, 60)};
            }).filter(function(l) { return l.text.length > 0; });

            // Check for OAuth/SSO indicators
            info.oauth = {
                hasGoogleAuth: !!document.querySelector('[class*="google" i], [id*="google" i], a[href*="google"]'),
                hasMicrosoftAuth: !!document.querySelector('[class*="microsoft" i], [id*="microsoft" i], a[href*="microsoft"]'),
                hasSSO: !!document.querySelector('[class*="sso" i], [id*="sso" i], a[href*="sso"]'),
                hasPasskey: !!document.querySelector('[class*="passkey" i], [class*="webauthn" i]')
            };

            // Check page text for auth-related content
            var bodyText = document.body ? document.body.innerText.substring(0, 1000) : '';
            info.bodyPreview = bodyText;

            // Check for captcha
            info.hasCaptcha = !!document.querySelector('[class*="captcha" i], [id*="captcha" i], iframe[src*="recaptcha"], iframe[src*="hcaptcha"]');

            return JSON.stringify(info, null, 2);
        })()
        "#,
        false,
    )?;

    eprintln!("[recon] Login page structure:");
    if let Some(val) = info.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("{}", s);
    }

    // Take screenshot
    let ss_path = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".config/clinical-product/healthcode-login.png");
    std::fs::create_dir_all(ss_path.parent().unwrap()).ok();
    if let Ok(bytes) = tab.capture_screenshot(
        headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
        None, None, true,
    ) {
        std::fs::write(&ss_path, &bytes)?;
        eprintln!("[recon] Screenshot: {}", ss_path.display());
    }

    Ok(())
}

fn cmd_login() -> Result<()> {
    // Delete any existing session
    keychain_delete().ok();

    eprintln!("[login] Launching Chrome (visible)...");
    let browser = launch_browser(false)?;
    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(30));

    eprintln!("[login] Navigating to Healthcode login...");
    tab.navigate_to(HC_LOGIN)?;
    std::thread::sleep(Duration::from_secs(3));

    eprintln!();
    eprintln!("[login] ==========================================");
    eprintln!("[login]  Log in to Healthcode manually.           ");
    eprintln!("[login]  I'll capture cookies once you're in.    ");
    eprintln!("[login] ==========================================");
    eprintln!();

    // Wait for authentication — poll until URL changes away from login
    for attempt in 0..150 {
        std::thread::sleep(Duration::from_secs(2));
        let url = tab.get_url();
        if !url.contains("login") && !url.contains("Login") && !url.contains("auth.healthcode") {
            eprintln!("[login] Authenticated! URL: {}", url);
            break;
        }
        if attempt == 149 {
            bail!("Timed out waiting for login (5 min).");
        }
        if attempt % 10 == 9 {
            eprintln!("[login] Waiting... ({}s)", (attempt + 1) * 2);
        }
    }

    std::thread::sleep(Duration::from_secs(2));

    // Capture cookies from all healthcode domains
    let cookies = capture_cookies(&tab)?;
    let json = serde_json::to_string(&cookies)?;
    keychain_store(&json)?;
    eprintln!("[login] Session stored in keychain ({} cookies).", cookies.len());

    // Dump the post-login page structure
    eprintln!("[login] Post-login page inspection...");
    let post_info = tab.evaluate(
        r#"
        (function() {
            var info = {};
            info.url = window.location.href;
            info.title = document.title;

            var links = document.querySelectorAll('a[href]');
            info.navLinks = Array.from(links).map(function(a) {
                return {href: a.href, text: a.textContent.trim().substring(0, 60)};
            }).filter(function(l) {
                var h = l.href.toLowerCase();
                return l.text.length > 0 && (h.includes('claim') || h.includes('auth')
                    || h.includes('form') || h.includes('submit') || h.includes('patient')
                    || h.includes('axa') || h.includes('extension'));
            });

            var buttons = document.querySelectorAll('button, a.btn');
            info.actionButtons = Array.from(buttons).map(function(el) {
                return {text: el.textContent.trim().substring(0, 60), href: el.href || ''};
            }).filter(function(b) { return b.text.length > 0; });

            return JSON.stringify(info, null, 2);
        })()
        "#,
        false,
    )?;

    if let Some(val) = post_info.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("{}", s);
    }

    eprintln!("[login] Press Enter to close browser...");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    Ok(())
}

fn cmd_inspect() -> Result<()> {
    let cookies = load_cookies()?;
    eprintln!("[inspect] Loaded {} cookies from keychain.", cookies.len());

    let headless = std::env::var("HC_VISIBLE").is_err();
    let browser = launch_browser(headless)?;
    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(30));

    inject_cookies(&tab, &cookies)?;

    tab.navigate_to(HC_BASE)?;
    std::thread::sleep(Duration::from_secs(5));

    let url = tab.get_url();
    eprintln!("[inspect] Post-cookie URL: {}", url);

    if url.contains("login") {
        bail!("Session expired. Run 'healthcode-spike login' first.");
    }

    // Comprehensive portal inspection
    let info = tab.evaluate(
        r#"
        (function() {
            var info = {};
            info.url = window.location.href;
            info.title = document.title;
            info.bodyPreview = document.body ? document.body.innerText.substring(0, 2000) : '';

            var links = document.querySelectorAll('a[href]');
            info.allLinks = Array.from(links).map(function(a) {
                return {href: a.href, text: a.textContent.trim().substring(0, 80)};
            }).filter(function(l) { return l.text.length > 0; }).slice(0, 50);

            var buttons = document.querySelectorAll('button, input[type="submit"], a.btn');
            info.allButtons = Array.from(buttons).map(function(el) {
                return {text: el.textContent.trim().substring(0, 60), class: el.className.substring(0, 60)};
            }).filter(function(b) { return b.text.length > 0; });

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

    if !headless {
        eprintln!("[inspect] Press Enter to close...");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
    }

    Ok(())
}

// --- Helpers ---

fn launch_browser(headless: bool) -> Result<Browser> {
    Browser::new(
        LaunchOptions::default_builder()
            .headless(headless)
            .window_size(Some((1280, 900)))
            .idle_browser_timeout(Duration::from_secs(600))
            .build()?,
    )
    .context("Failed to launch Chrome")
}

fn capture_cookies(tab: &Tab) -> Result<Vec<StoredCookie>> {
    let cdp_cookies = tab.call_method(Network::GetCookies {
        urls: Some(vec![
            "https://auth.healthcode.co.uk".to_string(),
            "https://healthcode.co.uk".to_string(),
            "https://www.healthcode.co.uk".to_string(),
        ]),
    })?;

    Ok(cdp_cookies.cookies.iter().map(|c| StoredCookie {
        name: c.name.clone(),
        value: c.value.clone(),
        domain: c.domain.clone(),
        path: c.path.clone(),
        secure: c.secure,
        http_only: c.http_only,
        expires: if c.expires > 0.0 { Some(c.expires) } else { None },
    }).collect())
}

fn inject_cookies(tab: &Tab, cookies: &[StoredCookie]) -> Result<()> {
    tab.navigate_to(HC_BASE)?;
    std::thread::sleep(Duration::from_secs(2));
    for cookie in cookies {
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
    Ok(())
}

fn keychain_store(data: &str) -> Result<()> {
    Command::new("security")
        .args(["add-generic-password", "-s", KEYCHAIN_SERVICE, "-a", KEYCHAIN_ACCOUNT, "-w", data, "-U"])
        .status()
        .context("Failed to store in keychain")?;
    Ok(())
}

fn keychain_delete() -> Result<()> {
    Command::new("security")
        .args(["delete-generic-password", "-s", KEYCHAIN_SERVICE, "-a", KEYCHAIN_ACCOUNT])
        .output()?;
    Ok(())
}

fn load_cookies() -> Result<Vec<StoredCookie>> {
    let output = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-a", KEYCHAIN_ACCOUNT, "-w"])
        .output()
        .context("Failed to read keychain")?;
    if !output.status.success() {
        bail!("No Healthcode session in keychain. Run 'healthcode-spike login' first.");
    }
    let json = String::from_utf8(output.stdout)?.trim().to_string();
    Ok(serde_json::from_str(&json)?)
}
