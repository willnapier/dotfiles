//! Zero-shot client onboarding pipeline.
//!
//! When a new client appears in TM3 (created by Olly), this module
//! automatically: scrapes their TM3 profile, derives a client ID,
//! scaffolds their directory, populates identity.yaml, updates
//! tm3-client-map, downloads documents, and imports them.

use anyhow::{bail, Context, Result};
use headless_chrome::protocol::cdp::Network;
use headless_chrome::{Browser, LaunchOptions};
use regex::Regex;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use crate::session_cookies;

const TM3_BASE: &str = "https://changeofharleystreet.tm3app.com";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Metadata scraped from a TM3 client profile page.
#[derive(Debug, Clone)]
pub struct TM3Profile {
    pub tm3_id: String,
    pub full_name: String,
    pub dob: Option<String>,
    pub referrer_name: Option<String>,
    pub referrer_practice: Option<String>,
    pub referrer_email: Option<String>,
    pub funding_source: Option<String>,
    pub policy_number: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
}

/// Result of the onboarding pipeline.
#[derive(Debug)]
pub struct OnboardResult {
    pub client_id: String,
    pub tm3_id: String,
    pub name: String,
    pub docs_imported: usize,
    pub skipped: bool,
}

// ---------------------------------------------------------------------------
// TM3 profile scraper
// ---------------------------------------------------------------------------

/// Launch a headless Chrome instance with TM3 session cookies.
fn launch_tm3_browser() -> Result<(Browser, std::sync::Arc<headless_chrome::Tab>)> {
    let cookies = session_cookies::load_cookies("tm3-session", "changeofharleystreet")
        .context("Failed to load TM3 session cookies")?;

    let headless = std::env::var("TM3_VISIBLE").is_err();
    let browser = Browser::new(
        LaunchOptions::default_builder()
            .headless(headless)
            .window_size(Some((1280, 900)))
            .idle_browser_timeout(Duration::from_secs(120))
            .args(vec![
                std::ffi::OsStr::new("--password-store=basic"),
                std::ffi::OsStr::new("--use-mock-keychain"),
            ])
            .build()?,
    )
    .context("Failed to launch Chrome")?;

    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(30));

    // Navigate to base URL to establish domain context
    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(3));

    // Inject session cookies
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

    // Re-navigate to trigger authenticated load
    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(5));

    let url = tab.get_url();
    if url.contains("login") || url.contains("Login") || url.contains("signin") {
        bail!(
            "TM3 session expired. Re-authenticate first:\n  \
             On Mac: run `tm3-upload login`, then `tm3-cookie-sync`"
        );
    }

    Ok((browser, tab))
}

/// Scrape a TM3 client profile page for metadata.
pub fn scrape_client_profile(tm3_id: &str) -> Result<TM3Profile> {
    let (_browser, tab) = launch_tm3_browser()?;

    let profile_url = format!("{}/contacts/clients/{}", TM3_BASE, tm3_id);
    eprintln!("[onboard] Navigating to client profile: {}", profile_url);
    tab.navigate_to(&profile_url)?;
    std::thread::sleep(Duration::from_secs(5));

    // Extract profile data via JavaScript
    let extraction_js = r#"
        (function() {
            var result = {
                full_name: '',
                dob: null,
                referrer_name: null,
                referrer_practice: null,
                referrer_email: null,
                funding_source: null,
                policy_number: null,
                address: null,
                phone: null,
                email: null
            };

            // Helper: find text content near a label
            function findFieldValue(labelText) {
                var labels = document.querySelectorAll('label, .field-label, dt, th, .label');
                for (var i = 0; i < labels.length; i++) {
                    var text = (labels[i].innerText || '').trim().toLowerCase();
                    if (text.includes(labelText.toLowerCase())) {
                        // Try next sibling, parent's next child, or dd
                        var next = labels[i].nextElementSibling;
                        if (next) {
                            var val = (next.innerText || next.value || '').trim();
                            if (val) return val;
                        }
                        // Try parent container
                        var parent = labels[i].parentElement;
                        if (parent) {
                            var inputs = parent.querySelectorAll('input, select, span.value, .field-value, dd');
                            for (var j = 0; j < inputs.length; j++) {
                                var v = inputs[j].value || inputs[j].innerText || '';
                                if (v.trim()) return v.trim();
                            }
                        }
                    }
                }
                return null;
            }

            // Client name — try page title, header, or name field
            var nameEl = document.querySelector('h1, h2, .client-name, .contact-name, [class*="client-header"]');
            if (nameEl) result.full_name = nameEl.innerText.trim();
            if (!result.full_name) {
                result.full_name = findFieldValue('name') || document.title || '';
            }

            // Date of birth
            result.dob = findFieldValue('date of birth') || findFieldValue('dob') || findFieldValue('birth');

            // Phone
            result.phone = findFieldValue('phone') || findFieldValue('mobile') || findFieldValue('tel');

            // Email
            result.email = findFieldValue('email');

            // Address
            result.address = findFieldValue('address');

            // Referrer/GP — TM3 may have a "Referred by" or "GP" section
            result.referrer_name = findFieldValue('referred by') || findFieldValue('referrer') || findFieldValue('gp');
            result.referrer_practice = findFieldValue('practice') || findFieldValue('surgery');
            result.referrer_email = findFieldValue('referrer email') || findFieldValue('gp email');

            // Funding / insurer
            result.funding_source = findFieldValue('insurer') || findFieldValue('funding') || findFieldValue('funder');
            result.policy_number = findFieldValue('policy') || findFieldValue('membership');

            // Fallback: scan page text for date patterns (DD/MM/YYYY or YYYY-MM-DD)
            if (!result.dob) {
                var pageText = document.body.innerText || '';
                var dateMatch = pageText.match(/\b(\d{1,2})[\/\-](\d{1,2})[\/\-](\d{4})\b/);
                if (dateMatch) {
                    var d = dateMatch[1], m = dateMatch[2], y = dateMatch[3];
                    // Heuristic: if first number > 12, it's DD/MM/YYYY
                    if (parseInt(d) > 12) {
                        result.dob = y + '-' + m.padStart(2,'0') + '-' + d.padStart(2,'0');
                    } else if (parseInt(m) > 12) {
                        result.dob = y + '-' + d.padStart(2,'0') + '-' + m.padStart(2,'0');
                    } else {
                        // Ambiguous — assume DD/MM/YYYY (UK convention)
                        result.dob = y + '-' + m.padStart(2,'0') + '-' + d.padStart(2,'0');
                    }
                }
            }

            return JSON.stringify(result);
        })()
    "#;

    let result = tab
        .evaluate(extraction_js, false)?
        .value
        .ok_or_else(|| anyhow::anyhow!("No result from profile extraction"))?;

    let json_str = result
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Profile extraction returned non-string"))?;

    let raw: serde_json::Value = serde_json::from_str(json_str)?;

    Ok(TM3Profile {
        tm3_id: tm3_id.to_string(),
        full_name: raw["full_name"].as_str().unwrap_or("").to_string(),
        dob: raw["dob"].as_str().map(|s| s.to_string()),
        referrer_name: raw["referrer_name"].as_str().map(|s| s.to_string()),
        referrer_practice: raw["referrer_practice"].as_str().map(|s| s.to_string()),
        referrer_email: raw["referrer_email"].as_str().map(|s| s.to_string()),
        funding_source: raw["funding_source"].as_str().map(|s| s.to_string()),
        policy_number: raw["policy_number"].as_str().map(|s| s.to_string()),
        address: raw["address"].as_str().map(|s| s.to_string()),
        phone: raw["phone"].as_str().map(|s| s.to_string()),
        email: raw["email"].as_str().map(|s| s.to_string()),
    })
}

// ---------------------------------------------------------------------------
// Client ID derivation
// ---------------------------------------------------------------------------

/// Derive a client ID from name and optional DOB.
///
/// Format: first initial + last initial + last 2 digits of birth year.
/// e.g. "Briscoe, Elizabeth" + DOB 1976 → "EB76"
/// Falls back to initials only if DOB unavailable.
pub fn derive_client_id(name: &str, dob: Option<&str>) -> String {
    let (first, last) = parse_name(name);

    let first_initial = first
        .chars()
        .next()
        .unwrap_or('X')
        .to_uppercase()
        .next()
        .unwrap_or('X');
    let last_initial = last
        .chars()
        .next()
        .unwrap_or('X')
        .to_uppercase()
        .next()
        .unwrap_or('X');

    let year_suffix = dob.and_then(|d| {
        // Try YYYY-MM-DD or DD/MM/YYYY
        let re = Regex::new(r"(\d{4})").unwrap();
        re.find(d).map(|m| {
            let year: u32 = m.as_str().parse().unwrap_or(0);
            format!("{:02}", year % 100)
        })
    });

    let base = match year_suffix {
        Some(y) => format!("{}{}{}", first_initial, last_initial, y),
        None => format!("{}{}", first_initial, last_initial),
    };

    // Collision check
    let clients_dir = dirs::home_dir()
        .unwrap_or_default()
        .join("Clinical/clients");

    if !clients_dir.join(&base).exists() {
        return base;
    }

    // Check if the existing client is the same person (matching DOB)
    if let Some(dob_str) = dob {
        let existing_identity = clients_dir.join(&base).join("identity.yaml");
        if let Ok(content) = std::fs::read_to_string(&existing_identity) {
            // Extract DOB from existing identity.yaml
            for line in content.lines() {
                let line = line.trim();
                if let Some((key, val)) = line.split_once(':') {
                    if key.trim().to_lowercase() == "dob" {
                        let existing_dob = val.trim().trim_matches('"').replace('/', "-");
                        let new_dob = dob_str.split('T').next().unwrap_or(dob_str).replace('/', "-");
                        if existing_dob == new_dob {
                            eprintln!("[onboard] {} already exists with same DOB — same person", base);
                            return base;
                        }
                    }
                }
            }
        }
    }

    // Different person with same initials+year — append suffix
    for suffix in 'b'..='z' {
        let candidate = format!("{}{}", base, suffix);
        if !clients_dir.join(&candidate).exists() {
            return candidate;
        }
    }

    base // unlikely: 25 collisions
}

/// Parse "Surname, Firstname" or "Firstname Surname" into (first, last).
fn parse_name(name: &str) -> (String, String) {
    let name = name.trim();

    // "Surname, Firstname" format (TM3 convention)
    if let Some((surname, given)) = name.split_once(',') {
        let given = given.trim();
        let surname = surname.trim();
        // Handle "Firstname (Nickname)" — use the first name
        let first = given.split_whitespace().next().unwrap_or(given);
        // Strip parenthetical nicknames
        let first = first.split('(').next().unwrap_or(first).trim();
        return (first.to_string(), surname.to_string());
    }

    // "Firstname Surname" format
    let parts: Vec<&str> = name.split_whitespace().collect();
    match parts.len() {
        0 => ("X".to_string(), "X".to_string()),
        1 => (parts[0].to_string(), "X".to_string()),
        _ => (
            parts[0].to_string(),
            parts.last().unwrap().to_string(),
        ),
    }
}

// ---------------------------------------------------------------------------
// Identity.yaml population
// ---------------------------------------------------------------------------

/// Write scraped metadata into a client's identity.yaml.
fn populate_identity(client_id: &str, profile: &TM3Profile) -> Result<()> {
    let clients_dir = dirs::home_dir()
        .unwrap_or_default()
        .join("Clinical/clients");
    let identity_path = clients_dir.join(client_id).join("identity.yaml");

    if !identity_path.exists() {
        bail!(
            "identity.yaml not found at {}. Run scaffold first.",
            identity_path.display()
        );
    }

    let (first_name, _) = parse_name(&profile.full_name);

    // Build the YAML content
    let mut yaml = String::new();
    yaml.push_str("---\n");
    yaml.push_str(&format!("name: \"{}\"\n", profile.full_name));
    yaml.push_str(&format!("title: \"{}\"\n", first_name));

    if let Some(ref dob) = profile.dob {
        yaml.push_str(&format!("dob: \"{}\"\n", dob));
    }
    if let Some(ref addr) = profile.address {
        yaml.push_str(&format!("address: \"{}\"\n", addr));
    }
    if let Some(ref phone) = profile.phone {
        yaml.push_str(&format!("phone: \"{}\"\n", phone));
    }
    if let Some(ref email) = profile.email {
        yaml.push_str(&format!("email: \"{}\"\n", email));
    }

    yaml.push_str(&format!("tm3_id: {}\n", profile.tm3_id));
    yaml.push_str("status: active\n");

    // Funding
    yaml.push_str("\nfunding:\n");
    if let Some(ref source) = profile.funding_source {
        yaml.push_str(&format!("  funding_type: \"{}\"\n", source));
    } else {
        yaml.push_str("  funding_type: null\n");
    }
    if let Some(ref policy) = profile.policy_number {
        yaml.push_str(&format!("  policy: \"{}\"\n", policy));
    }
    yaml.push_str("  session_duration: 50\n");

    // Referrer
    yaml.push_str("\nreferrer:\n");
    if let Some(ref name) = profile.referrer_name {
        yaml.push_str(&format!("  name: \"{}\"\n", name));
    } else {
        yaml.push_str("  name: null\n");
    }
    if let Some(ref practice) = profile.referrer_practice {
        yaml.push_str(&format!("  practice: \"{}\"\n", practice));
    }
    if let Some(ref email) = profile.referrer_email {
        yaml.push_str(&format!("  email: \"{}\"\n", email));
    }

    yaml.push_str("---\n");

    std::fs::write(&identity_path, &yaml)
        .with_context(|| format!("Failed to write {}", identity_path.display()))?;

    eprintln!(
        "[onboard] identity.yaml populated for {} ({})",
        client_id, profile.full_name
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Document download and import
// ---------------------------------------------------------------------------

/// Download all TM3 documents and import them into the client directory.
fn download_and_import_docs(client_id: &str, tm3_id: &str) -> Result<usize> {
    // Use the OS temp dir so this works on Mac, Linux and Windows. (Hardcoded
    // /tmp would 404 on Windows.)
    let tmp_path = std::env::temp_dir().join(format!("onboard-{client_id}"));
    let tmp_dir = tmp_path.to_string_lossy().into_owned();
    std::fs::create_dir_all(&tmp_dir).ok();

    // Download all documents via tm3-download
    eprintln!("[onboard] Downloading documents from TM3...");
    let download = Command::new("tm3-download")
        .args(["get-all", tm3_id, "-o", &tmp_dir])
        .output();

    match download {
        Ok(output) if output.status.success() => {
            eprintln!("[onboard] Documents downloaded to {}", tmp_dir);
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("[onboard] Warning: tm3-download failed: {}", stderr.trim());
            return Ok(0);
        }
        Err(e) => {
            eprintln!("[onboard] Warning: tm3-download not available: {}", e);
            return Ok(0);
        }
    }

    // Import each PDF
    let mut imported = 0;
    if let Ok(entries) = std::fs::read_dir(&tmp_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("pdf") {
                let path_str = path.to_string_lossy().to_string();
                eprintln!("[onboard] Importing: {}", path.file_name().unwrap_or_default().to_string_lossy());

                let import = Command::new("clinical")
                    .args(["import-doc", client_id, "--pdf", &path_str])
                    .output();

                match import {
                    Ok(output) if output.status.success() => {
                        imported += 1;
                    }
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        eprintln!("[onboard]   Warning: import failed: {}", stderr.trim());
                    }
                    Err(e) => {
                        eprintln!("[onboard]   Warning: clinical import-doc not available: {}", e);
                    }
                }
            }
        }
    }

    // Clean up temp dir
    std::fs::remove_dir_all(&tmp_dir).ok();

    Ok(imported)
}

// ---------------------------------------------------------------------------
// Orchestrator
// ---------------------------------------------------------------------------

/// Look up a TM3 client by name from the cached client data.
///
/// Reads from `~/.local/share/practiceforge/tm3-clients.json`.
///
/// Two refresh triggers:
/// 1. Time-based: cache older than 2 hours → refresh before lookup.
/// 2. Miss-based: name not in cache → force a refresh and retry once.
///    Catches the race where a brand-new client was booked in TM3 after
///    our last cache snapshot, e.g. 10 minutes before their first
///    appointment lands in the diary. Without this, the capture's
///    auto-onboard path produces a `???` ID for new bookings until the
///    next time-triggered refresh, which could be up to 2 hours away.
///
/// Returns (tm3_id, dob) if found.
fn lookup_tm3_client(name: &str) -> Option<(String, Option<String>)> {
    eprintln!("[onboard] Looking up \"{}\" in TM3 client cache...", name);

    // First pass: time-based refresh if cache is old or missing.
    if !crate::tm3_clients::is_cache_fresh(Duration::from_secs(2 * 3600)) {
        eprintln!("[onboard] Cache stale or missing — refreshing...");
        match crate::tm3_clients::refresh_cache() {
            Ok(n) => eprintln!("[onboard] Cache refreshed: {} clients", n),
            Err(e) => eprintln!("[onboard] Cache refresh failed: {}", e),
        }
    }

    let clients = crate::tm3_clients::load_cache()?;
    if let Some(client) = crate::tm3_clients::find_by_name(&clients, name) {
        return Some(extract_id_and_dob(client));
    }

    // Second pass: name wasn't in cache. Could be a new booking made
    // since the last snapshot, OR a spelling mismatch. Force a fresh
    // pull and try once more — if still not found, it's the latter.
    eprintln!(
        "[onboard] \"{}\" not in cache — forcing refresh (may be a new booking)...",
        name
    );
    match crate::tm3_clients::refresh_cache() {
        Ok(n) => eprintln!("[onboard] Cache force-refreshed: {} clients", n),
        Err(e) => {
            eprintln!("[onboard] Force-refresh failed: {}", e);
            return None;
        }
    }

    let clients = crate::tm3_clients::load_cache()?;
    let client = crate::tm3_clients::find_by_name(&clients, name)?;
    Some(extract_id_and_dob(client))
}

fn extract_id_and_dob(client: &crate::tm3_clients::TM3Client) -> (String, Option<String>) {
    let tm3_id = client.id.to_string();
    let dob = client
        .date_of_birth
        .as_deref()
        .map(crate::tm3_clients::clean_dob);

    eprintln!(
        "[onboard] Found: {} {} (TM3 ID: {}, DOB: {})",
        client.forename,
        client.surname,
        tm3_id,
        dob.as_deref().unwrap_or("unknown")
    );

    (tm3_id, dob)
}

/* BLOCK COMMENT START — dead code from diary click approach
    // Kept for reference — diary click approach that didn't work in headless Chrome
    let diary_url = format!("{}/diary/practitioner", "TM3_BASE");
    let _ = tab.navigate_to(&diary_url);
    eprintln!("[onboard] Waiting for diary...");
    // Poll for appointment elements
    for _ in 0..15 {
        let check = tab.evaluate(
            "document.querySelectorAll('div[title]').length",
            false,
        );
        if let Ok(r) = check {
            if r.value.as_ref().and_then(|v| v.as_f64()).unwrap_or(0.0) > 0.0 {
                break;
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    std::thread::sleep(Duration::from_secs(2));

    // Find the appointment element and click it via CDP mouse events
    // (JS .click() doesn't trigger React's event handlers in headless mode)
    let find_js = format!(
        r#"(function() {{
            var surname = '{}';
            var titles = document.querySelectorAll('div[title]');
            for (var i = 0; i < titles.length; i++) {{
                var t = titles[i].getAttribute('title') || '';
                if (t.includes(surname) && /^\d{{2}}:\d{{2}}-\d{{2}}:\d{{2}}/.test(t)) {{
                    var rect = titles[i].getBoundingClientRect();
                    return JSON.stringify({{
                        found: true,
                        x: rect.x + rect.width / 2,
                        y: rect.y + rect.height / 2
                    }});
                }}
            }}
            return JSON.stringify({{found: false, count: titles.length}});
        }})()"#,
        surname.replace('\'', "\\'").replace('"', "\\\"")
    );

    let coords = match tab.evaluate(&find_js, false) {
        Ok(r) => {
            let val = r.value.as_ref().and_then(|v| v.as_str()).unwrap_or("{}");
            if !val.contains("true") {
                eprintln!("[onboard] Could not find appointment for \"{}\" ({})", name, val);
                return None;
            }
            serde_json::from_str::<serde_json::Value>(val).ok()
        }
        Err(e) => {
            eprintln!("[onboard] Find failed: {}", e);
            return None;
        }
    };

    if let Some(ref coords) = coords {
        let x = coords["x"].as_f64().unwrap_or(0.0);
        let y = coords["y"].as_f64().unwrap_or(0.0);
        eprintln!("[onboard] Clicking at ({:.0}, {:.0})...", x, y);
        use headless_chrome::browser::tab::point::Point;
        let _ = tab.click_point(Point { x, y });
        eprintln!("[onboard] CDP click sent");
    } else {
        return None;
    }

    // Wait for the appointment popup/drawer to render.
    // TM3 opens a side panel with client details — poll for "Ref:" text
    for wait in 0..10 {
        std::thread::sleep(Duration::from_secs(1));
        let check = tab.evaluate(
            "document.body.innerText.includes('Ref:')",
            false,
        );
        if let Ok(r) = check {
            if r.value.as_ref().and_then(|v| v.as_bool()) == Some(true) {
                eprintln!("[onboard] Popup rendered ({}s)", wait + 1);
                break;
            }
        }
        if wait == 4 {
            // Try another CDP click
            eprintln!("[onboard] Retrying click...");
            if let Some(ref coords) = coords {
                let x = coords["x"].as_f64().unwrap_or(0.0);
                let y = coords["y"].as_f64().unwrap_or(0.0);
                use headless_chrome::browser::tab::point::Point;
                let _ = tab.click_point(Point { x, y });
            }
        }
    }

    // Extract Ref ID and DOB from the popup.
    // The popup shows: "[Ref: 5372]" and "22/12/1976 (Age: 49)"
    // Search the ENTIRE rendered DOM — React may use portals.
    let extract_js = r#"
        (function() {
            // Get ALL text from the page including portals
            var html = document.documentElement.outerHTML || '';
            var refMatch = html.match(/Ref:\s*(\d+)/);
            var dobMatch = html.match(/(\d{1,2}\/\d{1,2}\/\d{4})\s*\(Age/);

            // Also check for the popup's API data in intercepted responses
            var apiRef = null;
            var apiDob = null;
            if (window.__tm3_api_responses) {
                for (var i = window.__tm3_api_responses.length - 1; i >= 0; i--) {
                    var r = window.__tm3_api_responses[i];
                    if (r.body && r.body.includes('dateOfBirth')) {
                        var data = JSON.parse(r.body);
                        if (data.id) { apiRef = String(data.id); }
                        if (data.dateOfBirth) { apiDob = data.dateOfBirth; }
                        break;
                    }
                }
            }

            // Debug: list all elements containing "Ref:"
            var refEls = [];
            document.querySelectorAll('*').forEach(function(el) {
                if (el.children.length === 0 && (el.textContent || '').includes('Ref:')) {
                    refEls.push(el.textContent.trim().substring(0, 50));
                }
            });

            return JSON.stringify({
                ref_id: refMatch ? refMatch[1] : apiRef,
                dob: dobMatch ? dobMatch[1] : apiDob,
                ref_elements: refEls,
                html_length: html.length
            });
        })()
    "#;

    match tab.evaluate(extract_js, false) {
        Ok(r) => {
            let val = r.value.as_ref().and_then(|v| v.as_str()).unwrap_or("{}");
            eprintln!("[onboard] Popup data: {}", val);
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(val) {
                if let Some(ref_id) = parsed["ref_id"].as_str() {
                    if !ref_id.is_empty() {
                        eprintln!("[onboard] Found TM3 ID: {}", ref_id);
                        return Some(ref_id.to_string());
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("[onboard] Popup extraction failed: {}", e);
        }
    }

    eprintln!("[onboard] No TM3 ID found for \"{}\"", name);
    None
}

/// Recursively search a JSON value for a client object matching the surname.
/// Returns the TM3 ID if found.
fn find_client_in_json(value: &serde_json::Value, surname_lower: &str) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            // Check if this object looks like a client record
            let has_name = map.keys().any(|k| {
                let kl = k.to_lowercase();
                kl.contains("name") || kl.contains("surname") || kl == "lastname" || kl == "last_name"
            });
            if has_name {
                // Check all string values for surname match
                let name_match = map.values().any(|v| {
                    v.as_str().map(|s| s.to_lowercase().contains(surname_lower)).unwrap_or(false)
                });
                if name_match {
                    // Look for an ID field
                    for (k, v) in map {
                        let kl = k.to_lowercase();
                        if kl == "id" || kl == "clientid" || kl == "client_id" || kl == "contactid" || kl == "contact_id" {
                            if let Some(id) = v.as_u64() {
                                return Some(id.to_string());
                            }
                            if let Some(id) = v.as_str() {
                                if !id.is_empty() {
                                    return Some(id.to_string());
                                }
                            }
                        }
                    }
                }
            }
            // Recurse into values
            for v in map.values() {
                if let Some(id) = find_client_in_json(v, surname_lower) {
                    return Some(id);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Some(id) = find_client_in_json(item, surname_lower) {
                    return Some(id);
                }
            }
        }
        _ => {}
    }
    None
}

BLOCK COMMENT END */

/// Run the full onboarding pipeline for a new TM3 client.
pub fn onboard(tm3_name: &str, tm3_id: Option<&str>) -> Result<OnboardResult> {
    eprintln!("[onboard] Starting onboarding for: {}", tm3_name);

    // Step 1: Resolve TM3 ID + DOB from cache (or argument)
    let (tm3_id, api_dob) = match tm3_id {
        Some(id) => (Some(id.to_string()), None),
        None => {
            match lookup_tm3_client(tm3_name) {
                Some((id, dob)) => (Some(id), dob),
                None => {
                    eprintln!("[onboard] Not found in TM3 — proceeding with name only.");
                    (None, None)
                }
            }
        }
    };

    // Step 2: Build profile from cached TM3 client data
    let tm3_client = crate::tm3_clients::load_cache()
        .and_then(|clients| crate::tm3_clients::find_by_name(&clients, tm3_name).cloned());

    let profile = if let Some(ref c) = tm3_client {
        eprintln!("[onboard] Full profile from cache: {} {} (DOB: {})",
            c.forename, c.surname,
            c.date_of_birth.as_deref().unwrap_or("unknown"));
        TM3Profile {
            tm3_id: tm3_id.as_deref().unwrap_or("").to_string(),
            full_name: tm3_name.to_string(),
            dob: api_dob,
            referrer_name: None,
            referrer_practice: None,
            referrer_email: None,
            funding_source: c.patient_group.clone(),
            policy_number: None,
            address: c.address.clone(),
            phone: c.number.clone(),
            email: c.email.clone(),
        }
    } else {
        TM3Profile {
            tm3_id: tm3_id.as_deref().unwrap_or("").to_string(),
            full_name: tm3_name.to_string(),
            dob: api_dob,
            referrer_name: None, referrer_practice: None,
            referrer_email: None, funding_source: None, policy_number: None,
            address: None, phone: None, email: None,
        }
    };

    // Step 3: Derive client ID
    let client_id = derive_client_id(&profile.full_name, profile.dob.as_deref());
    eprintln!("[onboard] Derived client ID: {}", client_id);

    // Step 4: Check if already onboarded
    let clients_dir = dirs::home_dir()
        .unwrap_or_default()
        .join("Clinical/clients");
    if clients_dir.join(&client_id).exists() {
        eprintln!("[onboard] {} already exists — adding name mapping only.", client_id);
        // Still add the TM3 name format to the client map
        let _ = Command::new("tm3-client-add")
            .args([tm3_name, &client_id])
            .output();
        return Ok(OnboardResult {
            client_id: client_id.clone(),
            tm3_id: tm3_id.unwrap_or_default(),
            name: profile.full_name,
            docs_imported: 0,
            skipped: true,
        });
    }

    // Step 5: Scaffold
    eprintln!("[onboard] Scaffolding {}...", client_id);
    let scaffold = Command::new("clinical")
        .args(["scaffold", &client_id])
        .output()
        .context("Failed to run clinical scaffold")?;
    if !scaffold.status.success() {
        let stderr = String::from_utf8_lossy(&scaffold.stderr);
        bail!("Scaffold failed: {}", stderr.trim());
    }

    // Step 6: Populate identity.yaml
    populate_identity(&client_id, &profile)?;

    // Step 7: Add to tm3-client-map
    eprintln!("[onboard] Adding to tm3-client-map...");
    let map_add = Command::new("tm3-client-add")
        .args([tm3_name, &client_id])
        .output();
    match map_add {
        Ok(output) if output.status.success() => {
            eprintln!("[onboard] Added to tm3-client-map.toml");
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("[onboard] Warning: tm3-client-add failed: {}", stderr.trim());
        }
        Err(e) => {
            eprintln!("[onboard] Warning: tm3-client-add not available: {}", e);
        }
    }

    // Step 8: Download and import documents (only if we have a TM3 ID)
    let docs_imported = if let Some(ref id) = tm3_id {
        download_and_import_docs(&client_id, id)?
    } else {
        0
    };
    eprintln!("[onboard] {} document(s) imported.", docs_imported);

    // Step 9: Notify via DayPage
    let msg = format!(
        "dev:: Auto-onboarded {} as {} ({} doc{} imported)",
        profile.full_name,
        client_id,
        docs_imported,
        if docs_imported == 1 { "" } else { "s" }
    );
    let _ = Command::new("daypage-append").arg(&msg).output();

    eprintln!("[onboard] ✓ Onboarding complete: {} → {}", profile.full_name, client_id);

    Ok(OnboardResult {
        client_id,
        tm3_id: tm3_id.unwrap_or_default(),
        name: profile.full_name,
        docs_imported,
        skipped: false,
    })
}

/// Onboard with practitioner-supplied identifying info (name + DOB), skipping
/// the TM3 cache lookup. Used from the dashboard when a `???` row didn't
/// resolve via cache-refresh (e.g. TM3 not yet populated, or a spelling
/// mismatch the practitioner wants to bypass manually). Documents are
/// downloaded only if a TM3 numeric ID is supplied.
pub fn onboard_manual(
    name: &str,
    dob: &str,
    tm3_id: Option<&str>,
) -> Result<OnboardResult> {
    eprintln!("[onboard] Manual onboarding: {} (DOB: {})", name, dob);

    let profile = TM3Profile {
        tm3_id: tm3_id.unwrap_or_default().to_string(),
        full_name: name.to_string(),
        dob: Some(dob.to_string()),
        referrer_name: None,
        referrer_practice: None,
        referrer_email: None,
        funding_source: None,
        policy_number: None,
        address: None,
        phone: None,
        email: None,
    };

    let client_id = derive_client_id(&profile.full_name, profile.dob.as_deref());
    eprintln!("[onboard] Derived client ID: {}", client_id);

    let clients_dir = dirs::home_dir()
        .unwrap_or_default()
        .join("Clinical/clients");
    if clients_dir.join(&client_id).exists() {
        eprintln!("[onboard] {} already exists — adding name mapping only.", client_id);
        let _ = Command::new("tm3-client-add")
            .args([name, &client_id])
            .output();
        return Ok(OnboardResult {
            client_id: client_id.clone(),
            tm3_id: tm3_id.unwrap_or_default().to_string(),
            name: profile.full_name,
            docs_imported: 0,
            skipped: true,
        });
    }

    eprintln!("[onboard] Scaffolding {}...", client_id);
    let scaffold = Command::new("clinical")
        .args(["scaffold", &client_id])
        .output()
        .context("Failed to run clinical scaffold")?;
    if !scaffold.status.success() {
        let stderr = String::from_utf8_lossy(&scaffold.stderr);
        bail!("Scaffold failed: {}", stderr.trim());
    }

    populate_identity(&client_id, &profile)?;

    eprintln!("[onboard] Adding to tm3-client-map...");
    let map_add = Command::new("tm3-client-add")
        .args([name, &client_id])
        .output();
    match map_add {
        Ok(output) if output.status.success() => {
            eprintln!("[onboard] Added to tm3-client-map.toml");
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("[onboard] Warning: tm3-client-add failed: {}", stderr.trim());
        }
        Err(e) => {
            eprintln!("[onboard] Warning: tm3-client-add not available: {}", e);
        }
    }

    let docs_imported = if let Some(id) = tm3_id {
        download_and_import_docs(&client_id, id)?
    } else {
        0
    };

    let msg = if docs_imported > 0 {
        format!(
            "dev:: Manually onboarded {} as {} ({} doc{} imported)",
            profile.full_name, client_id, docs_imported,
            if docs_imported == 1 { "" } else { "s" }
        )
    } else {
        format!("dev:: Manually onboarded {} as {}", profile.full_name, client_id)
    };
    let _ = Command::new("daypage-append").arg(&msg).output();

    eprintln!("[onboard] ✓ Manual onboarding complete: {} → {}", profile.full_name, client_id);

    Ok(OnboardResult {
        client_id,
        tm3_id: tm3_id.unwrap_or_default().to_string(),
        name: profile.full_name,
        docs_imported,
        skipped: false,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_name_surname_first() {
        let (first, last) = parse_name("Briscoe, Elizabeth");
        assert_eq!(first, "Elizabeth");
        assert_eq!(last, "Briscoe");
    }

    #[test]
    fn test_parse_name_with_nickname() {
        let (first, last) = parse_name("Briscoe, Elizabeth (Beth)");
        assert_eq!(first, "Elizabeth");
        assert_eq!(last, "Briscoe");
    }

    #[test]
    fn test_parse_name_firstname_surname() {
        let (first, last) = parse_name("Elizabeth Briscoe");
        assert_eq!(first, "Elizabeth");
        assert_eq!(last, "Briscoe");
    }

    #[test]
    fn test_derive_id_with_dob() {
        let id = derive_client_id("Briscoe, Elizabeth", Some("1976-03-15"));
        assert!(id.starts_with("EB76"), "Expected EB76*, got {}", id);
    }

    #[test]
    fn test_derive_id_without_dob() {
        let id = derive_client_id("Da Silva, Marcos", None);
        assert!(id.starts_with("MD"), "Expected MD*, got {}", id);
    }

    #[test]
    fn test_derive_id_uk_date_format() {
        let id = derive_client_id("Smith, Jane", Some("15/03/1990"));
        assert!(id.starts_with("JS90"), "Expected JS90*, got {}", id);
    }
}
