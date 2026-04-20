//! TM3 document upload — uploads files to a patient's documents in TM3.
//!
//! Authentication: passkey + Touch ID → session cookies stored in macOS Keychain.
//!
//! Commands:
//!   tm3-upload login                    — authenticate via passkey, store cookies in keychain
//!   tm3-upload upload <tm3_id> <file>   — upload file to patient's documents
//!   tm3-upload check                    — verify stored session is valid

use anyhow::{bail, Context, Result};
use headless_chrome::browser::tab::Tab;
use headless_chrome::protocol::cdp::Network;
use headless_chrome::{Browser, LaunchOptions};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const TM3_BASE: &str = "https://changeofharleystreet.tm3app.com";
const KEYCHAIN_SERVICE: &str = "tm3-session";
const KEYCHAIN_ACCOUNT: &str = "changeofharleystreet";

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
    if args.len() < 2 {
        eprintln!("TM3 document upload tool");
        eprintln!();
        eprintln!("Usage:");
        eprintln!("  tm3-upload login                    — authenticate via passkey");
        eprintln!("  tm3-upload upload <tm3_id> <file>   — upload file to patient");
        eprintln!("  tm3-upload check                    — verify session is valid");
        std::process::exit(1);
    }

    match args[1].as_str() {
        "login" => cmd_login(),
        "upload" => {
            let tm3_id = args.get(2).context("Missing <tm3_id>")?;
            let file_path = args.get(3).context("Missing <file>")?;
            cmd_upload(tm3_id, file_path)
        }
        "check" => cmd_check(),
        other => bail!("Unknown command '{}'. Use login, upload, or check.", other),
    }
}

// ── Login: passkey auth → keychain ──────────────────────────────────────────

fn cmd_login() -> Result<()> {
    // Delete any existing session first
    keychain_delete().ok();

    eprintln!("[login] Launching Chrome...");
    let browser = launch_browser(false)?;
    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(30));

    eprintln!("[login] Navigating to TM3...");
    tab.navigate_to(TM3_BASE)?;
    tab.wait_until_navigated()?;

    // Click passkey button automatically
    eprintln!("[login] Clicking passkey login...");
    tab.evaluate(
        r#"
        (function() {
            var buttons = document.querySelectorAll('button');
            for (var i = 0; i < buttons.length; i++) {
                if (buttons[i].textContent.toLowerCase().includes('passkey')) {
                    buttons[i].click();
                    return;
                }
            }
        })()
        "#,
        false,
    )?;

    eprintln!("[login] Authenticate with Touch ID...");
    wait_for_auth(&tab)?;
    eprintln!("[login] Authenticated.");

    // Let SPA settle
    std::thread::sleep(Duration::from_secs(2));

    // Capture and store cookies
    let cookies = capture_cookies(&tab)?;
    let json = serde_json::to_string(&cookies)?;
    keychain_store(&json)?;
    eprintln!("[login] Session stored in keychain ({} cookies).", cookies.len());

    Ok(())
}

// ── Check: verify session is still valid ────────────────────────────────────

fn cmd_check() -> Result<()> {
    let cookies = load_cookies_from_keychain()?;
    eprintln!("[check] Loaded {} cookies from keychain.", cookies.len());

    let browser = launch_browser(true)?;
    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(15));

    inject_cookies(&tab, &cookies)?;

    // Reload with cookies
    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(5));

    let url = tab.get_url();
    if url.contains("login") || url.contains("Login") {
        eprintln!("[check] Session expired. Run 'tm3-upload login' to re-authenticate.");
        std::process::exit(1);
    }

    eprintln!("[check] Session valid. Logged in at: {}", url);
    Ok(())
}

// ── Upload: navigate to patient documents → upload file ─────────────────────

fn cmd_upload(tm3_id: &str, file_path: &str) -> Result<()> {
    let file = Path::new(file_path).canonicalize().context(format!(
        "File not found: {}",
        file_path
    ))?;
    eprintln!("[upload] File: {}", file.display());

    let cookies = load_cookies_from_keychain()?;
    eprintln!("[upload] Loaded session from keychain.");

    let headless = std::env::var("TM3_VISIBLE").is_err();
    let browser = launch_browser(headless)?;
    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(30));

    // Inject cookies and navigate to diary (SPA boot)
    inject_cookies(&tab, &cookies)?;
    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(5));

    let url = tab.get_url();
    if url.contains("login") {
        bail!("Session expired. Run 'tm3-upload login' to re-authenticate.");
    }
    eprintln!("[upload] Authenticated. Diary loaded.");

    // Navigate to patient documents via the correct URL path
    // The SPA must boot from diary first, then we navigate internally.
    let doc_url = format!("{}/contacts/clients/{}/documents", TM3_BASE, tm3_id);
    eprintln!("[upload] Navigating to patient {} documents...", tm3_id);

    // Try direct navigation to the correct URL first
    tab.navigate_to(&doc_url)?;
    std::thread::sleep(Duration::from_secs(5));

    let current_url = tab.get_url();
    eprintln!("[upload] URL: {}", current_url);

    // Check if the page rendered (look for the Attach File button)
    let page_ready = wait_for_documents_page(&tab)?;

    if !page_ready {
        eprintln!("[upload] Direct navigation failed. Using SPA search...");
        // Fall back to diary → Quick Search → patient → documents
        tab.navigate_to(TM3_BASE)?;
        std::thread::sleep(Duration::from_secs(3));
        navigate_via_search(&tab, tm3_id)?;
    }

    // Click "Attach File" button
    eprintln!("[upload] Clicking 'Attach File'...");
    let clicked = tab.evaluate(
        r#"
        (function() {
            var buttons = document.querySelectorAll('button');
            for (var i = 0; i < buttons.length; i++) {
                if (buttons[i].textContent.trim() === 'Attach File') {
                    buttons[i].click();
                    return true;
                }
            }
            return false;
        })()
        "#,
        false,
    )?;

    let button_found = clicked
        .value
        .as_ref()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if !button_found {
        // Try clicking the drop zone instead
        eprintln!("[upload] No 'Attach File' button — trying drop zone...");
        tab.evaluate(
            r#"
            (function() {
                var dz = document.querySelector('.drop-zone');
                if (dz) { dz.click(); return true; }
                return false;
            })()
            "#,
            false,
        )?;
    }

    std::thread::sleep(Duration::from_secs(1));

    // Set files on the hidden file input via CDP DOM.setFileInputFiles
    eprintln!("[upload] Setting file on input element via CDP...");
    let file_str = file.to_string_lossy().to_string();

    // Find the file input element and get its node ID
    let file_input = tab
        .find_element(r#"input[type="file"]"#)
        .context("No file input found on the documents page")?;

    let node_id = file_input
        .get_description()?
        .node_id;

    tab.handle_file_chooser(vec![file_str.clone()], node_id)?;
    eprintln!("[upload] File set: {}", file.display());

    // Wait for upload to complete
    eprintln!("[upload] Waiting for upload to complete...");
    std::thread::sleep(Duration::from_secs(5));

    // Check result
    let result = tab.evaluate(
        r#"
        (function() {
            var body = document.body.innerText;
            var hasError = body.includes('error') || body.includes('Error') || body.includes('failed');
            var hasSuccess = body.includes('uploaded') || body.includes('Uploaded') || body.includes('success');
            return JSON.stringify({
                url: window.location.href,
                hasError: hasError,
                hasSuccess: hasSuccess
            });
        })()
        "#,
        false,
    )?;

    if let Some(val) = result.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        eprintln!("[upload] Result: {}", s);
    }

    eprintln!("[upload] Done.");
    Ok(())
}

// ── Keychain helpers ────────────────────────────────────────────────────────

fn keychain_store(data: &str) -> Result<()> {
    if cfg!(target_os = "macos") {
        keychain_delete().ok();
        let status = Command::new("security")
            .args([
                "add-generic-password",
                "-s", KEYCHAIN_SERVICE,
                "-a", KEYCHAIN_ACCOUNT,
                "-w", data,
                "-U",
            ])
            .status()
            .context("Failed to run macOS security CLI")?;
        if !status.success() {
            bail!("Failed to store in macOS keychain (exit {})", status);
        }
    } else {
        use std::io::Write;
        let mut child = Command::new("secret-tool")
            .args([
                "store",
                "--label", &format!("{} session", KEYCHAIN_SERVICE),
                "service", KEYCHAIN_SERVICE,
                "account", KEYCHAIN_ACCOUNT,
            ])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .context("Failed to run secret-tool — is libsecret-tools installed?")?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(data.as_bytes())?;
        }
        child.wait()?;
    }
    Ok(())
}

fn keychain_load() -> Result<String> {
    let output = if cfg!(target_os = "macos") {
        Command::new("security")
            .args([
                "find-generic-password",
                "-s", KEYCHAIN_SERVICE,
                "-a", KEYCHAIN_ACCOUNT,
                "-w",
            ])
            .output()
            .context("Failed to run macOS security CLI")?
    } else {
        Command::new("secret-tool")
            .args(["lookup", "service", KEYCHAIN_SERVICE, "account", KEYCHAIN_ACCOUNT])
            .output()
            .context("Failed to run secret-tool — is libsecret-tools installed?")?
    };

    if !output.status.success() {
        bail!("No TM3 session in keychain. Run 'tm3-upload login' first.");
    }

    let data = String::from_utf8(output.stdout)?.trim().to_string();
    Ok(data)
}

fn keychain_delete() -> Result<()> {
    if cfg!(target_os = "macos") {
        Command::new("security")
            .args([
                "delete-generic-password",
                "-s", KEYCHAIN_SERVICE,
                "-a", KEYCHAIN_ACCOUNT,
            ])
            .output()?;
    } else {
        Command::new("secret-tool")
            .args(["clear", "service", KEYCHAIN_SERVICE, "account", KEYCHAIN_ACCOUNT])
            .output()?;
    }
    Ok(())
}

fn load_cookies_from_keychain() -> Result<Vec<StoredCookie>> {
    let json = keychain_load()?;
    let cookies: Vec<StoredCookie> = serde_json::from_str(&json)
        .context("Failed to parse stored cookies")?;
    Ok(cookies)
}

// ── Browser helpers ─────────────────────────────────────────────────────────

fn launch_browser(headless: bool) -> Result<Browser> {
    Browser::new(
        LaunchOptions::default_builder()
            .headless(headless)
            .window_size(Some((1280, 900)))
            .idle_browser_timeout(Duration::from_secs(600))
            .build()
            .context("Failed to build launch options")?,
    )
    .context("Failed to launch Chrome")
}

fn inject_cookies(tab: &Tab, cookies: &[StoredCookie]) -> Result<()> {
    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(3));

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

fn capture_cookies(tab: &Tab) -> Result<Vec<StoredCookie>> {
    let cdp_cookies = tab.call_method(Network::GetCookies {
        urls: Some(vec![TM3_BASE.to_string()]),
    })?;

    Ok(cdp_cookies
        .cookies
        .iter()
        .map(|c| StoredCookie {
            name: c.name.clone(),
            value: c.value.clone(),
            domain: c.domain.clone(),
            path: c.path.clone(),
            secure: c.secure,
            http_only: c.http_only,
            expires: if c.expires > 0.0 { Some(c.expires) } else { None },
        })
        .collect())
}

fn wait_for_auth(tab: &Tab) -> Result<()> {
    for attempt in 0..60 {
        std::thread::sleep(Duration::from_secs(2));
        let url = tab.get_url();
        if !url.contains("login") && !url.contains("Login") {
            return Ok(());
        }
        if attempt % 5 == 4 {
            eprintln!("[auth] Waiting... ({}s)", (attempt + 1) * 2);
        }
    }
    bail!("Timed out waiting for authentication (120s).")
}

fn wait_for_documents_page(tab: &Tab) -> Result<bool> {
    for _ in 0..10 {
        std::thread::sleep(Duration::from_secs(2));

        let check = tab
            .evaluate(
                r#"
                (function() {
                    var buttons = document.querySelectorAll('button');
                    for (var i = 0; i < buttons.length; i++) {
                        if (buttons[i].textContent.trim() === 'Attach File') return true;
                    }
                    var dz = document.querySelector('.drop-zone');
                    if (dz) return true;
                    return false;
                })()
                "#,
                false,
            )
            .ok()
            .and_then(|r| r.value)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if check {
            eprintln!("[upload] Documents page loaded.");
            return Ok(true);
        }
    }
    Ok(false)
}

fn navigate_via_search(tab: &Tab, tm3_id: &str) -> Result<()> {
    // Open Quick Search
    tab.evaluate(
        r#"
        (function() {
            var buttons = document.querySelectorAll('button');
            for (var i = 0; i < buttons.length; i++) {
                if (buttons[i].textContent.includes('Quick search')) {
                    buttons[i].click();
                    return;
                }
            }
        })()
        "#,
        false,
    )?;
    std::thread::sleep(Duration::from_secs(1));

    // Find and type into the search input using headless_chrome's type_into
    // (sends real key events, works with React)
    if let Ok(input) = tab.wait_for_element_with_custom_timeout(
        r#"input[placeholder*="Search"]"#,
        Duration::from_secs(5),
    ) {
        input.click()?;
        input.type_into(tm3_id)?;
        std::thread::sleep(Duration::from_secs(3));

        // Click the first search result
        tab.evaluate(
            r#"
            (function() {
                var results = document.querySelectorAll('[role="option"], li[class*="result"], a[href*="contacts/clients"]');
                if (results.length > 0) {
                    results[0].click();
                    return true;
                }
                return false;
            })()
            "#,
            false,
        )?;
        std::thread::sleep(Duration::from_secs(3));

        // Click Documents tab/link
        tab.evaluate(
            r#"
            (function() {
                var links = document.querySelectorAll('a, button, [role="tab"]');
                for (var i = 0; i < links.length; i++) {
                    var text = links[i].textContent.trim().toLowerCase();
                    if (text === 'documents' || text === 'files') {
                        links[i].click();
                        return true;
                    }
                }
                return false;
            })()
            "#,
            false,
        )?;
        std::thread::sleep(Duration::from_secs(3));

        if wait_for_documents_page(tab)? {
            return Ok(());
        }
    }

    bail!("Could not navigate to documents page via search. Try TM3_VISIBLE=1 to debug.")
}
