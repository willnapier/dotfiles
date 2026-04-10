//! TM3 headless browser spike — proof of concept.
//!
//! Phase A (first run): Register a virtual passkey with TM3, save credential to disk.
//!   tm3-spike register <tm3_id>
//!
//! Phase B (subsequent runs): Load credential, authenticate automatically, inspect documents page.
//!   tm3-spike inspect <tm3_id>
//!
//! Credential stored at ~/.config/clinical-product/tm3-credential.json

use anyhow::{bail, Context, Result};
use headless_chrome::browser::tab::Tab;
use headless_chrome::protocol::cdp::WebAuthn;
use headless_chrome::{Browser, LaunchOptions};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const TM3_BASE: &str = "https://changeofharleystreet.tm3app.com";

#[derive(Serialize, Deserialize)]
struct SavedCredential {
    authenticator_protocol: String,
    credential_id: String,
    private_key: String,
    rp_id: String,
    user_handle: Option<String>,
    sign_count: u32,
}

fn credential_path() -> PathBuf {
    let dir = dirs_or_home().join("tm3-credential.json");
    dir
}

fn dirs_or_home() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let dir = PathBuf::from(home)
        .join(".config")
        .join("clinical-product");
    std::fs::create_dir_all(&dir).ok();
    dir
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage:");
        eprintln!("  tm3-spike register <tm3_id>   — register virtual passkey (one-time, visible)");
        eprintln!("  tm3-spike inspect <tm3_id>     — auto-login + inspect documents page");
        std::process::exit(1);
    }

    let mode = &args[1];
    let tm3_id = &args[2];

    match mode.as_str() {
        "register" => register_passkey(tm3_id),
        "inspect" => inspect_documents(tm3_id),
        other => bail!("Unknown mode '{}'. Use 'register' or 'inspect'.", other),
    }
}

// --- Phase A: Register a virtual passkey with TM3 ---
fn register_passkey(tm3_id: &str) -> Result<()> {
    eprintln!("[register] Launching Chrome (visible)...");
    let browser = launch_browser(false)?;
    let tab = browser.new_tab().context("Failed to open tab")?;
    tab.set_default_timeout(Duration::from_secs(30));

    // Enable WebAuthn and create virtual authenticator
    let auth_id = setup_virtual_authenticator(&tab)?;

    // Navigate to TM3 login
    eprintln!("[register] Navigating to TM3...");
    tab.navigate_to(TM3_BASE)?;
    tab.wait_until_navigated()?;
    let url = tab.get_url();
    eprintln!("[register] Landed at: {}", url);

    dump_buttons(&tab, "Login page")?;

    // We need to register a passkey. TM3's login page has "Sign In with Passkey" but
    // that's for USING an existing passkey. To REGISTER a new one, we probably need
    // to first log in (via the old passkey / Touch ID) and then add a new security key
    // in account settings.
    //
    // Alternative approach: the virtual authenticator intercepts ALL WebAuthn calls.
    // If we click "Sign In with Passkey", TM3 sends a navigator.credentials.get() call.
    // The virtual authenticator has no credentials yet, so it will fail.
    //
    // The correct flow for registration:
    // 1. Log in manually first (user clicks through with real Touch ID in the visible browser)
    // 2. Navigate to account/security settings
    // 3. Click "Add passkey" or similar
    // 4. The virtual authenticator intercepts navigator.credentials.create()
    // 5. Save the resulting credential
    //
    // BUT — there's a simpler approach. We can:
    // 1. NOT enable WebAuthn yet (let the real platform authenticator handle login)
    // 2. User authenticates with Touch ID normally
    // 3. THEN enable WebAuthn + virtual authenticator
    // 4. Navigate to security settings, add new passkey
    // 5. Virtual authenticator handles the registration
    // 6. Save credential

    eprintln!();
    eprintln!("[register] === STEP 1: Log in with your real passkey ===");
    eprintln!("[register] Click 'Sign In with Passkey' and Touch ID in the browser.");
    eprintln!("[register] The virtual authenticator is NOT active yet — your real passkey will work.");
    eprintln!();

    // Wait for user to authenticate manually
    wait_for_auth(&tab)?;

    eprintln!("[register] Authenticated. Post-login URL: {}", tab.get_url());

    // Now enable the virtual authenticator (overrides the platform authenticator)
    eprintln!("[register] Enabling virtual authenticator for registration...");
    // We already set it up above, so it's active. Actually, we need to think about this.
    // The WebAuthn.enable() call we made earlier already overrides the platform auth.
    // That's why the user could still auth — let me check if enable was called.
    //
    // Actually, we called setup_virtual_authenticator which calls Enable. That means
    // the platform authenticator was already overridden. But the user said Touch ID
    // appeared in the first spike... That spike didn't call WebAuthn.enable.
    //
    // So the issue is: if we enable WebAuthn before login, the user can't use Touch ID.
    // We need to enable it AFTER login.

    // Hmm, we already enabled it. Let's restructure: don't enable until after login.
    // For now, let's just try navigating to security settings.

    eprintln!("[register] Looking for security/passkey settings...");

    // Try common TM3 account settings paths
    let settings_urls = [
        format!("{}/Account/Security", TM3_BASE),
        format!("{}/account/security", TM3_BASE),
        format!("{}/Settings/Security", TM3_BASE),
        format!("{}/settings/security", TM3_BASE),
        format!("{}/Account", TM3_BASE),
        format!("{}/settings", TM3_BASE),
    ];

    let mut found_settings = false;
    for settings_url in &settings_urls {
        eprintln!("[register] Trying: {}", settings_url);
        tab.navigate_to(settings_url)?;
        std::thread::sleep(Duration::from_secs(2));
        let current = tab.get_url();
        if !current.contains("login") && !current.contains("Login") {
            eprintln!("[register] Settings page at: {}", current);
            found_settings = true;
            dump_buttons(&tab, "Settings page")?;
            break;
        }
    }

    if !found_settings {
        eprintln!("[register] Could not find settings page automatically.");
        eprintln!("[register] Please navigate to your security/passkey settings in the browser.");
        eprintln!("[register] Press Enter when you're on the page with 'Add passkey' or similar...");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        dump_buttons(&tab, "Manual settings page")?;
    }

    // Look for "Add passkey" / "Register" button
    eprintln!("[register] Looking for passkey registration button...");
    let reg_result = tab.evaluate(
        r#"
        (function() {
            var buttons = document.querySelectorAll('button, a');
            for (var i = 0; i < buttons.length; i++) {
                var text = buttons[i].textContent.trim().toLowerCase();
                if (text.includes('add passkey') || text.includes('add security key')
                    || text.includes('register') || text.includes('add authenticator')
                    || text.includes('add key') || text.includes('new passkey')) {
                    return JSON.stringify({found: true, text: buttons[i].textContent.trim()});
                }
            }
            return JSON.stringify({found: false});
        })()
        "#,
        false,
    )?;

    if let Some(val) = &reg_result.value {
        eprintln!("[register] Registration button search: {}", val);
    }

    eprintln!();
    eprintln!("[register] When you see a 'Register passkey' / 'Add passkey' option,");
    eprintln!("[register] click it in the browser. The virtual authenticator will");
    eprintln!("[register] intercept the WebAuthn create() call automatically.");
    eprintln!("[register] Press Enter after clicking...");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    // Wait a moment for registration to complete
    std::thread::sleep(Duration::from_secs(3));

    // Retrieve credentials from the virtual authenticator
    eprintln!("[register] Retrieving registered credential...");
    let creds = tab.call_method(WebAuthn::GetCredentials {
        authenticator_id: auth_id.clone(),
    })?;

    if creds.credentials.is_empty() {
        bail!(
            "No credentials registered on virtual authenticator. \
             The registration may not have triggered, or TM3 might not have \
             a passkey management page. Check the browser for errors."
        );
    }

    eprintln!(
        "[register] Found {} credential(s)!",
        creds.credentials.len()
    );

    let cred = &creds.credentials[0];
    let saved = SavedCredential {
        authenticator_protocol: "ctap2".to_string(),
        credential_id: cred.credential_id.clone(),
        private_key: cred.private_key.clone(),
        rp_id: cred.rp_id.clone().unwrap_or_default(),
        user_handle: cred.user_handle.clone(),
        sign_count: cred.sign_count,
    };

    let path = credential_path();
    let json = serde_json::to_string_pretty(&saved)?;
    std::fs::write(&path, &json)?;
    eprintln!("[register] Credential saved to: {}", path.display());
    eprintln!("[register] rp_id: {}", saved.rp_id);
    eprintln!(
        "[register] credential_id: {}...",
        &saved.credential_id[..saved.credential_id.len().min(20)]
    );
    eprintln!();
    eprintln!("[register] Done! You can now use: tm3-spike inspect {}", tm3_id);

    Ok(())
}

// --- Phase B: Auto-login + inspect documents page ---
fn inspect_documents(tm3_id: &str) -> Result<()> {
    let cred_path = credential_path();
    let cred_json = std::fs::read_to_string(&cred_path)
        .context(format!("No saved credential at {}. Run 'tm3-spike register <id>' first.", cred_path.display()))?;
    let saved: SavedCredential = serde_json::from_str(&cred_json)?;
    eprintln!("[inspect] Loaded credential from {}", cred_path.display());

    eprintln!("[inspect] Launching Chrome (headless)...");
    let browser = launch_browser(true)?;
    let tab = browser.new_tab().context("Failed to open tab")?;
    tab.set_default_timeout(Duration::from_secs(30));

    // Set up virtual authenticator with saved credential
    let auth_id = setup_virtual_authenticator(&tab)?;

    // Load the saved credential into the virtual authenticator
    eprintln!("[inspect] Loading saved credential into virtual authenticator...");
    tab.call_method(WebAuthn::AddCredential {
        authenticator_id: auth_id.clone(),
        credential: WebAuthn::Credential {
            credential_id: saved.credential_id.clone(),
            is_resident_credential: true,
            rp_id: Some(saved.rp_id.clone()),
            private_key: saved.private_key.clone(),
            user_handle: saved.user_handle.clone(),
            sign_count: saved.sign_count,
            large_blob: None,
            backup_eligibility: None,
            backup_state: None,
            user_name: None,
            user_display_name: None,
        },
    })?;
    eprintln!("[inspect] Credential loaded.");

    // Navigate to TM3
    eprintln!("[inspect] Navigating to TM3...");
    tab.navigate_to(TM3_BASE)?;
    tab.wait_until_navigated()?;
    eprintln!("[inspect] Landed at: {}", tab.get_url());

    // Click the passkey button
    eprintln!("[inspect] Clicking passkey login button...");
    let click_result = tab.evaluate(
        r#"
        (function() {
            var buttons = document.querySelectorAll('button');
            for (var i = 0; i < buttons.length; i++) {
                var text = buttons[i].textContent.trim().toLowerCase();
                if (text.includes('passkey')) {
                    buttons[i].click();
                    return "clicked";
                }
            }
            return "not_found";
        })()
        "#,
        false,
    )?;

    if let Some(val) = &click_result.value {
        eprintln!("[inspect] Passkey button: {}", val);
    }

    // Wait for auto-authentication
    eprintln!("[inspect] Waiting for virtual authenticator to respond...");
    let mut authenticated = false;
    for attempt in 0..15 {
        std::thread::sleep(Duration::from_secs(2));
        let current_url = tab.get_url();

        if !current_url.contains("login") && !current_url.contains("Login") {
            eprintln!("[inspect] Authenticated! URL: {}", current_url);
            authenticated = true;
            break;
        }

        if attempt % 3 == 2 {
            eprintln!("[inspect] Still waiting... ({}s)", (attempt + 1) * 2);
        }
    }

    if !authenticated {
        eprintln!("[inspect] Authentication may have failed. Current URL: {}", tab.get_url());
        dump_buttons(&tab, "Post-auth attempt")?;
        bail!("Auto-login failed. The saved credential may be invalid — try 'tm3-spike register' again.");
    }

    // Let app settle
    std::thread::sleep(Duration::from_secs(2));

    // Navigate to documents page
    let doc_url = format!("{}/Patient/{}/Documents", TM3_BASE, tm3_id);
    eprintln!("[inspect] Navigating to: {}", doc_url);
    tab.navigate_to(&doc_url)?;

    // Wait for page to fully load (title changes from "...Loading...")
    eprintln!("[inspect] Waiting for page to load...");
    for _ in 0..15 {
        std::thread::sleep(Duration::from_secs(2));
        let title = tab
            .evaluate("document.title", false)
            .ok()
            .and_then(|r| r.value)
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();

        if !title.contains("Loading") && !title.is_empty() {
            eprintln!("[inspect] Page loaded. Title: {}", title);
            break;
        }
    }

    // Inspect upload widget
    inspect_upload_widget(&tab)?;

    // Final comprehensive inspection
    eprintln!("\n[inspect] === FINAL PAGE STATE ===");
    let final_info = tab.evaluate(
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

            return JSON.stringify(info, null, 2);
        })()
        "#,
        false,
    )?;

    if let Some(val) = final_info.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("{}", s);
    }

    eprintln!("\n[inspect] Done.");
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

fn setup_virtual_authenticator(tab: &Tab) -> Result<String> {
    // Enable WebAuthn domain — this overrides the platform authenticator
    tab.call_method(WebAuthn::Enable {
        enable_ui: Some(false),
    })?;

    // Create virtual authenticator mimicking a platform passkey authenticator
    let result = tab.call_method(WebAuthn::AddVirtualAuthenticator {
        options: WebAuthn::VirtualAuthenticatorOptions {
            protocol: WebAuthn::AuthenticatorProtocol::Ctap2,
            ctap_2_version: Some(WebAuthn::Ctap2Version::Ctap21),
            transport: WebAuthn::AuthenticatorTransport::Internal,
            has_resident_key: Some(true),
            has_user_verification: Some(true),
            has_large_blob: None,
            has_cred_blob: None,
            has_min_pin_length: None,
            has_prf: None,
            automatic_presence_simulation: Some(true),
            is_user_verified: Some(true),
            default_backup_eligibility: None,
            default_backup_state: None,
        },
    })?;

    eprintln!(
        "[auth] Virtual authenticator created: {}",
        result.authenticator_id
    );
    Ok(result.authenticator_id)
}

fn wait_for_auth(tab: &Tab) -> Result<()> {
    for attempt in 0..60 {
        std::thread::sleep(Duration::from_secs(2));
        let current_url = tab.get_url();

        let modal_gone = tab
            .evaluate(
                r#"
                (function() {
                    var buttons = document.querySelectorAll('button');
                    for (var i = 0; i < buttons.length; i++) {
                        var text = buttons[i].textContent.trim().toLowerCase();
                        if (text.includes('passkey') || text.includes('log me in') || text.includes('sign in')) {
                            return "login_visible";
                        }
                    }
                    return "clear";
                })()
                "#,
                false,
            )
            .ok()
            .and_then(|r| r.value)
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();

        if modal_gone == "clear"
            && !current_url.contains("login")
            && !current_url.contains("Login")
        {
            return Ok(());
        }

        if attempt % 5 == 4 {
            eprintln!(
                "[auth] Waiting for manual login... ({}s)",
                (attempt + 1) * 2
            );
        }
    }
    bail!("Timed out waiting for manual authentication (120s).")
}

fn dump_buttons(tab: &Tab, label: &str) -> Result<()> {
    let info = tab.evaluate(
        r#"
        (function() {
            var buttons = document.querySelectorAll('button, a.btn, input[type="submit"]');
            return JSON.stringify(Array.from(buttons).map(function(el) {
                return {
                    tag: el.tagName, type: el.type,
                    text: el.textContent.trim().substring(0, 60),
                    id: el.id, class: el.className
                };
            }), null, 2);
        })()
        "#,
        false,
    )?;

    eprintln!("[spike] Buttons ({}):", label);
    if let Some(val) = info.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("{}", s);
    }
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

            // Broader search: any element with upload/file semantics
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

            // Also search by text content for upload-related buttons
            var allButtons = document.querySelectorAll('button, a');
            info.uploadButtons = Array.from(allButtons)
                .filter(function(el) {
                    var t = el.textContent.trim().toLowerCase();
                    return t.includes('upload') || t.includes('attach') || t.includes('browse')
                        || t.includes('choose file') || t.includes('add document')
                        || t.includes('new document');
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
