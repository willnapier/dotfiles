//! TM3 document download — lists and downloads documents from a patient's record.
//!
//! Authentication: passkey session cookies stored in OS keychain (macOS) or
//! secret-tool (Linux). Same cookie format as tm3-upload.
//!
//! Commands:
//!   tm3-download list <tm3_id>                          — list documents
//!   tm3-download get <tm3_id> <doc_index>               — download one document
//!   tm3-download get-all <tm3_id>                       — download all documents

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use headless_chrome::browser::tab::Tab;
use headless_chrome::protocol::cdp::Network;
use headless_chrome::{Browser, LaunchOptions};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const TM3_BASE: &str = "https://changeofharleystreet.tm3app.com";
const KEYCHAIN_SERVICE: &str = "tm3-session";
const KEYCHAIN_ACCOUNT: &str = "changeofharleystreet";

// ── CLI ─────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "tm3-download", about = "Download documents from TM3 patient records")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List documents for a client
    List {
        /// TM3 client ID
        tm3_id: String,
        /// Output as JSON (for programmatic consumption by `clinical import-doc`)
        #[arg(long)]
        json: bool,
    },
    /// Download a single document by index (from `list` output)
    Get {
        /// TM3 client ID
        tm3_id: String,
        /// Document index (0-based, from `list` output)
        doc_index: usize,
        /// Output file path (default: current directory, original filename)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Download all documents for a client
    GetAll {
        /// TM3 client ID
        tm3_id: String,
        /// Output directory (default: current directory)
        #[arg(short, long)]
        output_dir: Option<PathBuf>,
    },
}

// ── Cookie types (same as tm3_upload) ───────────────────────────────────────

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

// ── Document metadata extracted from DOM ────────────────────────────────────

#[derive(Serialize, Deserialize, Debug)]
struct DocEntry {
    index: usize,
    name: String,
    date: String,
    #[serde(default)]
    doc_type: String,
    #[serde(default)]
    download_url: String,
    #[serde(default)]
    link_href: String,
}

// ── Main ────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Cmd::List { tm3_id, json } => cmd_list(&tm3_id, json),
        Cmd::Get {
            tm3_id,
            doc_index,
            output,
        } => cmd_get(&tm3_id, doc_index, output.as_deref()),
        Cmd::GetAll { tm3_id, output_dir } => cmd_get_all(&tm3_id, output_dir.as_deref()),
    }
}

// ── List documents ──────────────────────────────────────────────────────────

fn cmd_list(tm3_id: &str, json: bool) -> Result<()> {
    let cookies = load_cookies_from_keychain()?;
    eprintln!("[list] Loaded session from keychain.");

    let browser = launch_browser(true)?;
    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(30));

    navigate_to_documents(&tab, &cookies, tm3_id)?;

    let docs = extract_document_list(&tab)?;

    if json {
        // Machine-readable output for `clinical import-doc`
        println!("{}", serde_json::to_string(&docs)?);
        return Ok(());
    }

    if docs.is_empty() {
        println!("No documents found for client {}.", tm3_id);
    } else {
        println!(
            "{:<5} {:<40} {:<12} {}",
            "INDEX", "NAME", "DATE", "TYPE"
        );
        println!("{}", "-".repeat(75));
        for doc in &docs {
            println!(
                "{:<5} {:<40} {:<12} {}",
                doc.index,
                truncate(&doc.name, 40),
                doc.date,
                doc.doc_type
            );
        }
        println!("\n{} document(s) found.", docs.len());
    }

    Ok(())
}

// ── Download single document ────────────────────────────────────────────────

fn cmd_get(tm3_id: &str, doc_index: usize, output: Option<&Path>) -> Result<()> {
    let cookies = load_cookies_from_keychain()?;
    eprintln!("[get] Loaded session from keychain.");

    let browser = launch_browser(true)?;
    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(30));

    navigate_to_documents(&tab, &cookies, tm3_id)?;

    let docs = extract_document_list(&tab)?;
    let doc = docs
        .iter()
        .find(|d| d.index == doc_index)
        .context(format!(
            "Document index {} not found. Use `list` to see available documents (0..{}).",
            doc_index,
            docs.len().saturating_sub(1)
        ))?;

    eprintln!("[get] Downloading: {} ({})", doc.name, doc.date);

    let dest = match output {
        Some(p) => p.to_path_buf(),
        None => PathBuf::from(&sanitise_filename(&doc.name)),
    };

    download_document(&tab, &cookies, doc, &dest)?;
    println!("Saved: {}", dest.display());

    Ok(())
}

// ── Download all documents ──────────────────────────────────────────────────

fn cmd_get_all(tm3_id: &str, output_dir: Option<&Path>) -> Result<()> {
    let cookies = load_cookies_from_keychain()?;
    eprintln!("[get-all] Loaded session from keychain.");

    let out_dir = output_dir.unwrap_or_else(|| Path::new("."));
    if !out_dir.exists() {
        std::fs::create_dir_all(out_dir)?;
    }

    let browser = launch_browser(true)?;
    let tab = browser.new_tab()?;
    tab.set_default_timeout(Duration::from_secs(30));

    navigate_to_documents(&tab, &cookies, tm3_id)?;

    let docs = extract_document_list(&tab)?;
    if docs.is_empty() {
        println!("No documents found for client {}.", tm3_id);
        return Ok(());
    }

    eprintln!("[get-all] Found {} document(s). Downloading...", docs.len());

    let mut ok_count = 0;
    let mut err_count = 0;

    for doc in &docs {
        let filename = sanitise_filename(&doc.name);
        let dest = out_dir.join(&filename);

        // Avoid overwriting — append index if file exists
        let dest = if dest.exists() {
            let stem = dest
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let ext = dest
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            out_dir.join(format!("{}_{}{}", stem, doc.index, ext))
        } else {
            dest
        };

        eprint!("  [{}] {} ... ", doc.index, doc.name);
        match download_document(&tab, &cookies, doc, &dest) {
            Ok(()) => {
                eprintln!("OK ({})", dest.display());
                ok_count += 1;
            }
            Err(e) => {
                eprintln!("FAILED: {}", e);
                err_count += 1;
            }
        }
    }

    println!(
        "\nDone. {} downloaded, {} failed.",
        ok_count, err_count
    );

    Ok(())
}

// ── Navigation (same pattern as tm3_upload) ─────────────────────────────────

fn navigate_to_documents(tab: &Tab, cookies: &[StoredCookie], tm3_id: &str) -> Result<()> {
    // Inject cookies and boot SPA from base URL
    inject_cookies(tab, cookies)?;
    tab.navigate_to(TM3_BASE)?;
    std::thread::sleep(Duration::from_secs(5));

    let url = tab.get_url();
    if url.contains("login") || url.contains("Login") {
        bail!("Session expired. Run 'tm3-upload login' to re-authenticate.");
    }
    eprintln!("[nav] Authenticated. Diary loaded.");

    // Navigate to client documents page
    let doc_url = format!("{}/contacts/clients/{}/documents", TM3_BASE, tm3_id);
    eprintln!("[nav] Navigating to client {} documents...", tm3_id);

    tab.navigate_to(&doc_url)?;
    std::thread::sleep(Duration::from_secs(5));

    // Wait for documents page to render
    if !wait_for_documents_page(tab)? {
        // Fallback: SPA search navigation
        eprintln!("[nav] Direct navigation failed. Trying SPA search...");
        tab.navigate_to(TM3_BASE)?;
        std::thread::sleep(Duration::from_secs(3));
        navigate_via_search(tab, tm3_id)?;
    }

    Ok(())
}

fn wait_for_documents_page(tab: &Tab) -> Result<bool> {
    for _ in 0..10 {
        std::thread::sleep(Duration::from_secs(2));

        let ready = tab
            .evaluate(
                r#"
                (function() {
                    // Look for indicators that the documents page has loaded
                    var body = document.body ? document.body.innerText : "";
                    // Check for document-related UI elements
                    var hasDocContent = body.includes('Attach File')
                        || body.includes('No documents')
                        || body.includes('Document')
                        || document.querySelector('.drop-zone') !== null
                        || document.querySelectorAll('table tr, [class*="document"], [class*="file-list"]').length > 0;
                    return hasDocContent;
                })()
                "#,
                false,
            )
            .ok()
            .and_then(|r| r.value)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if ready {
            eprintln!("[nav] Documents page loaded.");
            return Ok(true);
        }
    }
    Ok(false)
}

fn navigate_via_search(tab: &Tab, tm3_id: &str) -> Result<()> {
    // Open Quick Search (same pattern as tm3_upload)
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
                var results = document.querySelectorAll(
                    '[role="option"], li[class*="result"], a[href*="contacts/clients"]'
                );
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

    bail!("Could not navigate to documents page. Try TM3_VISIBLE=1 to debug.")
}

// ── DOM extraction: document list ───────────────────────────────────────────

fn extract_document_list(tab: &Tab) -> Result<Vec<DocEntry>> {
    let result = tab.evaluate(
        r#"
        (function() {
            var docs = [];

            // Strategy 1: Table rows — TM3 often renders documents as a table
            var rows = document.querySelectorAll('table tbody tr');
            if (rows.length > 0) {
                for (var i = 0; i < rows.length; i++) {
                    var cells = rows[i].querySelectorAll('td');
                    if (cells.length < 2) continue;

                    var link = rows[i].querySelector('a[href], a[download]');
                    var name = '';
                    var href = '';

                    if (link) {
                        name = link.textContent.trim();
                        href = link.href || link.getAttribute('download') || '';
                    } else {
                        name = cells[0].textContent.trim();
                    }

                    if (!name || name.length < 2) continue;

                    var date = '';
                    var docType = '';
                    for (var j = 1; j < cells.length; j++) {
                        var cellText = cells[j].textContent.trim();
                        // Date detection: DD/MM/YYYY or YYYY-MM-DD or similar
                        if (!date && /\d{1,4}[\/-]\d{1,2}[\/-]\d{2,4}/.test(cellText)) {
                            date = cellText;
                        } else if (!docType && cellText.length > 0 && cellText.length < 40) {
                            docType = cellText;
                        }
                    }

                    docs.push({
                        index: i,
                        name: name,
                        date: date,
                        doc_type: docType,
                        download_url: '',
                        link_href: href
                    });
                }
            }

            // Strategy 2: List/card layout — fallback if no table
            if (docs.length === 0) {
                var items = document.querySelectorAll(
                    '[class*="document"], [class*="file-item"], [class*="file-list"] > *'
                );
                for (var k = 0; k < items.length; k++) {
                    var el = items[k];
                    var itemText = el.textContent.trim();
                    if (itemText.length < 3) continue;

                    var itemLink = el.querySelector('a[href], a[download]');
                    var itemName = itemLink
                        ? itemLink.textContent.trim()
                        : itemText.split('\n')[0].trim();
                    var itemHref = itemLink ? (itemLink.href || '') : '';

                    var dateMatch = itemText.match(/\d{1,4}[\/-]\d{1,2}[\/-]\d{2,4}/);

                    docs.push({
                        index: k,
                        name: itemName.substring(0, 120),
                        date: dateMatch ? dateMatch[0] : '',
                        doc_type: '',
                        download_url: '',
                        link_href: itemHref
                    });
                }
            }

            // Strategy 3: Any anchors with download-like attributes or paths
            if (docs.length === 0) {
                var allLinks = document.querySelectorAll(
                    'a[download], a[href*="download"], a[href*="document"], a[href*="file"]'
                );
                for (var m = 0; m < allLinks.length; m++) {
                    var a = allLinks[m];
                    var linkText = a.textContent.trim();
                    if (linkText.length < 2 || linkText.length > 200) continue;
                    docs.push({
                        index: m,
                        name: linkText.substring(0, 120),
                        date: '',
                        doc_type: '',
                        download_url: a.href || '',
                        link_href: a.href || ''
                    });
                }
            }

            return JSON.stringify(docs);
        })()
        "#,
        false,
    )?;

    let json_str = result
        .value
        .as_ref()
        .and_then(|v| v.as_str())
        .unwrap_or("[]");

    let docs: Vec<DocEntry> = serde_json::from_str(json_str)
        .context("Failed to parse document list from DOM")?;

    Ok(docs)
}

// ── Download a single document ──────────────────────────────────────────────

fn download_document(
    tab: &Tab,
    cookies: &[StoredCookie],
    doc: &DocEntry,
    dest: &Path,
) -> Result<()> {
    // Determine the download URL
    let url = if !doc.download_url.is_empty() {
        doc.download_url.clone()
    } else if !doc.link_href.is_empty() {
        doc.link_href.clone()
    } else {
        // Extract URL by clicking the document row and capturing the resulting link
        let url = extract_download_url_by_index(tab, doc.index)?;
        if url.is_empty() {
            bail!(
                "No download URL found for document '{}'. The DOM may need different selectors.",
                doc.name
            );
        }
        url
    };

    eprintln!("[download] URL: {}", url);

    // Build cookie header for reqwest
    let cookie_header = cookies
        .iter()
        .map(|c| format!("{}={}", c.name, c.value))
        .collect::<Vec<_>>()
        .join("; ");

    // Download via async reqwest inside a tokio runtime (blocking feature
    // is not enabled in Cargo.toml, but tokio full is available).
    let rt = tokio::runtime::Runtime::new()?;
    let (status, headers, bytes) = rt.block_on(async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?;

        let resp = client
            .get(&url)
            .header("Cookie", &cookie_header)
            .header("Referer", TM3_BASE)
            .send()
            .await
            .context("HTTP request failed")?;

        let status = resp.status();
        // Extract Content-Disposition before consuming body
        let cd = resp
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        let body = resp.bytes().await?;
        Ok::<_, anyhow::Error>((status, cd, body))
    })?;

    if !status.is_success() {
        bail!("Download failed: HTTP {} for {}", status, url);
    }

    // Try to get filename from Content-Disposition header
    let final_dest = if dest.is_dir()
        || dest.to_string_lossy() == "."
        || dest.to_string_lossy().ends_with('/')
    {
        let filename = headers
            .as_deref()
            .and_then(extract_filename_from_header)
            .unwrap_or_else(|| sanitise_filename(&doc.name));

        let dir = if dest.to_string_lossy() == "." {
            PathBuf::from(".")
        } else {
            dest.to_path_buf()
        };
        dir.join(filename)
    } else {
        dest.to_path_buf()
    };

    // Ensure parent directory exists
    if let Some(parent) = final_dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(&final_dest, &bytes)?;
    eprintln!(
        "[download] Saved {} bytes to {}",
        bytes.len(),
        final_dest.display()
    );

    Ok(())
}

/// Try to extract a download URL by clicking on the document row in the DOM.
fn extract_download_url_by_index(tab: &Tab, index: usize) -> Result<String> {
    let js = format!(
        r#"
        (function() {{
            // Try clicking the row to reveal a download link or trigger navigation
            var rows = document.querySelectorAll('table tbody tr');
            if (rows.length > {idx}) {{
                var row = rows[{idx}];
                // Look for any link in the row
                var link = row.querySelector('a[href]');
                if (link) return link.href;

                // Look for a download button/icon
                var btn = row.querySelector(
                    'button[class*="download" i], [class*="download" i], [title*="download" i]'
                );
                if (btn) {{
                    // Check if it has an onclick with a URL
                    var onclick = btn.getAttribute('onclick') || '';
                    var urlMatch = onclick.match(/https?:\/\/[^\s'"]+/);
                    if (urlMatch) return urlMatch[0];
                }}
            }}

            // Try list/card items
            var items = document.querySelectorAll(
                '[class*="document"], [class*="file-item"], [class*="file-list"] > *'
            );
            if (items.length > {idx}) {{
                var link = items[{idx}].querySelector('a[href]');
                if (link) return link.href;
            }}

            return '';
        }})()
        "#,
        idx = index
    );

    let result = tab.evaluate(&js, false)?;
    let url = result
        .value
        .as_ref()
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(url)
}

// ── Keychain / secret storage ───────────────────────────────────────────────

fn load_cookies_from_keychain() -> Result<Vec<StoredCookie>> {
    let json = keychain_load()?;
    let cookies: Vec<StoredCookie> =
        serde_json::from_str(&json).context("Failed to parse stored cookies")?;
    Ok(cookies)
}

#[cfg(target_os = "macos")]
fn keychain_load() -> Result<String> {
    let output = Command::new("security")
        .args([
            "find-generic-password",
            "-s", KEYCHAIN_SERVICE,
            "-a", KEYCHAIN_ACCOUNT,
            "-w",
        ])
        .output()
        .context("Failed to run security CLI")?;

    if !output.status.success() {
        bail!("No TM3 session in keychain. Run 'tm3-upload login' first.");
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

#[cfg(target_os = "linux")]
fn keychain_load() -> Result<String> {
    // secret-tool uses libsecret / GNOME Keyring / KDE Wallet
    let output = Command::new("secret-tool")
        .args([
            "lookup",
            "service", KEYCHAIN_SERVICE,
            "account", KEYCHAIN_ACCOUNT,
        ])
        .output()
        .context("Failed to run secret-tool. Is libsecret installed?")?;

    if !output.status.success() {
        bail!(
            "No TM3 session in secret storage. \
             Store cookies with: secret-tool store --label 'TM3 session' \
             service {} account {}",
            KEYCHAIN_SERVICE,
            KEYCHAIN_ACCOUNT
        );
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn keychain_load() -> Result<String> {
    bail!("Unsupported platform for keychain access. macOS and Linux are supported.");
}

// ── Browser helpers (same as tm3_upload) ────────────────────────────────────

fn launch_browser(headless: bool) -> Result<Browser> {
    let headless = if std::env::var("TM3_VISIBLE").is_ok() {
        false
    } else {
        headless
    };

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

// ── Utility helpers ─────────────────────────────────────────────────────────

fn sanitise_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();

    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        "document".to_string()
    } else {
        cleaned
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

fn extract_filename_from_header(header: &str) -> Option<String> {
    // Parse Content-Disposition: attachment; filename="report.pdf"
    // or: attachment; filename*=UTF-8''report.pdf
    if let Some(pos) = header.find("filename=") {
        let rest = &header[pos + 9..];
        let filename = rest
            .trim_start_matches('"')
            .split('"')
            .next()
            .unwrap_or(rest)
            .trim_end_matches(';')
            .trim();
        if !filename.is_empty() {
            return Some(filename.to_string());
        }
    }
    if let Some(pos) = header.find("filename*=") {
        let rest = &header[pos + 10..];
        // UTF-8''encoded_name
        if let Some(name_start) = rest.find("''") {
            let name = &rest[name_start + 2..];
            let name = name.split(';').next().unwrap_or(name).trim();
            if !name.is_empty() {
                // URL-decode
                return Some(
                    percent_decode(name),
                );
            }
        }
    }
    None
}

fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            } else {
                result.push('%');
                result.push_str(&hex);
            }
        } else {
            result.push(c);
        }
    }
    result
}
