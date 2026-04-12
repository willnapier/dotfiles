//! Healthcode insurer portal automation.
//!
//! Subcommands:
//!   healthcode_spike recon              — inspect login page (no credentials needed)
//!   healthcode_spike login              — manual login + cookie capture to keychain
//!   healthcode_spike inspect            — load cookies, inspect the portal
//!   healthcode_spike fill <client_id>   — fill authorisation form from clinical data

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
        .join(".config/clinical-product")
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
