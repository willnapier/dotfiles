//! Import referral documents from TM3 into the local client directory.
//!
//! Flow: TM3 documents page → download PDFs → pdftotext → clean → save as .md
//! The extracted text is picked up by `find_correspondence()` in the note
//! generation prompt, giving the model access to the referral context.
//!
//! Document types handled:
//! - Referral letters (from GP or other referrer)
//! - Patient information forms (self-reported by the client)
//! - Any other clinical documents Olly has uploaded

use anyhow::{bail, Context, Result};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::process::Command;

use clinical_core::client;
use clinical_core::identity;

/// Extract text from a PDF file using `pdftotext`.
pub fn extract_pdf_text(pdf_path: &Path) -> Result<String> {
    let output = Command::new("pdftotext")
        .args(["-layout", pdf_path.to_str().unwrap(), "-"])
        .output()
        .context("Failed to run pdftotext — is poppler installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("pdftotext failed: {}", stderr.trim());
    }

    let text = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(text)
}

/// Clean extracted PDF text: strip headers, footers, page numbers,
/// excessive whitespace, and letterhead noise.
pub fn clean_extracted_text(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    let mut cleaned = Vec::new();

    // Patterns to strip
    let page_num_re = Regex::new(r"^\s*-?\s*\d+\s*-?\s*$").unwrap();
    let page_break_re = Regex::new(r"^\f").unwrap();

    let mut consecutive_blank = 0u32;

    for line in &lines {
        // Skip form feed characters (page breaks)
        let line = page_break_re.replace(line, "");
        let trimmed = line.trim();

        // Skip standalone page numbers
        if page_num_re.is_match(trimmed) {
            continue;
        }

        // Collapse multiple blank lines to max 2
        if trimmed.is_empty() {
            consecutive_blank += 1;
            if consecutive_blank <= 2 {
                cleaned.push(String::new());
            }
            continue;
        }

        consecutive_blank = 0;
        cleaned.push(trimmed.to_string());
    }

    // Trim leading/trailing blank lines
    let result = cleaned.join("\n");
    result.trim().to_string()
}

/// Save extracted document text to the client directory.
///
/// Filename format: `YYYY-MM-DD-<doc_type>.md`
/// where doc_type is "referral", "patient-info", "gp-letter", etc.
pub fn save_document_text(
    client_id: &str,
    doc_type: &str,
    date: &str,
    text: &str,
) -> Result<PathBuf> {
    let client_dir = client::client_dir(client_id);
    if !client_dir.exists() {
        bail!("Client directory not found: {}", client_dir.display());
    }

    let filename = format!("{}-{}.md", date, doc_type);
    let path = client_dir.join(&filename);

    // Don't overwrite existing files
    if path.exists() {
        eprintln!("  Already exists: {} — skipping", path.display());
        return Ok(path);
    }

    // Add a header for context
    let content = format!(
        "# {} (imported from TM3)\n\n{}\n",
        doc_type_label(doc_type),
        text
    );

    std::fs::write(&path, &content)
        .with_context(|| format!("Failed to write: {}", path.display()))?;

    Ok(path)
}

fn doc_type_label(doc_type: &str) -> String {
    match doc_type {
        "referral" => "Referral Letter".to_string(),
        "patient-info" => "Patient Information Form".to_string(),
        "gp-letter" => "GP Letter".to_string(),
        "assessment" => "Assessment Report".to_string(),
        other => other.replace('-', " "),
    }
}

/// Classify a TM3 document name into a doc_type.
/// Returns None if the document should be skipped (e.g., appointment reminders).
pub fn classify_document(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();

    if lower.contains("referral") {
        Some("referral")
    } else if lower.contains("patient info")
        || lower.contains("patient information")
        || lower.contains("registration")
        || lower.contains("intake")
    {
        Some("patient-info")
    } else if lower.contains("phq") || lower.contains("gad") || lower.contains("questionnaire") {
        Some("assessment")
    } else if lower.contains("gp letter") || lower.contains("gp report") {
        Some("gp-letter")
    } else if lower.contains("appointment")
        || lower.contains("reminder")
        || lower.contains("sms")
        || lower.contains("docusign")
        || lower.contains("invoice")
    {
        // Skip administrative documents
        None
    } else {
        // Unknown document — import as generic
        Some("clinical-document")
    }
}

/// Import a local PDF file into a client's directory.
/// This is the "offline" path — for when you have the PDF already downloaded.
pub fn import_local_pdf(
    client_id: &str,
    pdf_path: &str,
    doc_type: Option<&str>,
    date: Option<&str>,
) -> Result<()> {
    let path = Path::new(pdf_path);
    if !path.exists() {
        bail!("PDF not found: {}", pdf_path);
    }

    // Extract text
    eprintln!("Extracting text from {}...", path.display());
    let raw_text = extract_pdf_text(path)?;
    let cleaned = clean_extracted_text(&raw_text);

    if cleaned.is_empty() {
        bail!("No text extracted from PDF (may be a scanned image — OCR not supported).");
    }

    eprintln!("  Extracted {} chars ({} lines)", cleaned.len(), cleaned.lines().count());

    // Determine doc type
    let doc_type = doc_type.unwrap_or_else(|| {
        let filename = path.file_name().unwrap().to_string_lossy();
        classify_document(&filename).unwrap_or("clinical-document")
    });

    // Determine date
    let date = date.map(String::from).unwrap_or_else(|| {
        // Try to extract date from filename
        let filename = path.file_name().unwrap().to_string_lossy();
        let date_re = Regex::new(r"(\d{4}-\d{2}-\d{2})").unwrap();
        date_re
            .captures(&filename)
            .map(|c| c[1].to_string())
            .unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string())
    });

    // Save
    let saved_path = save_document_text(client_id, doc_type, &date, &cleaned)?;
    eprintln!("  Saved: {}", saved_path.display());

    Ok(())
}

/// Import referral documents from TM3 for a client.
/// This is the "online" path — uses headless browser to download from TM3.
pub fn import_from_tm3(client_id: &str, dry_run: bool) -> Result<()> {
    // Load identity for tm3_id
    let id_path = client::identity_path(client_id);
    let ident = identity::load_identity(&id_path)
        .with_context(|| format!("Failed to load identity: {}", id_path.display()))?;

    let tm3_id = match &ident.tm3_id {
        Some(val) => match val {
            serde_yaml::Value::Number(n) => n.to_string(),
            serde_yaml::Value::String(s) if !s.is_empty() => s.clone(),
            _ => bail!("No tm3_id set in identity.yaml for {}", client_id),
        },
        None => bail!("No tm3_id set in identity.yaml for {}", client_id),
    };

    eprintln!("Importing documents for {} (TM3 ID: {})...", client_id, tm3_id);

    // Call the TM3 document scraper binary
    let output = Command::new("tm3-import-docs")
        .args([&tm3_id])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            // Parse the JSON output: list of {name, url, type} objects
            let docs: Vec<serde_json::Value> =
                serde_json::from_str(&stdout).context("Failed to parse tm3-import-docs output")?;

            if docs.is_empty() {
                eprintln!("No documents found in TM3 for this client.");
                return Ok(());
            }

            eprintln!("Found {} documents in TM3:", docs.len());
            for doc in &docs {
                let name = doc["name"].as_str().unwrap_or("unknown");
                let doc_type = classify_document(name);
                let status = if doc_type.is_some() { "→ import" } else { "→ skip" };
                eprintln!("  {} {}", name, status);
            }

            if dry_run {
                eprintln!("\n(dry-run — no documents downloaded)");
                return Ok(());
            }

            // Download and import each relevant document
            for doc in &docs {
                let name = doc["name"].as_str().unwrap_or("unknown");
                let url = doc["url"].as_str().unwrap_or("");
                let doc_type = match classify_document(name) {
                    Some(t) => t,
                    None => continue, // skip administrative docs
                };

                eprintln!("  Downloading: {}...", name);

                // Download PDF to temp file
                let tmp_path = format!("/tmp/tm3-import-{}.pdf", uuid_short());
                let dl_status = Command::new("curl")
                    .args(["-s", "-o", &tmp_path, url])
                    .status();

                match dl_status {
                    Ok(s) if s.success() => {
                        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                        if let Err(e) = import_local_pdf(client_id, &tmp_path, Some(doc_type), Some(&today)) {
                            eprintln!("  Warning: failed to import {}: {}", name, e);
                        }
                        std::fs::remove_file(&tmp_path).ok();
                    }
                    _ => {
                        eprintln!("  Warning: failed to download {}", name);
                    }
                }
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            eprintln!(
                "tm3-import-docs not available or failed: {}",
                stderr.trim()
            );
            eprintln!("Use `clinical import-doc <client_id> --pdf <path>` to import a local PDF instead.");
        }
        Err(_) => {
            eprintln!("tm3-import-docs not found on PATH.");
            eprintln!("Use `clinical import-doc <client_id> --pdf <path>` to import a local PDF instead.");
        }
    }

    Ok(())
}

fn uuid_short() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{:x}", t)
}
