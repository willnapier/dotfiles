//! TM3 headless browser spike — proof of concept.
//!
//! Logs into TM3 at changeofharleystreet.tm3app.com, navigates to a client's
//! documents page, and inspects the upload widget DOM structure.
//!
//! Credentials via env vars: TM3_USER, TM3_PASS
//! Client TM3 ID via arg: tm3-spike <tm3_id>
//!
//! This is a throwaway spike — it exists to discover:
//! 1. Login flow (form selectors, redirects, MFA?)
//! 2. Client documents page URL pattern
//! 3. Upload widget DOM structure (for headless_chrome file_chooser interception)

use anyhow::{bail, Context, Result};
use headless_chrome::{Browser, LaunchOptions};
use std::time::Duration;

const TM3_BASE: &str = "https://changeofharleystreet.tm3app.com";

fn main() -> Result<()> {
    let tm3_id = std::env::args()
        .nth(1)
        .context("Usage: tm3-spike <tm3_id>")?;

    let user =
        std::env::var("TM3_USER").context("TM3_USER env var required (TM3 login email)")?;
    let pass =
        std::env::var("TM3_PASS").context("TM3_PASS env var required (TM3 login password)")?;

    eprintln!("[spike] Launching Chrome...");
    let browser = Browser::new(
        LaunchOptions::default_builder()
            .headless(false) // visible for spike — switch to true for automation
            .window_size(Some((1280, 900)))
            .idle_browser_timeout(Duration::from_secs(120))
            .build()
            .context("Failed to build launch options")?,
    )
    .context("Failed to launch Chrome — is it installed?")?;

    let tab = browser.new_tab().context("Failed to open new tab")?;
    tab.set_default_timeout(Duration::from_secs(30));

    // --- Phase 1: Navigate to login page ---
    eprintln!("[spike] Navigating to TM3 login...");
    tab.navigate_to(TM3_BASE)
        .context("Failed to navigate to TM3")?;
    tab.wait_until_navigated()
        .context("Timed out waiting for TM3 page load")?;

    // Dump the page title and URL to understand where we landed
    let url = tab.get_url();
    eprintln!("[spike] Landed at: {}", url);

    // Try to find login form elements — inspect what's on the page
    eprintln!("[spike] Inspecting page for login form...");
    let _page_html = tab
        .evaluate("document.documentElement.outerHTML", false)
        .context("Failed to get page HTML")?;

    // Extract and report form-related elements
    let form_info = tab
        .evaluate(
            r#"
            (function() {
                var info = {};

                // Find all input elements
                var inputs = document.querySelectorAll('input');
                info.inputs = Array.from(inputs).map(function(el) {
                    return {
                        type: el.type,
                        name: el.name,
                        id: el.id,
                        placeholder: el.placeholder,
                        class: el.className
                    };
                });

                // Find all forms
                var forms = document.querySelectorAll('form');
                info.forms = Array.from(forms).map(function(el) {
                    return {
                        action: el.action,
                        method: el.method,
                        id: el.id,
                        class: el.className
                    };
                });

                // Find submit buttons
                var buttons = document.querySelectorAll('button, input[type="submit"]');
                info.buttons = Array.from(buttons).map(function(el) {
                    return {
                        tag: el.tagName,
                        type: el.type,
                        text: el.textContent.trim().substring(0, 50),
                        id: el.id,
                        class: el.className
                    };
                });

                return JSON.stringify(info, null, 2);
            })()
            "#,
            false,
        )
        .context("Failed to inspect login form")?;

    eprintln!("[spike] Login page structure:");
    if let Some(val) = form_info.value {
        if let Some(s) = val.as_str() {
            println!("{}", s);
        } else {
            println!("{}", val);
        }
    }

    // --- Phase 2: Attempt login ---
    eprintln!("[spike] Attempting login...");

    // Try common selectors for email/username field
    let email_selectors = [
        r#"input[type="email"]"#,
        r#"input[name="email"]"#,
        r#"input[name="username"]"#,
        r#"input[name="Email"]"#,
        r#"input[name="UserName"]"#,
        r#"input[id="Email"]"#,
        r#"input[id="email"]"#,
        r#"input[type="text"]"#,
    ];

    let password_selectors = [
        r#"input[type="password"]"#,
        r#"input[name="password"]"#,
        r#"input[name="Password"]"#,
        r#"input[id="Password"]"#,
        r#"input[id="password"]"#,
    ];

    let mut email_found = false;
    for sel in &email_selectors {
        if let Ok(el) = tab.find_element(sel) {
            eprintln!("[spike] Found email field: {}", sel);
            el.click()?;
            el.type_into(&user)?;
            email_found = true;
            break;
        }
    }
    if !email_found {
        bail!("Could not find email/username input field. Check the login page structure above.");
    }

    let mut pass_found = false;
    for sel in &password_selectors {
        if let Ok(el) = tab.find_element(sel) {
            eprintln!("[spike] Found password field: {}", sel);
            el.click()?;
            el.type_into(&pass)?;
            pass_found = true;
            break;
        }
    }
    if !pass_found {
        bail!("Could not find password input field. Check the login page structure above.");
    }

    // Find and click the submit button
    let submit_selectors = [
        r#"button[type="submit"]"#,
        r#"input[type="submit"]"#,
        r#"button.btn-primary"#,
        r#"button.login-btn"#,
        r#"#loginButton"#,
        r#"button"#, // last resort: first button
    ];

    let mut submitted = false;
    for sel in &submit_selectors {
        if let Ok(el) = tab.find_element(sel) {
            eprintln!("[spike] Clicking submit: {}", sel);
            el.click()?;
            submitted = true;
            break;
        }
    }
    if !submitted {
        // Try submitting the form directly
        eprintln!("[spike] No button found, submitting form via JS...");
        tab.evaluate("document.forms[0].submit()", false)?;
    }

    // Wait for navigation after login
    eprintln!("[spike] Waiting for post-login navigation...");
    std::thread::sleep(Duration::from_secs(5));

    let post_login_url = tab.get_url();
    eprintln!("[spike] Post-login URL: {}", post_login_url);

    // Check if login succeeded (we're no longer on the login page)
    if post_login_url.contains("login") || post_login_url.contains("Login") {
        eprintln!("[spike] WARNING: May still be on login page. Check for MFA or error.");
        let error_info = tab
            .evaluate(
                r#"
                (function() {
                    var errors = document.querySelectorAll('.error, .alert, .validation-summary-errors, .text-danger');
                    return Array.from(errors).map(function(el) { return el.textContent.trim(); }).join('\n');
                })()
                "#,
                false,
            )
            .ok();
        if let Some(info) = error_info {
            if let Some(val) = info.value {
                eprintln!("[spike] Error messages on page: {}", val);
            }
        }
    }

    // --- Phase 3: Navigate to client documents page ---
    // TM3 URL pattern is typically /Patient/Documents/<id> or similar
    let doc_url_candidates = [
        format!("{}/Patient/{}/Documents", TM3_BASE, tm3_id),
        format!("{}/Patient/Documents/{}", TM3_BASE, tm3_id),
        format!("{}/patients/{}/documents", TM3_BASE, tm3_id),
        format!("{}/#/patients/{}/documents", TM3_BASE, tm3_id),
    ];

    eprintln!("[spike] Navigating to client {} documents page...", tm3_id);
    eprintln!("[spike] Will try URL patterns in order.");

    // First, let's see what the main app URL structure looks like
    let nav_info = tab
        .evaluate(
            r#"
            (function() {
                var links = document.querySelectorAll('a[href]');
                var hrefs = Array.from(links).map(function(a) { return a.href; })
                    .filter(function(h) { return h.includes('Patient') || h.includes('patient') || h.includes('Document') || h.includes('document'); });
                return JSON.stringify(hrefs.slice(0, 20));
            })()
            "#,
            false,
        )
        .ok();

    if let Some(info) = nav_info {
        if let Some(val) = info.value {
            eprintln!("[spike] Relevant navigation links found:");
            if let Some(s) = val.as_str() {
                println!("{}", s);
            } else {
                println!("{}", val);
            }
        }
    }

    // Try each URL pattern
    for candidate_url in &doc_url_candidates {
        eprintln!("[spike] Trying: {}", candidate_url);
        if tab.navigate_to(candidate_url).is_ok() {
            std::thread::sleep(Duration::from_secs(3));
            let current = tab.get_url();
            eprintln!("[spike] Landed at: {}", current);

            // If we didn't get redirected back to login or to an error, this might be it
            if !current.contains("login") && !current.contains("Login") && !current.contains("404")
            {
                eprintln!("[spike] This URL pattern appears to work.");

                // Inspect the upload widget
                inspect_upload_widget(&tab)?;
                break;
            }
        }
    }

    // --- Phase 4: Comprehensive page inspection ---
    eprintln!("[spike] Final page state inspection...");
    let final_info = tab
        .evaluate(
            r#"
            (function() {
                var info = {};
                info.url = window.location.href;
                info.title = document.title;

                // Look for file input elements
                var fileInputs = document.querySelectorAll('input[type="file"]');
                info.fileInputs = Array.from(fileInputs).map(function(el) {
                    return {
                        name: el.name,
                        id: el.id,
                        accept: el.accept,
                        multiple: el.multiple,
                        class: el.className,
                        visible: el.offsetParent !== null
                    };
                });

                // Look for upload-related buttons/areas
                var uploadElements = document.querySelectorAll('[class*="upload"], [class*="Upload"], [id*="upload"], [id*="Upload"], [class*="drop"], [class*="Drop"]');
                info.uploadElements = Array.from(uploadElements).map(function(el) {
                    return {
                        tag: el.tagName,
                        id: el.id,
                        class: el.className,
                        text: el.textContent.trim().substring(0, 100)
                    };
                });

                // Look for drag-and-drop zones
                var dropzones = document.querySelectorAll('[dropzone], [ondrop], [ondragover]');
                info.dropzones = Array.from(dropzones).map(function(el) {
                    return {
                        tag: el.tagName,
                        id: el.id,
                        class: el.className
                    };
                });

                return JSON.stringify(info, null, 2);
            })()
            "#,
            false,
        )
        .context("Failed to inspect final page state")?;

    eprintln!("\n[spike] === UPLOAD WIDGET ANALYSIS ===");
    if let Some(val) = final_info.value {
        if let Some(s) = val.as_str() {
            println!("{}", s);
        } else {
            println!("{}", val);
        }
    }

    // Keep browser open for manual inspection
    eprintln!("\n[spike] Browser left open for manual inspection.");
    eprintln!("[spike] Press Enter to close...");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    Ok(())
}

fn inspect_upload_widget(tab: &headless_chrome::Tab) -> Result<()> {
    eprintln!("[spike] Inspecting upload widget DOM...");

    let widget_info = tab
        .evaluate(
            r#"
            (function() {
                var info = {};

                // Find all iframes (upload might be in an iframe)
                var iframes = document.querySelectorAll('iframe');
                info.iframes = Array.from(iframes).map(function(el) {
                    return { src: el.src, id: el.id, name: el.name };
                });

                // Find file inputs (visible and hidden)
                var fileInputs = document.querySelectorAll('input[type="file"]');
                info.fileInputs = Array.from(fileInputs).map(function(el) {
                    var rect = el.getBoundingClientRect();
                    return {
                        name: el.name,
                        id: el.id,
                        accept: el.accept,
                        multiple: el.multiple,
                        class: el.className,
                        visible: el.offsetParent !== null,
                        width: rect.width,
                        height: rect.height
                    };
                });

                // Look for any element with upload/attach semantics
                var uploadTriggers = document.querySelectorAll(
                    'button[class*="upload" i], button[class*="attach" i], ' +
                    'a[class*="upload" i], a[class*="attach" i], ' +
                    '[role="button"][class*="upload" i], ' +
                    '.k-upload, .k-dropzone, .kendo-upload, ' +  // Kendo UI
                    '.dz-clickable, .dropzone, ' +                 // Dropzone.js
                    '.fine-uploader, .qq-upload-button'            // Fine Uploader
                );
                info.uploadTriggers = Array.from(uploadTriggers).map(function(el) {
                    return {
                        tag: el.tagName,
                        id: el.id,
                        class: el.className,
                        text: el.textContent.trim().substring(0, 80),
                        ariaLabel: el.getAttribute('aria-label')
                    };
                });

                // Check for known upload library globals
                info.libraries = {
                    kendo: typeof kendo !== 'undefined',
                    Dropzone: typeof Dropzone !== 'undefined',
                    qq: typeof qq !== 'undefined',
                    jQuery: typeof jQuery !== 'undefined',
                    angular: typeof angular !== 'undefined',
                    React: typeof React !== 'undefined' || !!document.querySelector('[data-reactroot]')
                };

                return JSON.stringify(info, null, 2);
            })()
            "#,
            false,
        )
        .context("Failed to inspect upload widget")?;

    eprintln!("[spike] Upload widget DOM structure:");
    if let Some(val) = widget_info.value {
        if let Some(s) = val.as_str() {
            println!("{}", s);
        } else {
            println!("{}", val);
        }
    }

    Ok(())
}
