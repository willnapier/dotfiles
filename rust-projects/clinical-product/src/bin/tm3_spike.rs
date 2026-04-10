//! TM3 headless browser spike — proof of concept.
//!
//! Launches Chrome (visible), navigates to TM3, clicks "Sign me in with Pass Key",
//! waits for passkey auth, then navigates to a client's documents page and inspects
//! the upload widget DOM structure.
//!
//! Usage: tm3-spike <tm3_id>

use anyhow::{bail, Context, Result};
use headless_chrome::{Browser, LaunchOptions};
use std::time::Duration;

const TM3_BASE: &str = "https://changeofharleystreet.tm3app.com";

fn main() -> Result<()> {
    let tm3_id = std::env::args()
        .nth(1)
        .context("Usage: tm3-spike <tm3_id>")?;

    eprintln!("[spike] Launching Chrome...");
    let browser = Browser::new(
        LaunchOptions::default_builder()
            .headless(false)
            .window_size(Some((1280, 900)))
            .idle_browser_timeout(Duration::from_secs(120))
            .build()
            .context("Failed to build launch options")?,
    )
    .context("Failed to launch Chrome — is it installed?")?;

    let tab = browser.new_tab().context("Failed to open new tab")?;
    tab.set_default_timeout(Duration::from_secs(30));

    // --- Phase 1: Navigate to TM3 ---
    eprintln!("[spike] Navigating to TM3...");
    tab.navigate_to(TM3_BASE)
        .context("Failed to navigate to TM3")?;
    tab.wait_until_navigated()
        .context("Timed out waiting for TM3 page load")?;

    let url = tab.get_url();
    eprintln!("[spike] Landed at: {}", url);

    // Dump page structure for diagnostics
    dump_page_structure(&tab, "Initial page")?;

    // --- Phase 2: Find and click passkey button ---
    eprintln!("[spike] Looking for passkey login button...");

    // Search by text content — the button says "Sign me in with Pass Key"
    let passkey_clicked = tab
        .evaluate(
            r#"
            (function() {
                var buttons = document.querySelectorAll('button, a, input[type="submit"]');
                for (var i = 0; i < buttons.length; i++) {
                    var text = buttons[i].textContent.trim().toLowerCase();
                    if (text.includes('pass key') || text.includes('passkey')) {
                        buttons[i].click();
                        return JSON.stringify({found: true, text: buttons[i].textContent.trim(), tag: buttons[i].tagName, class: buttons[i].className, id: buttons[i].id});
                    }
                }
                return JSON.stringify({found: false});
            })()
            "#,
            false,
        )
        .context("Failed to search for passkey button")?;

    if let Some(val) = &passkey_clicked.value {
        let s = val.as_str().unwrap_or("");
        eprintln!("[spike] Passkey button search result: {}", s);
        if s.contains("\"found\":false") || s.contains("\"found\": false") {
            eprintln!("[spike] No passkey button found on initial page.");
            eprintln!("[spike] The login page may look different from a fresh session.");
            eprintln!("[spike] Dumping all clickable elements...");
            dump_page_structure(&tab, "No passkey found")?;
            bail!("Could not find passkey button. See page structure dump above.");
        }
    }

    // Wait for passkey authentication — user needs to Touch ID
    eprintln!();
    eprintln!("[spike] ========================================");
    eprintln!("[spike]  PASSKEY TRIGGERED — authenticate now   ");
    eprintln!("[spike]  (Touch ID / Apple Watch / system prompt)");
    eprintln!("[spike] ========================================");
    eprintln!();

    // Poll until we're past the login page
    let mut authenticated = false;
    for attempt in 0..30 {
        std::thread::sleep(Duration::from_secs(2));
        let current_url = tab.get_url();

        // Check if the "Still here?" / login modal has disappeared
        let modal_gone = tab
            .evaluate(
                r#"
                (function() {
                    // Check if we're on a real app page (no login modal visible)
                    var buttons = document.querySelectorAll('button');
                    for (var i = 0; i < buttons.length; i++) {
                        var text = buttons[i].textContent.trim().toLowerCase();
                        if (text.includes('pass key') || text.includes('passkey') || text.includes('log me in')) {
                            return "modal_still_visible";
                        }
                    }
                    return "clear";
                })()
                "#,
                false,
            )
            .ok();

        let status = modal_gone
            .as_ref()
            .and_then(|r| r.value.as_ref())
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        if status == "clear" && !current_url.contains("login") && !current_url.contains("Login") {
            eprintln!(
                "[spike] Authenticated! (attempt {}, url: {})",
                attempt + 1,
                current_url
            );
            authenticated = true;
            break;
        }

        if attempt % 5 == 4 {
            eprintln!(
                "[spike] Still waiting for auth... ({}s, url: {})",
                (attempt + 1) * 2,
                current_url
            );
        }
    }

    if !authenticated {
        bail!("Timed out waiting for passkey authentication (60s). Did Touch ID appear?");
    }

    // Let the app settle after auth
    std::thread::sleep(Duration::from_secs(2));

    // --- Phase 3: Discover URL structure ---
    eprintln!("[spike] Discovering TM3 URL structure...");
    let post_login_url = tab.get_url();
    eprintln!("[spike] Post-login URL: {}", post_login_url);

    // Scrape navigation links to understand URL patterns
    let nav_links = tab
        .evaluate(
            r#"
            (function() {
                var links = document.querySelectorAll('a[href]');
                var hrefs = Array.from(links).map(function(a) {
                    return {href: a.href, text: a.textContent.trim().substring(0, 60)};
                }).filter(function(item) {
                    var h = item.href.toLowerCase();
                    return h.includes('patient') || h.includes('client')
                        || h.includes('document') || h.includes('diary');
                });
                return JSON.stringify(hrefs.slice(0, 30), null, 2);
            })()
            "#,
            false,
        )
        .ok();

    if let Some(info) = nav_links {
        if let Some(val) = info.value {
            eprintln!("[spike] Navigation links (patient/document/diary):");
            let fallback = val.to_string();
            let s = val.as_str().unwrap_or(&fallback);
            println!("{}", s);
        }
    }

    // --- Phase 4: Navigate to client documents page ---
    let doc_url_candidates = [
        format!("{}/Patient/{}/Documents", TM3_BASE, tm3_id),
        format!("{}/Patient/Documents/{}", TM3_BASE, tm3_id),
        format!("{}/patients/{}/documents", TM3_BASE, tm3_id),
        format!("{}/#/patients/{}/documents", TM3_BASE, tm3_id),
        format!("{}/#/Patient/{}/Documents", TM3_BASE, tm3_id),
    ];

    eprintln!(
        "[spike] Navigating to client {} documents page...",
        tm3_id
    );

    let mut found_docs = false;
    for candidate_url in &doc_url_candidates {
        eprintln!("[spike] Trying: {}", candidate_url);
        if tab.navigate_to(candidate_url).is_ok() {
            std::thread::sleep(Duration::from_secs(3));
            let current = tab.get_url();
            eprintln!("[spike] → Landed at: {}", current);

            if !current.contains("login") && !current.contains("Login") {
                eprintln!("[spike] URL pattern appears to work.");
                found_docs = true;
                inspect_upload_widget(&tab)?;
                break;
            }
        }
    }

    if !found_docs {
        eprintln!("[spike] None of the guessed URL patterns worked.");
        eprintln!("[spike] Navigate manually in the browser to the documents page,");
        eprintln!("[spike] then press Enter and I'll inspect whatever page you're on.");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;

        let manual_url = tab.get_url();
        eprintln!("[spike] Manual navigation landed at: {}", manual_url);
        inspect_upload_widget(&tab)?;
    }

    // --- Phase 5: Final comprehensive inspection ---
    eprintln!("\n[spike] === FINAL PAGE STATE ===");
    let final_info = tab
        .evaluate(
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

                var dropzones = document.querySelectorAll('[dropzone], [ondrop], [ondragover]');
                info.dropzones = Array.from(dropzones).map(function(el) {
                    return { tag: el.tagName, id: el.id, class: el.className };
                });

                return JSON.stringify(info, null, 2);
            })()
            "#,
            false,
        )
        .context("Failed to inspect final page state")?;

    if let Some(val) = final_info.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("{}", s);
    }

    eprintln!("\n[spike] Browser left open for manual inspection.");
    eprintln!("[spike] Press Enter to close...");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    Ok(())
}

fn dump_page_structure(tab: &headless_chrome::Tab, label: &str) -> Result<()> {
    let info = tab
        .evaluate(
            r#"
            (function() {
                var info = {};

                var inputs = document.querySelectorAll('input');
                info.inputs = Array.from(inputs).map(function(el) {
                    return {
                        type: el.type, name: el.name, id: el.id,
                        placeholder: el.placeholder, class: el.className
                    };
                });

                var forms = document.querySelectorAll('form');
                info.forms = Array.from(forms).map(function(el) {
                    return {
                        action: el.action, method: el.method,
                        id: el.id, class: el.className
                    };
                });

                var buttons = document.querySelectorAll('button, input[type="submit"], a.btn');
                info.buttons = Array.from(buttons).map(function(el) {
                    return {
                        tag: el.tagName, type: el.type,
                        text: el.textContent.trim().substring(0, 60),
                        id: el.id, class: el.className
                    };
                });

                return JSON.stringify(info, null, 2);
            })()
            "#,
            false,
        )
        .context("Failed to dump page structure")?;

    eprintln!("[spike] Page structure ({}):", label);
    if let Some(val) = info.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("{}", s);
    }
    Ok(())
}

fn inspect_upload_widget(tab: &headless_chrome::Tab) -> Result<()> {
    eprintln!("[spike] Inspecting upload widget DOM...");

    let widget_info = tab
        .evaluate(
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

                var uploadTriggers = document.querySelectorAll(
                    'button[class*="upload" i], button[class*="attach" i], ' +
                    'a[class*="upload" i], a[class*="attach" i], ' +
                    '[role="button"][class*="upload" i], ' +
                    '.k-upload, .k-dropzone, .kendo-upload, ' +
                    '.dz-clickable, .dropzone, ' +
                    '.fine-uploader, .qq-upload-button'
                );
                info.uploadTriggers = Array.from(uploadTriggers).map(function(el) {
                    return {
                        tag: el.tagName, id: el.id, class: el.className,
                        text: el.textContent.trim().substring(0, 80),
                        ariaLabel: el.getAttribute('aria-label')
                    };
                });

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

    eprintln!("[spike] Upload widget DOM:");
    if let Some(val) = widget_info.value {
        let fallback = val.to_string();
        let s = val.as_str().unwrap_or(&fallback);
        println!("{}", s);
    }

    Ok(())
}
