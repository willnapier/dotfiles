//! Healthcode insurer portal automation.
//!
//! Subcommands:
//!   healthcode_spike recon              — inspect login page (no credentials needed)
//!   healthcode_spike login              — manual login + cookie capture to keychain
//!   healthcode_spike inspect            — load cookies, inspect the portal
//!   healthcode_spike auto-login          — automated login with TOTP support
//!   healthcode_spike totp-test            — verify TOTP secret is correct
//!   healthcode_spike fill <client_id>   — fill authorisation form from clinical data
//!   healthcode_spike explore             — navigate into ePractice and map pages/forms

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use headless_chrome::browser::tab::Tab;
use headless_chrome::protocol::cdp::Network;
use headless_chrome::{Browser, LaunchOptions};
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "healthcode", about = "Healthcode insurer portal automation")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Inspect the login page structure (no credentials needed)
    Recon,
    /// Manual login — opens visible browser, captures session cookies
    Login,
    /// Load cookies and inspect the authenticated portal
    Inspect,
    /// Automated login using stored credentials, captures session cookies
    AutoLogin,
    /// Test TOTP code generation (verify secret is correct)
    TotpTest,
    /// Navigate into ePractice and map all available pages/forms
    Explore,
    /// Fill an authorisation form from clinical data
    Fill {
        /// Client ID (used to get form data via `clinical auth form <id>`)
        client_id: String,
        /// Don't submit — just populate and screenshot
        #[arg(long)]
        dry_run: bool,
    },
}

const HC_LOGIN: &str = "https://auth.healthcode.co.uk/login";
const HC_BASE: &str = "https://auth.healthcode.co.uk";
const KEYCHAIN_SERVICE: &str = "healthcode-session";
const KEYCHAIN_ACCOUNT: &str = "healthcode";
const CRED_SERVICE: &str = "healthcode-login";
const TOTP_SERVICE: &str = "healthcode-totp";
const TOTP_ACCOUNT: &str = "00QHY2BID";

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
    let cli = Cli::parse();
    match cli.command {
        Cmd::Recon => cmd_recon(),
        Cmd::Login => cmd_login(),
        Cmd::Inspect => cmd_inspect(),
        Cmd::AutoLogin => cmd_auto_login(),
        Cmd::TotpTest => cmd_totp_test(),
        Cmd::Explore => cmd_explore(),
        Cmd::Fill { client_id, dry_run } => cmd_fill(&client_id, dry_run),
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
        .join(".config/practiceforge/healthcode-login.png");
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

fn load_credentials() -> Result<(String, String)> {
    // Username is the account name in the keychain entry
    let username = "00QHY2BID".to_string();

    let output = if cfg!(target_os = "macos") {
        Command::new("security")
            .args(["find-generic-password", "-s", CRED_SERVICE, "-a", &username, "-w"])
            .output()
            .context("Failed to read credentials from macOS keychain")?
    } else {
        Command::new("secret-tool")
            .args(["lookup", "service", CRED_SERVICE, "account", &username])
            .output()
            .context("Failed to read credentials from secret-service")?
    };

    if !output.status.success() {
        bail!(
            "Healthcode credentials not found. Store them first:\n  \
             macOS: security add-generic-password -s {} -a {} -w <password>\n  \
             Linux: echo '<password>' | secret-tool store --label Healthcode service {} account {}",
            CRED_SERVICE, username, CRED_SERVICE, username
        );
    }

    let password = String::from_utf8(output.stdout)?.trim().to_string();
    Ok((username, password))
}

fn cmd_auto_login() -> Result<()> {
    // Delete any existing session
    keychain_delete().ok();

    let (username, password) = load_credentials()?;
    eprintln!("[auto-login] Credentials loaded for {}", username);

    eprintln!("[auto-login] Launching Chrome (visible for review)...");
    let browser = launch_browser(false)?;
    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(30));

    eprintln!("[auto-login] Navigating to Healthcode login...");
    tab.navigate_to(HC_LOGIN)?;
    std::thread::sleep(Duration::from_secs(5));

    // Inspect login page and fill credentials
    eprintln!("[auto-login] Filling credentials...");

    let escaped_user = username.replace('\\', "\\\\").replace('"', "\\\"");
    let escaped_pass = password.replace('\\', "\\\\").replace('"', "\\\"");

    // Fill username field (name="username", id="j_username")
    let user_filled = tab.evaluate(&format!(r#"
        (function() {{
            var el = document.getElementById('j_username')
                  || document.querySelector('input[name="username"]');
            if (el) {{
                el.value = "{escaped_user}";
                el.dispatchEvent(new Event('input', {{bubbles: true}}));
                el.dispatchEvent(new Event('change', {{bubbles: true}}));
                return el.id || el.name;
            }}
            return null;
        }})()
    "#), false)?;

    if let Some(val) = &user_filled.value {
        if val.is_null() {
            eprintln!("[auto-login] WARNING: Could not find username field");
        } else {
            eprintln!("[auto-login] Username filled (field: {})", val);
        }
    }

    // Fill BOTH password fields: visible (id="password") and hidden (id="j_password").
    // Healthcode's JS copies visible→hidden on keypress, but programmatic .value=
    // skips that. We fill both explicitly and fire all relevant events.
    let pass_filled = tab.evaluate(&format!(r#"
        (function() {{
            var visible = document.getElementById('password');
            var hidden = document.getElementById('j_password');
            var filled = [];
            if (visible) {{
                visible.value = "{escaped_pass}";
                visible.dispatchEvent(new Event('input', {{bubbles: true}}));
                visible.dispatchEvent(new Event('change', {{bubbles: true}}));
                visible.dispatchEvent(new Event('keyup', {{bubbles: true}}));
                filled.push('password');
            }}
            if (hidden) {{
                hidden.value = "{escaped_pass}";
                hidden.dispatchEvent(new Event('input', {{bubbles: true}}));
                hidden.dispatchEvent(new Event('change', {{bubbles: true}}));
                filled.push('j_password');
            }}
            return filled.length > 0 ? filled.join('+') : null;
        }})()
    "#), false)?;

    if let Some(val) = &pass_filled.value {
        if val.is_null() {
            eprintln!("[auto-login] WARNING: Could not find password field");
        } else {
            eprintln!("[auto-login] Password filled (field: {})", val);
        }
    }

    // Click the Login button (id="form_submit", type="button" — JS-driven,
    // not a standard form submit)
    eprintln!("[auto-login] Submitting...");
    let _ = tab.evaluate(r#"
        (function() {
            var el = document.getElementById('form_submit');
            if (el) { el.click(); return 'form_submit'; }
            // Fallback: any button with Login text
            var buttons = document.querySelectorAll('button');
            for (var i = 0; i < buttons.length; i++) {
                if (buttons[i].textContent.trim().toLowerCase() === 'login') {
                    buttons[i].click();
                    return buttons[i].id || 'login-button';
                }
            }
            return null;
        })()
    "#, false)?;

    // Wait for page to respond after submit
    std::thread::sleep(Duration::from_secs(3));

    // Check if we hit an MFA/TOTP page
    let page_text = tab
        .evaluate(
            "document.body.innerText.substring(0, 500)",
            false,
        )
        .ok()
        .and_then(|r| r.value)
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();

    if page_text.to_lowercase().contains("authenticator")
        || page_text.to_lowercase().contains("verification code")
        || page_text.to_lowercase().contains("two-factor")
        || page_text.to_lowercase().contains("2fa")
        || page_text.to_lowercase().contains("one-time")
    {
        eprintln!("[auto-login] MFA/TOTP prompt detected. Generating code...");

        match load_totp_secret() {
            Ok(secret) => {
                let code = generate_totp_code(&secret)?;
                eprintln!("[auto-login] TOTP code generated ({}**)", &code[..2]);

                // Fill the TOTP field
                let escaped_code = code.replace('"', "\\\"");
                tab.evaluate(
                    &format!(
                        r#"
                    (function() {{
                        var selectors = [
                            'input[name*="code" i]', 'input[name*="otp" i]',
                            'input[name*="token" i]', 'input[name*="totp" i]',
                            'input[id*="code" i]', 'input[id*="otp" i]',
                            'input[type="tel"]', 'input[type="number"]',
                            'input[autocomplete="one-time-code"]',
                            'input[maxlength="6"]', 'input[pattern*="\\d"]'
                        ];
                        for (var i = 0; i < selectors.length; i++) {{
                            var el = document.querySelector(selectors[i]);
                            if (el) {{
                                el.value = "{escaped_code}";
                                el.dispatchEvent(new Event('input', {{bubbles: true}}));
                                el.dispatchEvent(new Event('change', {{bubbles: true}}));
                                return el.id || el.name || selectors[i];
                            }}
                        }}
                        return null;
                    }})()
                "#
                    ),
                    false,
                )?;

                // Click verify/submit button
                std::thread::sleep(Duration::from_secs(1));
                tab.evaluate(
                    r#"
                    (function() {
                        var selectors = [
                            'button[type="submit"]', 'input[type="submit"]',
                            'button[id*="verify" i]', 'button[id*="submit" i]',
                            'button:not([type])'
                        ];
                        for (var i = 0; i < selectors.length; i++) {
                            try {
                                var el = document.querySelector(selectors[i]);
                                if (el) { el.click(); return el.textContent || 'clicked'; }
                            } catch(e) {}
                        }
                        // Fallback: click any button with verify/submit/confirm text
                        var buttons = document.querySelectorAll('button');
                        for (var i = 0; i < buttons.length; i++) {
                            var text = buttons[i].textContent.toLowerCase();
                            if (text.includes('verify') || text.includes('submit') || text.includes('confirm')) {
                                buttons[i].click();
                                return buttons[i].textContent;
                            }
                        }
                        return null;
                    })()
                "#,
                    false,
                )?;

                eprintln!("[auto-login] TOTP submitted, waiting for redirect...");
            }
            Err(e) => {
                eprintln!("[auto-login] TOTP secret not available: {}", e);
                eprintln!("[auto-login] Enter the code manually in the browser.");
            }
        }
    }

    // Wait for authentication
    eprintln!("[auto-login] Waiting for redirect...");
    for attempt in 0..30 {
        std::thread::sleep(Duration::from_secs(2));
        let url = tab.get_url();
        if !url.contains("login") && !url.contains("Login") && !url.contains("auth.healthcode") {
            eprintln!("[auto-login] Authenticated! URL: {}", url);

            std::thread::sleep(Duration::from_secs(2));
            let cookies = capture_cookies(&tab)?;
            let json = serde_json::to_string(&cookies)?;
            keychain_store(&json)?;
            eprintln!("[auto-login] Session stored ({} cookies).", cookies.len());

            // Dump post-login structure
            let info = tab.evaluate(r#"
                (function() {
                    var info = { url: window.location.href, title: document.title };
                    var links = document.querySelectorAll('a[href]');
                    info.navLinks = Array.from(links).map(function(a) {
                        return {href: a.href, text: a.textContent.trim().substring(0, 80)};
                    }).filter(function(l) { return l.text.length > 0; }).slice(0, 30);
                    return JSON.stringify(info, null, 2);
                })()
            "#, false)?;

            if let Some(val) = info.value {
                let fallback = val.to_string();
                let s = val.as_str().unwrap_or(&fallback);
                eprintln!("[auto-login] Portal structure:");
                println!("{}", s);
            }

            // Screenshot
            let ss_path = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
                .join(".config/practiceforge/healthcode-autologin.png");
            if let Ok(bytes) = tab.capture_screenshot(
                headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
                None, None, true,
            ) {
                std::fs::write(&ss_path, &bytes)?;
                eprintln!("[auto-login] Screenshot: {}", ss_path.display());
            }

            return Ok(());
        }
        if attempt % 5 == 4 {
            eprintln!("[auto-login] Still waiting... ({}s)", (attempt + 1) * 2);
        }
    }

    // If we get here, auto-login may have failed
    eprintln!("[auto-login] Auto-login didn't redirect after 60s.");
    eprintln!("[auto-login] The browser is still open — complete login manually if needed.");
    eprintln!("[auto-login] Press Enter when done...");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    let url = tab.get_url();
    if url.contains("login") || url.contains("Login") {
        bail!("Login failed — still on login page.");
    }

    let cookies = capture_cookies(&tab)?;
    let json = serde_json::to_string(&cookies)?;
    keychain_store(&json)?;
    eprintln!("[auto-login] Session stored ({} cookies).", cookies.len());

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
        bail!("Session expired. Run 'healthcode login' first.");
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

fn cmd_explore() -> Result<()> {
    let cookies = load_cookies()?;
    eprintln!("[explore] Loaded {} cookies.", cookies.len());

    let headless = std::env::var("HC_VISIBLE").is_err();
    let browser = launch_browser(headless)?;
    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(30));

    inject_cookies(&tab, &cookies)?;

    // Navigate to portal
    tab.navigate_to("https://accounts.healthcode.co.uk/index.html#/product-services")?;
    std::thread::sleep(Duration::from_secs(5));

    let url = tab.get_url();
    if url.contains("login") {
        bail!("Session expired. Run 'healthcode auto-login' first.");
    }

    eprintln!("[explore] On portal: {}", url);

    // Click "Proceed to your product" to enter ePractice
    eprintln!("[explore] Clicking into ePractice...");
    let click_result = tab.evaluate(r#"
        (function() {
            var buttons = document.querySelectorAll('button, a');
            for (var i = 0; i < buttons.length; i++) {
                var text = buttons[i].textContent.toLowerCase();
                if (text.includes('proceed')) {
                    buttons[i].click();
                    return 'Clicked: ' + buttons[i].textContent.trim();
                }
            }
            return 'No proceed button found';
        })()
    "#, false)?;

    if let Some(val) = &click_result.value {
        eprintln!("[explore] {}", val.as_str().unwrap_or("(no result)"));
    }

    // Wait for ePractice to load
    std::thread::sleep(Duration::from_secs(8));

    let epractice_url = tab.get_url();
    eprintln!("[explore] ePractice URL: {}", epractice_url);

    // Deep DOM inspection of ePractice
    let info = tab.evaluate(r#"
        (function() {
            var info = {};
            info.url = window.location.href;
            info.title = document.title;
            info.bodyPreview = document.body ? document.body.innerText.substring(0, 3000) : '';

            // All navigation links
            var links = document.querySelectorAll('a[href], [role="menuitem"], [role="tab"]');
            info.navigation = Array.from(links).map(function(a) {
                return {
                    href: a.href || '',
                    text: a.textContent.trim().substring(0, 80),
                    class: (a.className || '').toString().substring(0, 60),
                    role: a.getAttribute('role') || ''
                };
            }).filter(function(l) { return l.text.length > 0 && l.text.length < 80; }).slice(0, 50);

            // All buttons
            var buttons = document.querySelectorAll('button, input[type="submit"], a.btn');
            info.buttons = Array.from(buttons).map(function(el) {
                return {
                    text: el.textContent.trim().substring(0, 60),
                    class: (el.className || '').toString().substring(0, 60),
                    href: el.href || '',
                    id: el.id || ''
                };
            }).filter(function(b) { return b.text.length > 0; }).slice(0, 30);

            // All forms
            var forms = document.querySelectorAll('form');
            info.forms = Array.from(forms).map(function(f) {
                return {
                    action: f.action,
                    method: f.method,
                    id: f.id,
                    class: f.className.substring(0, 60),
                    inputs: Array.from(f.querySelectorAll('input, select, textarea')).map(function(inp) {
                        return {
                            tag: inp.tagName, type: inp.type, name: inp.name, id: inp.id,
                            placeholder: inp.placeholder || ''
                        };
                    })
                };
            });

            // Look for menu/sidebar navigation
            var navElements = document.querySelectorAll('nav, [role="navigation"], .sidebar, .menu, [class*="nav"], [class*="menu"]');
            info.navMenus = Array.from(navElements).map(function(nav) {
                var items = Array.from(nav.querySelectorAll('a, button, [role="menuitem"]'));
                return {
                    tag: nav.tagName,
                    class: (nav.className || '').toString().substring(0, 80),
                    items: items.map(function(item) {
                        return { text: item.textContent.trim().substring(0, 60), href: item.href || '' };
                    }).filter(function(i) { return i.text.length > 0; }).slice(0, 20)
                };
            }).filter(function(n) { return n.items.length > 0; });

            // Look specifically for claim/auth/AXA related content
            info.claimRelated = [];
            var allText = document.body ? document.body.innerText : '';
            var keywords = ['claim', 'auth', 'axa', 'bupa', 'aviva', 'vitality', 'extension', 'referral', 'pre-auth'];
            for (var i = 0; i < keywords.length; i++) {
                if (allText.toLowerCase().includes(keywords[i])) {
                    info.claimRelated.push(keywords[i]);
                }
            }

            return JSON.stringify(info, null, 2);
        })()
    "#, false)?;

    if let Some(val) = info.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("{}", s);
    }

    // Screenshot
    let ss_path = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".config/practiceforge/healthcode-epractice.png");
    if let Ok(bytes) = tab.capture_screenshot(
        headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
        None, None, true,
    ) {
        std::fs::write(&ss_path, &bytes)?;
        eprintln!("[explore] Screenshot: {}", ss_path.display());
    }

    // If there are sub-pages, try to navigate to each and capture structure
    // Look for iframe-based navigation (common in ePractice-style portals)
    let iframes = tab.evaluate(r#"
        (function() {
            var frames = document.querySelectorAll('iframe');
            return Array.from(frames).map(function(f) {
                return { src: f.src || '', id: f.id || '', name: f.name || '' };
            });
        })()
    "#, false)?;

    if let Some(val) = &iframes.value {
        let s = val.to_string();
        if s != "[]" {
            eprintln!("[explore] Iframes found: {}", s);
        }
    }

    // --- Step 2: Navigate to Patient List ---
    eprintln!("[explore] Step 2: Navigating to Patient List...");
    let expand_patients = tab.evaluate(r#"
        (function() {
            // Click "Patients" menu header to expand it
            var headers = document.querySelectorAll('.ui-panelmenu-header');
            for (var i = 0; i < headers.length; i++) {
                if (headers[i].textContent.trim().includes('Patients')) {
                    headers[i].querySelector('a').click();
                    return 'Expanded Patients menu';
                }
            }
            return 'Patients menu not found';
        })()
    "#, false)?;
    if let Some(val) = &expand_patients.value {
        eprintln!("[explore] {}", val.as_str().unwrap_or("(no result)"));
    }
    std::thread::sleep(Duration::from_secs(2));

    // Click "Patient List" submenu item
    let click_plist = tab.evaluate(r#"
        (function() {
            var items = document.querySelectorAll('.ui-menuitem-link');
            for (var i = 0; i < items.length; i++) {
                if (items[i].textContent.trim() === 'Patient List') {
                    items[i].click();
                    return 'Clicked Patient List';
                }
            }
            return 'Patient List not found';
        })()
    "#, false)?;
    if let Some(val) = &click_plist.value {
        eprintln!("[explore] {}", val.as_str().unwrap_or("(no result)"));
    }
    std::thread::sleep(Duration::from_secs(5));

    // Scrape the patient list page
    let patient_list = tab.evaluate(r#"
        (function() {
            var info = {};
            info.url = window.location.href;
            info.title = document.title;
            info.bodyPreview = document.body ? document.body.innerText.substring(0, 3000) : '';

            // Find patient table/list
            var tables = document.querySelectorAll('table');
            info.tables = Array.from(tables).map(function(t) {
                var headers = Array.from(t.querySelectorAll('th')).map(function(th) {
                    return th.textContent.trim();
                });
                var rows = Array.from(t.querySelectorAll('tbody tr')).slice(0, 5).map(function(tr) {
                    return Array.from(tr.querySelectorAll('td')).map(function(td) {
                        return td.textContent.trim().substring(0, 50);
                    });
                });
                return { headers: headers, sampleRows: rows, rowCount: t.querySelectorAll('tbody tr').length };
            }).filter(function(t) { return t.headers.length > 0 || t.sampleRows.length > 0; });

            // Find any search/filter inputs
            var inputs = document.querySelectorAll('input[type="text"], input[type="search"]');
            info.searchInputs = Array.from(inputs).map(function(inp) {
                return { name: inp.name, id: inp.id, placeholder: inp.placeholder || '' };
            });

            // Find links that might lead to patient records
            var links = document.querySelectorAll('a[href*="patient" i], a[href*="Patient" i], [onclick*="patient" i]');
            info.patientLinks = Array.from(links).slice(0, 10).map(function(a) {
                return { text: a.textContent.trim().substring(0, 60), href: a.href || '', onclick: (a.getAttribute('onclick') || '').substring(0, 100) };
            });

            return JSON.stringify(info, null, 2);
        })()
    "#, false)?;

    eprintln!("[explore] Step 2 - Patient List:");
    if let Some(val) = patient_list.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("=== PATIENT LIST ===");
        println!("{}", s);
    }

    // Screenshot
    let ss2 = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".config/practiceforge/healthcode-patients.png");
    if let Ok(bytes) = tab.capture_screenshot(
        headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
        None, None, true,
    ) {
        std::fs::write(&ss2, &bytes)?;
        eprintln!("[explore] Screenshot: {}", ss2.display());
    }

    // --- Step 3: Click first patient to see record structure ---
    eprintln!("[explore] Step 3: Opening first patient record...");
    let click_patient = tab.evaluate(r#"
        (function() {
            // Try clicking the first row in a patient table
            var rows = document.querySelectorAll('table tbody tr');
            if (rows.length > 0) {
                rows[0].click();
                // Also try double-click (some PrimeFaces tables use dblclick)
                var evt = new MouseEvent('dblclick', { bubbles: true });
                rows[0].dispatchEvent(evt);
                return 'Clicked first patient row (single + double click)';
            }
            // Try clicking any patient link
            var links = document.querySelectorAll('a');
            for (var i = 0; i < links.length; i++) {
                var text = links[i].textContent.trim();
                // Skip navigation items, look for patient names
                if (text.length > 3 && text.length < 50 && !text.match(/^(Status|Patients|Appointments|Accounting|Contacts|Reports|Documents|Settings|Quick|Add|Logout|Copyright)/)) {
                    // This might be a patient name
                    if (links[i].closest('table') || links[i].closest('[class*="list"]')) {
                        links[i].click();
                        return 'Clicked link: ' + text;
                    }
                }
            }
            return 'No patient to click';
        })()
    "#, false)?;

    if let Some(val) = &click_patient.value {
        eprintln!("[explore] {}", val.as_str().unwrap_or("(no result)"));
    }

    std::thread::sleep(Duration::from_secs(5));

    // --- Step 4: Scrape patient record page ---
    let patient_record = tab.evaluate(r#"
        (function() {
            var info = {};
            info.url = window.location.href;
            info.title = document.title;
            info.bodyPreview = document.body ? document.body.innerText.substring(0, 4000) : '';

            // Find tabs (patient records often have tabbed interfaces)
            var tabs = document.querySelectorAll('[role="tab"], .ui-tabs-header, .ui-tabview-nav li, [class*="tab"]');
            info.tabs = Array.from(tabs).map(function(t) {
                return { text: t.textContent.trim().substring(0, 40), class: (t.className || '').toString().substring(0, 60) };
            }).filter(function(t) { return t.text.length > 0 && t.text.length < 40; });

            // Find all forms
            var forms = document.querySelectorAll('form');
            info.forms = Array.from(forms).map(function(f) {
                var inputs = Array.from(f.querySelectorAll('input, select, textarea')).map(function(inp) {
                    return { tag: inp.tagName, type: inp.type, name: inp.name, id: inp.id, placeholder: inp.placeholder || '', label: '' };
                });
                return { action: f.action, id: f.id, inputCount: inputs.length, inputs: inputs.slice(0, 20) };
            }).filter(function(f) { return f.inputCount > 0; });

            // Find buttons related to claims/auth/billing
            var buttons = document.querySelectorAll('button, a.btn, input[type="submit"], [role="button"]');
            info.actionButtons = Array.from(buttons).map(function(b) {
                return { text: b.textContent.trim().substring(0, 60), class: (b.className || '').toString().substring(0, 60), id: b.id || '' };
            }).filter(function(b) {
                var t = b.text.toLowerCase();
                return b.text.length > 0 && (t.includes('claim') || t.includes('auth') || t.includes('bill') || t.includes('invoice') || t.includes('submit') || t.includes('new') || t.includes('create') || t.includes('extension'));
            });

            // All navigation/action items on this page
            var allActions = document.querySelectorAll('a, button, [role="menuitem"], [role="tab"]');
            info.allActions = Array.from(allActions).map(function(a) {
                return { text: a.textContent.trim().substring(0, 60), href: a.href || '' };
            }).filter(function(a) { return a.text.length > 2 && a.text.length < 60; }).slice(0, 50);

            // Search for keywords in the full page
            var fullText = document.body ? document.body.innerText.toLowerCase() : '';
            var keywords = ['claim', 'authorisation', 'authorization', 'auth', 'pre-auth', 'extension', 'axa', 'bupa', 'aviva', 'vitality', 'insurer', 'membership', 'policy', 'diagnosis', 'icd', 'sessions', 'treatment'];
            info.keywordsFound = keywords.filter(function(k) { return fullText.includes(k); });

            return JSON.stringify(info, null, 2);
        })()
    "#, false)?;

    eprintln!("[explore] Step 4 - Patient Record:");
    if let Some(val) = patient_record.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("=== PATIENT RECORD ===");
        println!("{}", s);
    }

    // Screenshot
    let ss3 = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".config/practiceforge/healthcode-patient-record.png");
    if let Ok(bytes) = tab.capture_screenshot(
        headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
        None, None, true,
    ) {
        std::fs::write(&ss3, &bytes)?;
        eprintln!("[explore] Screenshot: {}", ss3.display());
    }

    // --- Step 5: Look for Accounting/eBill section ---
    eprintln!("[explore] Step 5: Checking Accounting/eBill...");
    // Navigate back to home first
    tab.navigate_to("https://www.veda.healthcode.co.uk/epractice/pages/HomePage/specialistviewpage.xhtml")?;
    std::thread::sleep(Duration::from_secs(5));

    // Click Accounting menu
    let expand_acct = tab.evaluate(r#"
        (function() {
            var headers = document.querySelectorAll('.ui-panelmenu-header');
            for (var i = 0; i < headers.length; i++) {
                if (headers[i].textContent.trim().includes('Accounting')) {
                    headers[i].querySelector('a').click();
                    return 'Expanded Accounting menu';
                }
            }
            return 'Accounting menu not found';
        })()
    "#, false)?;
    if let Some(val) = &expand_acct.value {
        eprintln!("[explore] {}", val.as_str().unwrap_or("(no result)"));
    }
    std::thread::sleep(Duration::from_secs(2));

    // Scrape all submenu items under Accounting before clicking
    let acct_items = tab.evaluate(r#"
        (function() {
            var items = document.querySelectorAll('.ui-menuitem-link');
            return JSON.stringify(Array.from(items).map(function(a) {
                return a.textContent.trim();
            }).filter(function(t) { return t.length > 0; }));
        })()
    "#, false)?;
    if let Some(val) = &acct_items.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        eprintln!("[explore] All submenu items visible: {}", s);
    }

    // Click Quick eBill
    let click_ebill = tab.evaluate(r#"
        (function() {
            var items = document.querySelectorAll('.ui-menuitem-link');
            for (var i = 0; i < items.length; i++) {
                if (items[i].textContent.trim() === 'Quick eBill') {
                    items[i].click();
                    return 'Clicked Quick eBill';
                }
            }
            return 'Quick eBill not found';
        })()
    "#, false)?;
    if let Some(val) = &click_ebill.value {
        eprintln!("[explore] {}", val.as_str().unwrap_or("(no result)"));
    }
    std::thread::sleep(Duration::from_secs(5));

    // Scrape eBill page
    let ebill = tab.evaluate(r#"
        (function() {
            var info = {};
            info.url = window.location.href;
            info.bodyPreview = document.body ? document.body.innerText.substring(0, 3000) : '';

            var forms = document.querySelectorAll('form');
            info.forms = Array.from(forms).map(function(f) {
                var inputs = Array.from(f.querySelectorAll('input, select, textarea')).map(function(inp) {
                    return { tag: inp.tagName, type: inp.type, name: inp.name, id: inp.id, placeholder: inp.placeholder || '' };
                });
                return { id: f.id, inputCount: inputs.length, inputs: inputs.slice(0, 30) };
            }).filter(function(f) { return f.inputCount > 0; });

            var fullText = document.body ? document.body.innerText.toLowerCase() : '';
            var keywords = ['claim', 'authorisation', 'authorization', 'pre-auth', 'extension', 'axa', 'insurer', 'membership', 'policy', 'diagnosis', 'sessions', 'treatment'];
            info.keywordsFound = keywords.filter(function(k) { return fullText.includes(k); });

            return JSON.stringify(info, null, 2);
        })()
    "#, false)?;

    eprintln!("[explore] Step 5 - Quick eBill:");
    if let Some(val) = ebill.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("=== QUICK EBILL ===");
        println!("{}", s);
    }

    let ss4 = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".config/practiceforge/healthcode-ebill.png");
    if let Ok(bytes) = tab.capture_screenshot(
        headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
        None, None, true,
    ) {
        std::fs::write(&ss4, &bytes)?;
        eprintln!("[explore] Screenshot: {}", ss4.display());
    }

    // --- Step 6: Explore all top-level menu sections ---
    eprintln!("[explore] Step 6: Mapping all menu sections...");
    tab.navigate_to("https://www.veda.healthcode.co.uk/epractice/pages/HomePage/specialistviewpage.xhtml")?;
    std::thread::sleep(Duration::from_secs(5));

    let all_menus = tab.evaluate(r#"
        (function() {
            var result = {};
            var headers = document.querySelectorAll('.ui-panelmenu-header');
            result.menuSections = Array.from(headers).map(function(h) {
                return h.textContent.trim();
            });

            // Expand all menus and capture all items
            for (var i = 0; i < headers.length; i++) {
                var a = headers[i].querySelector('a');
                if (a) a.click();
            }

            return JSON.stringify(result, null, 2);
        })()
    "#, false)?;
    if let Some(val) = &all_menus.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("=== ALL MENU SECTIONS ===");
        println!("{}", s);
    }

    std::thread::sleep(Duration::from_secs(2));

    // Now scrape all visible submenu items after expanding all
    let all_items = tab.evaluate(r#"
        (function() {
            var items = document.querySelectorAll('.ui-menuitem-link, .ui-panelmenu-content .ui-menuitem a');
            return JSON.stringify(Array.from(items).map(function(a) {
                return {
                    text: a.textContent.trim(),
                    href: a.href || '',
                    onclick: (a.getAttribute('onclick') || '').substring(0, 120),
                    parentMenu: (function() {
                        var panel = a.closest('.ui-panelmenu-panel');
                        if (panel) {
                            var header = panel.querySelector('.ui-panelmenu-header');
                            return header ? header.textContent.trim() : '';
                        }
                        return '';
                    })()
                };
            }).filter(function(i) { return i.text.length > 0; }), null, 2);
        })()
    "#, false)?;
    if let Some(val) = &all_items.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("=== ALL SUBMENU ITEMS ===");
        println!("{}", s);
    }

    let ss5 = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".config/practiceforge/healthcode-all-menus.png");
    if let Ok(bytes) = tab.capture_screenshot(
        headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
        None, None, true,
    ) {
        std::fs::write(&ss5, &bytes)?;
        eprintln!("[explore] Screenshot: {}", ss5.display());
    }

    if !headless {
        eprintln!("[explore] Press Enter to close...");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
    }

    Ok(())
}

fn cmd_fill(client_id: &str, dry_run: bool) -> Result<()> {
    // 1. Get form data from `clinical auth form <client_id>`
    let output = Command::new("clinical")
        .args(["auth", "form", client_id])
        .output()
        .context("Failed to run `clinical auth form`")?;
    if !output.status.success() {
        bail!(
            "clinical auth form failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let form_json: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse form JSON")?;

    // 2. Load cookies from keychain
    let cookies = load_cookies()?;

    // 3. Launch visible browser (form filling should ALWAYS be visible for review)
    let browser = launch_browser(false)?;
    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(30));

    // 4. Inject cookies
    inject_cookies(&tab, &cookies)?;

    // 5. Navigate to Healthcode portal
    tab.navigate_to(HC_BASE)?;
    std::thread::sleep(Duration::from_secs(3));

    // Check session is valid
    let url = tab.get_url();
    if url.contains("login") {
        bail!("Session expired. Run 'healthcode login' first.");
    }

    // 6. Navigate to the authorisation form
    // TODO: This needs the actual Healthcode portal navigation.
    // For now, look for links containing "auth", "claim", "extension", "form"
    eprintln!("[fill] Looking for authorisation form link...");
    let nav_result = tab.evaluate(
        r#"
        (function() {
            var links = document.querySelectorAll('a[href]');
            var targets = Array.from(links).filter(function(a) {
                var text = a.textContent.toLowerCase();
                var href = a.href.toLowerCase();
                return text.includes('auth') || text.includes('extension')
                    || text.includes('claim') || text.includes('new request')
                    || href.includes('auth') || href.includes('claim');
            });
            if (targets.length > 0) {
                return JSON.stringify(targets.map(function(a) {
                    return {href: a.href, text: a.textContent.trim()};
                }));
            }
            return "[]";
        })()
        "#,
        false,
    )?;

    // Log what we found for the user
    if let Some(val) = &nav_result.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        eprintln!("[fill] Found navigation targets: {}", s);
    }

    // 7. Populate form fields
    // Build a JS snippet that fills each field by selector or label
    eprintln!("[fill] Populating form fields from clinical data...");

    let patient = &form_json["patient"];
    let specialist = &form_json["specialist"];
    let clinical = &form_json["clinical"];
    let sessions = &form_json["sessions"];

    // Generic field population via multiple strategies:
    // - by name attribute
    // - by label text
    // - by placeholder text
    let sessions_authorised = sessions["sessions_authorised"].to_string();
    let sessions_used = sessions["sessions_used_current_auth"].to_string();
    let sessions_remaining = sessions["sessions_remaining"].to_string();

    let field_mappings: Vec<(&str, &str)> = vec![
        // Patient fields
        ("patient_name", patient["name"].as_str().unwrap_or("")),
        (
            "date_of_birth",
            patient["date_of_birth"].as_str().unwrap_or(""),
        ),
        ("dob", patient["date_of_birth"].as_str().unwrap_or("")),
        ("address", patient["address"].as_str().unwrap_or("")),
        ("phone", patient["phone"].as_str().unwrap_or("")),
        ("telephone", patient["phone"].as_str().unwrap_or("")),
        (
            "membership",
            patient["membership_number"].as_str().unwrap_or(""),
        ),
        (
            "member_number",
            patient["membership_number"].as_str().unwrap_or(""),
        ),
        (
            "policy",
            patient["membership_number"].as_str().unwrap_or(""),
        ),
        // Specialist fields
        (
            "specialist_name",
            specialist["name"].as_str().unwrap_or(""),
        ),
        ("practitioner", specialist["name"].as_str().unwrap_or("")),
        (
            "provider_number",
            specialist["provider_number"].as_str().unwrap_or(""),
        ),
        (
            "expertise",
            specialist["area_of_expertise"].as_str().unwrap_or(""),
        ),
        // Clinical fields
        ("diagnosis", clinical["diagnosis"].as_str().unwrap_or("")),
        (
            "diagnostic_code",
            clinical["diagnostic_code"].as_str().unwrap_or(""),
        ),
        ("icd", clinical["diagnostic_code"].as_str().unwrap_or("")),
        (
            "therapy_model",
            clinical["model_of_therapy"].as_str().unwrap_or(""),
        ),
        // Session fields
        ("sessions_authorised", &sessions_authorised),
        ("sessions_used", &sessions_used),
        ("sessions_remaining", &sessions_remaining),
        (
            "sessions_requested",
            sessions["additional_sessions_requested"]
                .as_str()
                .unwrap_or(""),
        ),
    ];

    // Build JS to populate fields
    let mut js_parts = Vec::new();
    for (field_name, value) in &field_mappings {
        if value.is_empty() || *value == "null" || value.starts_with("[TO COMPLETE") {
            continue;
        }
        let escaped_value = value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n");
        let escaped_name = field_name.replace('"', "\\\"");
        // Try multiple selectors for each field
        js_parts.push((
            *field_name,
            *value,
            format!(
                r#"
            (function() {{
                var val = "{escaped_value}";
                var selectors = [
                    'input[name*="{escaped_name}" i]',
                    'textarea[name*="{escaped_name}" i]',
                    'input[id*="{escaped_name}" i]',
                    'textarea[id*="{escaped_name}" i]',
                    'input[placeholder*="{escaped_name}" i]'
                ];
                for (var i = 0; i < selectors.length; i++) {{
                    var el = document.querySelector(selectors[i]);
                    if (el) {{
                        el.value = val;
                        el.dispatchEvent(new Event('input', {{bubbles: true}}));
                        el.dispatchEvent(new Event('change', {{bubbles: true}}));
                        return true;
                    }}
                }}
                return false;
            }})()
            "#
            ),
        ));
    }

    // Execute each field population
    let mut filled = 0;
    let mut missed = 0;
    for (field_name, value, js) in &js_parts {
        match tab.evaluate(js, false) {
            Ok(result) => {
                let success = result
                    .value
                    .as_ref()
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if success {
                    filled += 1;
                    eprintln!("  + {}: {}", field_name, truncate_str(value, 40));
                } else {
                    missed += 1;
                    eprintln!("  - {}: no matching field found", field_name);
                }
            }
            Err(_) => {
                missed += 1;
            }
        }
    }

    eprintln!(
        "\n[fill] Populated {}/{} fields ({} not found on page).",
        filled,
        filled + missed,
        missed
    );

    // 8. Screenshot the populated form
    let ss_path = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".config/practiceforge")
        .join(format!("healthcode-fill-{}.png", client_id));
    std::fs::create_dir_all(ss_path.parent().unwrap()).ok();
    if let Ok(bytes) = tab.capture_screenshot(
        headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
        None,
        None,
        true,
    ) {
        std::fs::write(&ss_path, &bytes)?;
        eprintln!("[fill] Screenshot: {}", ss_path.display());
    }

    if dry_run {
        eprintln!("\n[fill] Dry run -- not submitting. Review the browser window.");
        eprintln!("[fill] Press Enter to close...");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
    } else {
        eprintln!("\n[fill] Review the form in the browser.");
        eprintln!("[fill] Fields marked [TO COMPLETE] need manual input.");
        eprintln!("[fill] When ready, submit manually in the browser.");
        eprintln!("[fill] Press Enter to close when done...");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
    }

    // Capture any updated cookies
    let cookies = capture_cookies(&tab)?;
    let json = serde_json::to_string(&cookies)?;
    keychain_store(&json)?;

    Ok(())
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max - 3])
    }
}

fn load_totp_secret() -> Result<String> {
    let output = if cfg!(target_os = "macos") {
        Command::new("security")
            .args(["find-generic-password", "-s", TOTP_SERVICE, "-a", TOTP_ACCOUNT, "-w"])
            .output()
            .context("Failed to read TOTP secret from macOS keychain")?
    } else {
        Command::new("secret-tool")
            .args(["lookup", "service", TOTP_SERVICE, "account", TOTP_ACCOUNT])
            .output()
            .context("Failed to read TOTP secret from secret-service")?
    };

    if !output.status.success() {
        bail!(
            "TOTP secret not found. Store it first:\n  \
             Linux: \"<secret>\" | secret-tool store --label \"Healthcode TOTP\" service {} account {}\n  \
             macOS: security add-generic-password -s {} -a {} -w <secret>",
            TOTP_SERVICE, TOTP_ACCOUNT, TOTP_SERVICE, TOTP_ACCOUNT
        );
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn generate_totp_code(secret: &str) -> Result<String> {
    use totp_rs::{Algorithm, Secret, TOTP};

    let totp = TOTP::new(
        Algorithm::SHA1,
        6,  // digits
        1,  // skew
        30, // step (seconds)
        Secret::Encoded(secret.to_string())
            .to_bytes()
            .map_err(|e| anyhow::anyhow!("Invalid TOTP secret: {}", e))?,
    )
    .map_err(|e| anyhow::anyhow!("Failed to create TOTP: {}", e))?;

    Ok(totp
        .generate_current()
        .map_err(|e| anyhow::anyhow!("Failed to generate TOTP code: {}", e))?)
}

fn cmd_totp_test() -> Result<()> {
    let secret = load_totp_secret()?;
    let code = generate_totp_code(&secret)?;
    println!("Current TOTP code: {}", code);
    println!("(Compare with your authenticator app to verify the secret is correct)");
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
    if cfg!(target_os = "macos") {
        Command::new("security")
            .args(["add-generic-password", "-s", KEYCHAIN_SERVICE, "-a", KEYCHAIN_ACCOUNT, "-w", data, "-U"])
            .status()
            .context("Failed to store in macOS keychain")?;
    } else {
        // Linux: pipe data to secret-tool via stdin
        let mut child = Command::new("secret-tool")
            .args(["store", "--label", "Healthcode session", "service", KEYCHAIN_SERVICE, "account", KEYCHAIN_ACCOUNT])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .context("Failed to run secret-tool — is libsecret installed?")?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin.write_all(data.as_bytes())?;
        }
        child.wait().context("secret-tool failed")?;
    }
    Ok(())
}

fn keychain_delete() -> Result<()> {
    if cfg!(target_os = "macos") {
        Command::new("security")
            .args(["delete-generic-password", "-s", KEYCHAIN_SERVICE, "-a", KEYCHAIN_ACCOUNT])
            .output()?;
    } else {
        Command::new("secret-tool")
            .args(["clear", "service", KEYCHAIN_SERVICE, "account", KEYCHAIN_ACCOUNT])
            .output()?;
    }
    Ok(())
}

fn load_cookies() -> Result<Vec<StoredCookie>> {
    let output = if cfg!(target_os = "macos") {
        Command::new("security")
            .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-a", KEYCHAIN_ACCOUNT, "-w"])
            .output()
            .context("Failed to read macOS keychain")?
    } else {
        Command::new("secret-tool")
            .args(["lookup", "service", KEYCHAIN_SERVICE, "account", KEYCHAIN_ACCOUNT])
            .output()
            .context("Failed to read secret-service — is libsecret installed?")?
    };
    if !output.status.success() {
        bail!("No Healthcode session in keychain. Run 'healthcode login' first.");
    }
    let json = String::from_utf8(output.stdout)?.trim().to_string();
    Ok(serde_json::from_str(&json)?)
}
