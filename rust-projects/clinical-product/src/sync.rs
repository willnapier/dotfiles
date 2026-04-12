//! TM3 diary sync — compare TM3 appointments against local client directories.
//!
//! Scrapes the TM3 diary via headless Chrome, lists local `~/Clinical/clients/`
//! directories, and reports which TM3 clients have no local directory and which
//! local clients don't appear in today's diary.

use anyhow::{bail, Context, Result};
use headless_chrome::protocol::cdp::Network;
use headless_chrome::{Browser, LaunchOptions};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::session_cookies::{self, StoredCookie};

const TM3_BASE: &str = "https://changeofharleystreet.tm3app.com";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A client found in the TM3 diary.
pub struct TM3Client {
    pub name: String,
    /// TM3 contact/client ID, if extractable from links or data attributes.
    pub tm3_id: Option<String>,
    /// The appointment time range shown in the diary, e.g. "10:00 - 10:50".
    pub next_appointment: Option<String>,
}

/// Result of comparing TM3 diary against local client directories.
pub struct SyncResult {
    /// All clients found in today's TM3 diary.
    pub tm3_clients: Vec<TM3Client>,
    /// All local client directory IDs (from ~/Clinical/clients/).
    pub local_clients: Vec<String>,
    /// TM3 diary clients with no matching local directory.
    pub unmatched_tm3: Vec<TM3Client>,
    /// Local directories not seen in today's TM3 diary (informational).
    pub unmatched_local: Vec<String>,
}

// ---------------------------------------------------------------------------
// Core public API
// ---------------------------------------------------------------------------

/// Scrape the TM3 diary for today's clients via headless Chrome.
pub fn scrape_tm3_diary() -> Result<Vec<TM3Client>> {
    // 1. Load session cookies
    let cookies = session_cookies::load_cookies("tm3-session", "changeofharleystreet")
        .context("Failed to load TM3 session cookies")?;

    // 2. Launch Chrome (headless unless TM3_VISIBLE is set)
    let headless = std::env::var("TM3_VISIBLE").is_err();
    eprintln!(
        "[sync] Launching Chrome ({})...",
        if headless { "headless" } else { "visible" }
    );

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

    // 3. Navigate to base URL (establishes domain context for cookies)
    eprintln!("[sync] Navigating to TM3...");
    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(3));

    // 4. Inject session cookies
    inject_cookies(&tab, &cookies)?;

    // 5. Re-navigate to trigger authenticated load
    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(5));

    let url = tab.get_url();
    eprintln!("[sync] Post-auth URL: {}", url);

    // If we're still on a login page, the session is expired
    if url.contains("login") || url.contains("Login") || url.contains("signin") {
        bail!(
            "TM3 session expired. Re-authenticate first:\n  \
             On Mac: run the browser cookie tool, then `tm3-cookie-sync`\n  \
             Or update ~/.tm3-cookies.json manually."
        );
    }

    // 6. Wait for the diary/scheduler to render (React SPA needs time)
    eprintln!("[sync] Waiting for diary to render...");
    std::thread::sleep(Duration::from_secs(3));

    // 7. Extract appointment data via multiple strategies
    let extraction_js = r#"
        (function() {
            var results = [];
            var seen = {};  // deduplicate by name

            // Strategy 1: Elements with data-uid (Kendo scheduler events)
            var uidEls = document.querySelectorAll('[data-uid]');
            for (var i = 0; i < uidEls.length; i++) {
                var el = uidEls[i];
                var text = (el.innerText || '').trim();
                if (text.length > 2 && text.length < 300) {
                    var timeMatch = text.match(/(\d{1,2}:\d{2}\s*-\s*\d{1,2}:\d{2})/);
                    var nameText = text;
                    if (timeMatch) {
                        // Remove time part to isolate name
                        nameText = text.replace(timeMatch[0], '').trim();
                    }
                    // Clean up: remove leading/trailing punctuation, newlines
                    nameText = nameText.replace(/[\n\r]+/g, ' ').replace(/^\s*[-,]\s*/, '').trim();
                    if (nameText.length > 1 && !seen[nameText]) {
                        seen[nameText] = true;
                        // Look for client links in this element or ancestors
                        var clientId = null;
                        var links = el.querySelectorAll('a[href*="contacts/clients"]');
                        if (links.length === 0 && el.closest) {
                            var parent = el.closest('[data-uid]');
                            if (parent) links = parent.querySelectorAll('a[href*="contacts/clients"]');
                        }
                        if (links.length > 0) {
                            var href = links[0].getAttribute('href') || '';
                            var idMatch = href.match(/contacts\/clients\/(\d+)/);
                            if (idMatch) clientId = idMatch[1];
                        }
                        results.push({
                            name: nameText,
                            tm3_id: clientId,
                            appointment: timeMatch ? timeMatch[1] : null,
                            source: 'data-uid'
                        });
                    }
                }
            }

            // Strategy 2: Elements matching time pattern in div/td/span
            var allEls = document.querySelectorAll('div, td, span, li');
            var timePattern = /(\d{1,2}:\d{2}\s*-\s*\d{1,2}:\d{2})/;
            for (var j = 0; j < allEls.length; j++) {
                var el2 = allEls[j];
                var txt = (el2.innerText || '').trim();
                if (txt.length < 10 || txt.length > 300) continue;
                if (!timePattern.test(txt)) continue;

                var timeMatch2 = txt.match(timePattern);
                var nameText2 = txt.replace(timeMatch2[0], '').replace(/[\n\r]+/g, ' ').trim();
                // Strip common noise: therapy type labels, room names
                nameText2 = nameText2.replace(/^\s*[-,]\s*/, '').trim();

                if (nameText2.length > 1 && !seen[nameText2]) {
                    seen[nameText2] = true;
                    var clientId2 = null;
                    var links2 = el2.querySelectorAll('a[href*="contacts/clients"]');
                    if (links2.length > 0) {
                        var href2 = links2[0].getAttribute('href') || '';
                        var idMatch2 = href2.match(/contacts\/clients\/(\d+)/);
                        if (idMatch2) clientId2 = idMatch2[1];
                    }
                    results.push({
                        name: nameText2,
                        tm3_id: clientId2,
                        appointment: timeMatch2[1],
                        source: 'time-pattern'
                    });
                }
            }

            // Strategy 3: Kendo scheduler event classes
            var eventSelectors = [
                '[class*="k-event"]',
                '[class*="scheduler-event"]',
                '[class*="SchedulerEvent"]',
                '[class*="appointment"]',
                '[class*="Appointment"]',
                '[class*="booking"]'
            ];
            for (var s = 0; s < eventSelectors.length; s++) {
                var eventEls = document.querySelectorAll(eventSelectors[s]);
                for (var k = 0; k < eventEls.length; k++) {
                    var el3 = eventEls[k];
                    var txt3 = (el3.innerText || '').trim();
                    if (txt3.length < 2 || txt3.length > 300) continue;

                    var timeMatch3 = txt3.match(timePattern);
                    var nameText3 = txt3;
                    if (timeMatch3) {
                        nameText3 = txt3.replace(timeMatch3[0], '').replace(/[\n\r]+/g, ' ').trim();
                    }
                    nameText3 = nameText3.replace(/^\s*[-,]\s*/, '').trim();

                    if (nameText3.length > 1 && !seen[nameText3]) {
                        seen[nameText3] = true;
                        var clientId3 = null;
                        var links3 = el3.querySelectorAll('a[href*="contacts/clients"]');
                        if (links3.length > 0) {
                            var href3 = links3[0].getAttribute('href') || '';
                            var idMatch3 = href3.match(/contacts\/clients\/(\d+)/);
                            if (idMatch3) clientId3 = idMatch3[1];
                        }
                        results.push({
                            name: nameText3,
                            tm3_id: clientId3,
                            appointment: timeMatch3 ? timeMatch3[1] : null,
                            source: 'event-class'
                        });
                    }
                }
            }

            // Strategy 4: Any links to client pages
            var clientLinks = document.querySelectorAll('a[href*="contacts/clients"]');
            for (var m = 0; m < clientLinks.length; m++) {
                var link = clientLinks[m];
                var linkText = (link.innerText || '').trim();
                var href4 = link.getAttribute('href') || '';
                var idMatch4 = href4.match(/contacts\/clients\/(\d+)/);

                if (linkText.length > 1 && !seen[linkText]) {
                    seen[linkText] = true;
                    // Try to find a time near this link
                    var parent4 = link.parentElement;
                    var nearText = parent4 ? (parent4.innerText || '') : '';
                    var timeMatch4 = nearText.match(timePattern);

                    results.push({
                        name: linkText,
                        tm3_id: idMatch4 ? idMatch4[1] : null,
                        appointment: timeMatch4 ? timeMatch4[1] : null,
                        source: 'client-link'
                    });
                }
            }

            return JSON.stringify(results);
        })()
    "#;

    let result = tab
        .evaluate(extraction_js, false)
        .context("Failed to evaluate diary extraction JS")?;

    let json_str = match &result.value {
        Some(val) => val.as_str().unwrap_or("[]").to_string(),
        None => "[]".to_string(),
    };

    let raw: Vec<RawExtracted> =
        serde_json::from_str(&json_str).context("Failed to parse diary extraction JSON")?;

    eprintln!("[sync] Extracted {} appointment entries.", raw.len());

    // Deduplicate by name (prefer entries with a tm3_id)
    let mut by_name: HashMap<String, TM3Client> = HashMap::new();
    for entry in raw {
        let key = entry.name.to_lowercase();
        let existing = by_name.get(&key);
        let dominated = match existing {
            None => true,
            Some(prev) => prev.tm3_id.is_none() && entry.tm3_id.is_some(),
        };
        if dominated {
            by_name.insert(
                key,
                TM3Client {
                    name: entry.name,
                    tm3_id: entry.tm3_id,
                    next_appointment: entry.appointment,
                },
            );
        }
    }

    let mut clients: Vec<TM3Client> = by_name.into_values().collect();
    clients.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Ok(clients)
}

/// List local client directory names under ~/Clinical/clients/.
pub fn list_local_clients() -> Result<Vec<String>> {
    let dir = clients_dir();
    let mut ids = Vec::new();

    if !dir.exists() {
        eprintln!(
            "[sync] Warning: {} does not exist.",
            dir.display()
        );
        return Ok(ids);
    }

    let entries =
        std::fs::read_dir(&dir).with_context(|| format!("Cannot read {}", dir.display()))?;

    for entry in entries.flatten() {
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if let Some(name) = entry.file_name().to_str() {
                if !name.starts_with('.') {
                    ids.push(name.to_string());
                }
            }
        }
    }

    ids.sort();
    Ok(ids)
}

/// Compare TM3 diary against local client directories and return a SyncResult.
pub fn sync_check() -> Result<SyncResult> {
    let tm3_clients = scrape_tm3_diary()?;
    let local_clients = list_local_clients()?;

    // Build a lookup of local clients: client_id -> (name, tm3_id) from identity.yaml
    let local_index = build_local_index(&local_clients);

    let mut unmatched_tm3: Vec<TM3Client> = Vec::new();
    let mut matched_local_ids: Vec<String> = Vec::new();

    for tm3 in &tm3_clients {
        let matched = find_local_match(tm3, &local_index);
        if let Some(client_id) = matched {
            matched_local_ids.push(client_id);
        } else {
            unmatched_tm3.push(TM3Client {
                name: tm3.name.clone(),
                tm3_id: tm3.tm3_id.clone(),
                next_appointment: tm3.next_appointment.clone(),
            });
        }
    }

    // Local clients not in today's diary
    let unmatched_local: Vec<String> = local_clients
        .iter()
        .filter(|id| !matched_local_ids.contains(id))
        .cloned()
        .collect();

    Ok(SyncResult {
        tm3_clients: tm3_clients
            .into_iter()
            .map(|c| TM3Client {
                name: c.name,
                tm3_id: c.tm3_id,
                next_appointment: c.next_appointment,
            })
            .collect(),
        local_clients,
        unmatched_tm3,
        unmatched_local,
    })
}

/// Pretty-print the sync result to stdout.
pub fn display_sync_result(result: &SyncResult) {
    println!("=== TM3 Diary Sync ===\n");

    // Summary counts
    println!(
        "TM3 diary:      {} client(s) with appointments today",
        result.tm3_clients.len()
    );
    println!(
        "Local clients:  {} directory(ies) in ~/Clinical/clients/",
        result.local_clients.len()
    );
    println!();

    // Today's diary
    if !result.tm3_clients.is_empty() {
        println!("--- Today's diary ---");
        for client in &result.tm3_clients {
            let time = client
                .next_appointment
                .as_deref()
                .unwrap_or("(no time)");
            let id_tag = match &client.tm3_id {
                Some(id) => format!("  [TM3 #{}]", id),
                None => String::new(),
            };
            println!("  {} - {}{}", time, client.name, id_tag);
        }
        println!();
    }

    // Unmatched TM3 clients (action needed)
    if !result.unmatched_tm3.is_empty() {
        println!(
            "--- Unmatched TM3 clients ({}) --- ACTION NEEDED",
            result.unmatched_tm3.len()
        );
        println!("  These clients have appointments but no local directory:\n");
        for client in &result.unmatched_tm3 {
            let suggested_id = suggest_client_id(&client.name);
            let time = client
                .next_appointment
                .as_deref()
                .unwrap_or("(no time)");
            println!("  {} - {}", time, client.name);
            if let Some(id) = &client.tm3_id {
                println!("    TM3 ID: {}", id);
            }
            println!("    Suggested: clinical scaffold {}", suggested_id);
            println!();
        }
    } else {
        println!("--- All TM3 diary clients matched to local directories ---");
        println!();
    }

    // Unmatched local (informational)
    if !result.unmatched_local.is_empty() {
        println!(
            "--- Local-only clients ({}) --- no appointment today",
            result.unmatched_local.len()
        );
        for id in &result.unmatched_local {
            println!("  {}", id);
        }
        println!();
    }
}

// ---------------------------------------------------------------------------
// Internal types and helpers
// ---------------------------------------------------------------------------

/// Raw extraction result from the JS evaluation.
#[derive(serde::Deserialize)]
struct RawExtracted {
    name: String,
    tm3_id: Option<String>,
    appointment: Option<String>,
    #[allow(dead_code)]
    source: Option<String>,
}

/// Local client index entry parsed from identity.yaml.
struct LocalClientEntry {
    client_id: String,
    /// Full name from identity.yaml (if present).
    name: Option<String>,
    /// TM3 contact ID from identity.yaml (if present).
    tm3_id: Option<String>,
}

/// Path to ~/Clinical/clients/.
fn clients_dir() -> PathBuf {
    dirs::home_dir()
        .expect("cannot resolve home directory")
        .join("Clinical")
        .join("clients")
}

/// Path to a client's identity.yaml.
fn identity_yaml_path(client_id: &str) -> PathBuf {
    clients_dir()
        .join(client_id)
        .join("private")
        .join("identity.yaml")
}

/// Build an index of local clients from their identity.yaml files.
fn build_local_index(client_ids: &[String]) -> Vec<LocalClientEntry> {
    client_ids
        .iter()
        .map(|id| {
            let yaml_path = identity_yaml_path(id);
            let (name, tm3_id) = parse_identity_yaml(&yaml_path);
            LocalClientEntry {
                client_id: id.clone(),
                name,
                tm3_id,
            }
        })
        .collect()
}

/// Light identity.yaml parser -- extracts `name` and `tm3_id` fields.
///
/// Mirrors the approach used in dashboard.rs: line-by-line key: value parsing
/// without pulling in a full YAML crate.
fn parse_identity_yaml(path: &PathBuf) -> (Option<String>, Option<String>) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return (None, None),
    };

    let mut name: Option<String> = None;
    let mut tm3_id: Option<String> = None;

    for line in content.lines() {
        let line = line.trim();
        if let Some((key, val)) = line.split_once(':') {
            let key = key.trim().to_lowercase();
            let val = val.trim().trim_matches('"').trim().to_string();
            if val.is_empty() {
                continue;
            }
            match key.as_str() {
                "name" | "full_name" => name = Some(val),
                "tm3_id" | "tm3id" => tm3_id = Some(val),
                _ => {}
            }
        }
    }

    (name, tm3_id)
}

/// Try to match a TM3 diary client against the local client index.
///
/// Matching priority:
/// 1. Exact TM3 ID match (most reliable)
/// 2. Exact name match (case-insensitive) against identity.yaml name
/// 3. Fuzzy name match against both the directory ID and the identity name
fn find_local_match(tm3: &TM3Client, index: &[LocalClientEntry]) -> Option<String> {
    let tm3_name_lower = tm3.name.to_lowercase();

    // 1. TM3 ID match
    if let Some(ref tm3_id) = tm3.tm3_id {
        for entry in index {
            if let Some(ref local_id) = entry.tm3_id {
                if local_id == tm3_id {
                    return Some(entry.client_id.clone());
                }
            }
        }
    }

    // 2. Exact name match against identity.yaml name
    for entry in index {
        if let Some(ref local_name) = entry.name {
            if local_name.to_lowercase() == tm3_name_lower {
                return Some(entry.client_id.clone());
            }
        }
    }

    // 3. Fuzzy matching: check if the TM3 name components appear in the client_id
    //    Client IDs typically look like "firstname-lastname" or "firstname_lastname"
    let tm3_parts: Vec<&str> = tm3.name.split_whitespace().collect();
    if tm3_parts.len() >= 2 {
        let first = tm3_parts[0].to_lowercase();
        let last = tm3_parts.last().unwrap().to_lowercase();

        for entry in index {
            let id_lower = entry.client_id.to_lowercase();
            // Check if both first and last name appear in the directory name
            if id_lower.contains(&first) && id_lower.contains(&last) {
                return Some(entry.client_id.clone());
            }
            // Also check the stored name for partial matches
            if let Some(ref local_name) = entry.name {
                let local_lower = local_name.to_lowercase();
                if local_lower.contains(&first) && local_lower.contains(&last) {
                    return Some(entry.client_id.clone());
                }
            }
        }
    }

    None
}

/// Suggest a client directory ID from a display name.
///
/// "John Smith" -> "john-smith"
fn suggest_client_id(name: &str) -> String {
    name.to_lowercase()
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join("-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect()
}

/// Inject TM3 session cookies into a Chrome tab.
fn inject_cookies(tab: &headless_chrome::browser::tab::Tab, cookies: &[StoredCookie]) -> Result<()> {
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
