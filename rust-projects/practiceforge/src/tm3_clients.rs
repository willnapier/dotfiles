//! TM3 client cache — fetches all client records from the TM3 API
//! via headless Chrome and caches locally.
//!
//! The cache is a JSON file at `~/.local/share/practiceforge/tm3-clients.json`.
//! Refreshed hourly during diary capture. Onboarding reads from cache
//! without launching a browser.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

use crate::session_cookies;

const TM3_BASE: &str = "https://changeofharleystreet.tm3app.com";

/// A client record from the TM3 API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TM3Client {
    pub id: u64,
    pub surname: String,
    pub forename: String,
    pub title: Option<String>,
    pub name: Option<String>,
    #[serde(rename = "dateOfBirth")]
    pub date_of_birth: Option<String>,
    pub email: Option<String>,
    pub number: Option<String>,
    pub address: Option<String>,
    #[serde(rename = "postCode")]
    pub post_code: Option<String>,
    pub gender: Option<String>,
    #[serde(rename = "practitionerName")]
    pub practitioner_name: Option<String>,
    #[serde(rename = "practitionerId")]
    pub practitioner_id: Option<u64>,
    #[serde(rename = "patientGroup")]
    pub patient_group: Option<String>,
    #[serde(rename = "registrationDate")]
    pub registration_date: Option<String>,
}

/// Cached client data with metadata.
#[derive(Serialize, Deserialize)]
struct ClientCache {
    fetched_at: String,
    total: usize,
    clients: Vec<TM3Client>,
}

fn cache_path() -> PathBuf {
    dirs::home_dir()
        .expect("no home dir")
        .join(".local/share/practiceforge/tm3-clients.json")
}

/// Refresh the client cache by fetching all clients from TM3's API
/// via headless Chrome. Returns the number of clients fetched.
pub fn refresh_cache() -> Result<usize> {
    let cookies = session_cookies::load_cookies("tm3-session", "changeofharleystreet")
        .context("Failed to load TM3 session cookies")?;

    eprintln!("[tm3-clients] Launching headless Chrome for API call...");
    let browser = headless_chrome::Browser::new(
        headless_chrome::LaunchOptions::default_builder()
            .headless(true)
            .window_size(Some((1280, 900)))
            .idle_browser_timeout(Duration::from_secs(60))
            .args(vec![
                std::ffi::OsStr::new("--password-store=basic"),
                std::ffi::OsStr::new("--use-mock-keychain"),
            ])
            .build()?,
    )?;

    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(30));

    // Navigate and inject cookies
    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(3));

    for cookie in &cookies {
        let _ = tab.call_method(headless_chrome::protocol::cdp::Network::SetCookie {
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

    // Re-navigate to authenticate
    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(5));

    let url = tab.get_url();
    if url.contains("login") {
        anyhow::bail!("TM3 session expired. Run 'tm3-upload login' to re-authenticate.");
    }

    // Fetch all clients via in-browser API call
    eprintln!("[tm3-clients] Fetching all clients from TM3 API...");
    let api_js = r#"(async function() {
        try {
            var resp = await fetch('/api/json/reply/CustomerAdvancedSearchRequest', {
                method: 'POST',
                headers: {'Content-Type': 'application/json'},
                body: JSON.stringify({Take: 5000, Skip: 0})
            });
            var data = await resp.json();
            return JSON.stringify(data);
        } catch(e) {
            return JSON.stringify({error: e.message});
        }
    })()"#;

    let result = tab.evaluate(api_js, true)
        .context("Failed to execute API call")?;

    let json_str = result.value
        .as_ref()
        .and_then(|v| v.as_str())
        .context("API call returned no data")?;

    let data: serde_json::Value = serde_json::from_str(json_str)
        .context("Failed to parse API response")?;

    if let Some(err) = data.get("error").and_then(|v| v.as_str()) {
        anyhow::bail!("TM3 API error: {}", err);
    }

    let results = data["results"]
        .as_array()
        .context("API response has no results array")?;

    let clients: Vec<TM3Client> = results.iter()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect();

    let total = clients.len();
    eprintln!("[tm3-clients] Fetched {} clients", total);

    // Write cache
    let cache = ClientCache {
        fetched_at: chrono::Local::now().to_rfc3339(),
        total,
        clients,
    };

    let cache_dir = cache_path().parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&cache_dir)?;
    let json = serde_json::to_string(&cache)?;
    std::fs::write(cache_path(), &json)
        .with_context(|| format!("Failed to write cache: {}", cache_path().display()))?;

    eprintln!("[tm3-clients] Cache written to {}", cache_path().display());
    Ok(total)
}

/// Load the client cache from disk. Returns None if cache doesn't exist.
pub fn load_cache() -> Option<Vec<TM3Client>> {
    let data = std::fs::read_to_string(cache_path()).ok()?;
    let cache: ClientCache = serde_json::from_str(&data).ok()?;
    Some(cache.clients)
}

/// Check if the cache is fresh (less than max_age old).
pub fn is_cache_fresh(max_age: Duration) -> bool {
    let path = cache_path();
    if !path.exists() { return false; }
    let metadata = std::fs::metadata(&path).ok();
    metadata.and_then(|m| m.modified().ok())
        .map(|modified| modified.elapsed().unwrap_or(Duration::MAX) < max_age)
        .unwrap_or(false)
}

/// Look up a client by surname (case-insensitive). Returns the first match.
pub fn find_by_surname<'a>(clients: &'a [TM3Client], surname: &str) -> Option<&'a TM3Client> {
    let surname_lower = surname.to_lowercase();
    clients.iter().find(|c| c.surname.to_lowercase() == surname_lower)
}

/// Look up a client by full name "Surname, Forename" or "Surname, Forename (Nickname)".
pub fn find_by_name<'a>(clients: &'a [TM3Client], name: &str) -> Option<&'a TM3Client> {
    let name_lower = name.to_lowercase();

    // Extract surname from "Surname, Forename (Nickname)" format
    let surname = name.split(',').next().unwrap_or(name).trim().to_lowercase();
    let forename_part = name.split(',').nth(1)
        .map(|s| s.trim().split('(').next().unwrap_or(s).trim().to_lowercase())
        .unwrap_or_default();

    // Try exact name match first
    for client in clients {
        if let Some(ref full_name) = client.name {
            if full_name.to_lowercase().contains(&surname) &&
               (forename_part.is_empty() || full_name.to_lowercase().contains(&forename_part)) {
                return Some(client);
            }
        }
    }

    // Fallback: surname + forename match
    clients.iter().find(|c| {
        c.surname.to_lowercase() == surname &&
        (forename_part.is_empty() || c.forename.to_lowercase().contains(&forename_part))
    })
}

/// Extract a clean DOB string (YYYY-MM-DD) from the API format.
pub fn clean_dob(dob: &str) -> String {
    dob.split('T').next().unwrap_or(dob).to_string()
}
