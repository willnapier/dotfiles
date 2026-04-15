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

    // Try appending a letter
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
    let tmp_dir = format!("/tmp/onboard-{}", client_id);
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

/// Look up a TM3 client ID by intercepting the TM3 REST API.
///
/// Installs fetch/XHR interceptors, navigates to the contacts page,
/// types the surname into the search box, and extracts the client ID
/// and DOB from the `CustomerAdvancedSearchRequest` API response.
fn lookup_tm3_id_by_search(name: &str) -> Option<String> {
    let surname = if let Some((s, _)) = name.split_once(',') {
        s.trim()
    } else {
        name.split_whitespace().last().unwrap_or(name)
    };

    eprintln!("[onboard] Looking up TM3 ID for \"{}\" via diary click...", name);

    let (_browser, tab) = match launch_tm3_browser() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[onboard] Failed to launch browser: {}", e);
            return None;
        }
    };

    // Intercept TM3 API responses to extract client data.
    // Navigate to contacts page — the SPA fetches client data via XHR/fetch.
    // We capture the response JSON which contains client IDs, names, and DOBs.
    eprintln!("[onboard] Intercepting TM3 API for client data...");

    // Install API interceptors BEFORE page load using addScriptToEvaluateOnNewDocument
    use headless_chrome::protocol::cdp::Page;
    let _ = tab.call_method(Page::AddScriptToEvaluateOnNewDocument {
        source: r#"
        (function() {
            window.__tm3_api_responses = [];
            var origFetch = window.fetch;
            window.fetch = function() {
                return origFetch.apply(this, arguments).then(function(response) {
                    var url = response.url || '';
                    if (url.includes('contact') || url.includes('client') || url.includes('api')) {
                        response.clone().text().then(function(body) {
                            try {
                                window.__tm3_api_responses.push({url: url, body: body.substring(0, 50000)});
                            } catch(e) {}
                        });
                    }
                    return response;
                });
            };
            // Also patch XMLHttpRequest
            var origOpen = XMLHttpRequest.prototype.open;
            var origSend = XMLHttpRequest.prototype.send;
            XMLHttpRequest.prototype.open = function(method, url) {
                this.__url = url;
                return origOpen.apply(this, arguments);
            };
            XMLHttpRequest.prototype.send = function() {
                var self = this;
                this.addEventListener('load', function() {
                    var url = self.__url || '';
                    if (url.includes('contact') || url.includes('client') || url.includes('api')) {
                        try {
                            window.__tm3_api_responses.push({url: url, body: self.responseText.substring(0, 10000)});
                        } catch(e) {}
                    }
                });
                return origSend.apply(this, arguments);
            };
            return 'interceptors installed';
        })()
    "#.to_string(),
        world_name: None,
        include_command_line_api: None,
        run_immediately: None,
    });

    // Force a FULL page reload (not SPA route change) to trigger fresh API calls
    // after interceptors are installed
    let contacts_url = format!("{}/contacts/clients", TM3_BASE);
    let _ = tab.evaluate(
        &format!("window.location.href = '{}'", contacts_url),
        false,
    );
    eprintln!("[onboard] Waiting for contacts page API responses...");
    std::thread::sleep(Duration::from_secs(10));

    // Collect captured API responses
    let collect_js = "JSON.stringify(window.__tm3_api_responses || [])";
    let responses = match tab.evaluate(collect_js, false) {
        Ok(r) => r.value.as_ref().and_then(|v| v.as_str()).unwrap_or("[]").to_string(),
        Err(_) => "[]".to_string(),
    };

    eprintln!("[onboard] Captured {} API response(s)", {
        let v: Vec<serde_json::Value> = serde_json::from_str(&responses).unwrap_or_default();
        v.len()
    });

    // Parse responses looking for client data with our surname
    let api_entries: Vec<serde_json::Value> = serde_json::from_str(&responses).unwrap_or_default();
    let surname_lower = surname.to_lowercase();

    for entry in &api_entries {
        let body = entry["body"].as_str().unwrap_or("");
        let url = entry["url"].as_str().unwrap_or("");
        eprintln!("[onboard] API: {} ({} bytes)", url, body.len());

        // Dump customer/diary responses for debugging
        if url.contains("Customer") || url.contains("Diary") {
            let dump_name = url.split('/').last().unwrap_or("unknown").split('?').next().unwrap_or("unknown");
            let dump_path = format!("/tmp/tm3-api-{}.json", dump_name);
            let _ = std::fs::write(&dump_path, body);
            eprintln!("[onboard]   → dumped to {}", dump_path);
        }

        // Try to parse as JSON and search for our client
        if let Ok(data) = serde_json::from_str::<serde_json::Value>(body) {
            // Search recursively for client-like objects with our surname
            if let Some(id) = find_client_in_json(&data, &surname_lower) {
                eprintln!("[onboard] Found TM3 ID: {} from API response", id);
                return Some(id);
            }
        }
    }

    eprintln!("[onboard] No TM3 ID found for \"{}\" in API responses", name);
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

/// Run the full onboarding pipeline for a new TM3 client.
pub fn onboard(tm3_name: &str, tm3_id: Option<&str>) -> Result<OnboardResult> {
    eprintln!("[onboard] Starting onboarding for: {}", tm3_name);

    // Step 1: Resolve TM3 ID (from argument, or scrape from diary)
    let tm3_id = match tm3_id {
        Some(id) => Some(id.to_string()),
        None => {
            let found = lookup_tm3_id_by_search(tm3_name);
            if found.is_none() {
                eprintln!("[onboard] No TM3 ID found — proceeding with name only.");
            }
            found
        }
    };

    // Step 2: Scrape TM3 profile (if we have an ID)
    let profile = if let Some(ref id) = tm3_id {
        eprintln!("[onboard] Scraping TM3 profile (ID: {})...", id);
        match scrape_client_profile(id) {
            Ok(p) => {
                eprintln!("[onboard] Profile: {} (DOB: {})",
                    p.full_name,
                    p.dob.as_deref().unwrap_or("unknown")
                );
                p
            }
            Err(e) => {
                eprintln!("[onboard] Warning: profile scrape failed: {}", e);
                TM3Profile {
                    tm3_id: id.clone(),
                    full_name: tm3_name.to_string(),
                    dob: None, referrer_name: None, referrer_practice: None,
                    referrer_email: None, funding_source: None, policy_number: None,
                    address: None, phone: None, email: None,
                }
            }
        }
    } else {
        TM3Profile {
            tm3_id: String::new(),
            full_name: tm3_name.to_string(),
            dob: None, referrer_name: None, referrer_practice: None,
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
        eprintln!("[onboard] {} already exists — skipping.", client_id);
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
