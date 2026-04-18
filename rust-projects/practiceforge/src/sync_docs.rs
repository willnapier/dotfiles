//! Ongoing TM3 document sync — polls TM3 for newly added documents
//! across all known clients and imports them into local correspondence/.
//!
//! Differs from onboard.rs::download_and_import_docs (which runs once at
//! onboard): this command is idempotent and intended to run on a timer.
//!
//! Delegates per-client work to `clinical import-doc <id>`, which:
//!   - lists TM3 docs via `tm3-download list <tm3_id> --json`
//!   - downloads each non-skipped doc via `tm3-download get`
//!   - extracts text via pdftotext
//!   - saves to correspondence/ (Route C) with skip-if-exists dedup
//!
//! Dedup is filename-based: `YYYY-MM-DD-<doc_type>.md`. Already-present
//! files are silently skipped inside save_document_text().

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Result of a sync run for a single client.
#[derive(Debug)]
pub struct ClientSyncResult {
    pub client_id: String,
    pub tm3_id: Option<String>,
    /// None = skipped (no tm3_id); Some(err) = failure; empty = nothing to do.
    pub outcome: SyncOutcome,
}

#[derive(Debug)]
pub enum SyncOutcome {
    /// Client has no tm3_id in identity.yaml.
    NoTm3Id,
    /// Import ran, stderr captured for reporting.
    Ran { stderr: String, success: bool },
    /// tm3-download / clinical binaries unavailable or crashed.
    Error(String),
}

/// Aggregate result across all scanned clients.
#[derive(Debug, Default)]
pub struct SyncDocsResult {
    pub scanned: usize,
    pub no_tm3_id: usize,
    pub ran: usize,
    pub errored: usize,
    pub per_client: Vec<ClientSyncResult>,
}

/// Sync documents for all clients with a `tm3_id` in identity.yaml.
///
/// `dry_run` is passed through to `clinical import-doc --dry-run`, which
/// lists remote docs without downloading.
/// `client_filter` restricts the sync to a single client ID.
pub fn sync_all(dry_run: bool, client_filter: Option<&str>) -> Result<SyncDocsResult> {
    let clients_dir = clients_dir();
    if !clients_dir.exists() {
        anyhow::bail!("Clients directory not found: {}", clients_dir.display());
    }

    let mut result = SyncDocsResult::default();

    let ids = list_client_ids(&clients_dir)?;
    for id in ids {
        if let Some(filter) = client_filter {
            if id != filter {
                continue;
            }
        }

        result.scanned += 1;

        let tm3_id = read_tm3_id(&id);
        let outcome = match &tm3_id {
            None => {
                result.no_tm3_id += 1;
                SyncOutcome::NoTm3Id
            }
            Some(_) => run_import(&id, dry_run),
        };

        match &outcome {
            SyncOutcome::NoTm3Id => {}
            SyncOutcome::Ran { success, .. } => {
                if *success {
                    result.ran += 1;
                } else {
                    result.errored += 1;
                }
            }
            SyncOutcome::Error(_) => {
                result.errored += 1;
            }
        }

        result.per_client.push(ClientSyncResult {
            client_id: id,
            tm3_id,
            outcome,
        });
    }

    Ok(result)
}

fn clients_dir() -> PathBuf {
    if let Ok(root) = std::env::var("CLINICAL_ROOT") {
        PathBuf::from(root).join("clients")
    } else {
        dirs::home_dir()
            .expect("no home dir")
            .join("Clinical")
            .join("clients")
    }
}

fn list_client_ids(dir: &std::path::Path) -> Result<Vec<String>> {
    let mut ids: Vec<String> = fs::read_dir(dir)
        .with_context(|| format!("read {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    ids.sort();
    Ok(ids)
}

/// Cheap YAML scan — read identity.yaml and pull the tm3_id field.
/// We don't deserialize the whole identity here to avoid coupling
/// to the clinical-core schema.
fn read_tm3_id(client_id: &str) -> Option<String> {
    // Route C first (ident at client root), fall back to Route A (private/).
    let candidates = [
        clients_dir().join(client_id).join("identity.yaml"),
        clients_dir().join(client_id).join("private").join("identity.yaml"),
    ];
    for path in &candidates {
        if let Ok(text) = fs::read_to_string(path) {
            for line in text.lines() {
                let trimmed = line.trim_start();
                if let Some(rest) = trimmed.strip_prefix("tm3_id:") {
                    // Strip inline YAML comment before testing value.
                    let without_comment = rest.split('#').next().unwrap_or("");
                    let val = without_comment
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'');
                    if !val.is_empty() && val != "null" && val != "~" {
                        return Some(val.to_string());
                    }
                }
            }
        }
    }
    None
}

fn run_import(client_id: &str, dry_run: bool) -> SyncOutcome {
    let mut cmd = Command::new("clinical");
    cmd.arg("import-doc").arg(client_id);
    if dry_run {
        cmd.arg("--dry-run");
    }

    match cmd.output() {
        Ok(out) => SyncOutcome::Ran {
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            success: out.status.success(),
        },
        Err(e) => SyncOutcome::Error(format!("failed to invoke clinical: {e}")),
    }
}

/// Pretty-print a sync result to stderr.
pub fn print_report(result: &SyncDocsResult, verbose: bool) {
    eprintln!(
        "[sync-docs] scanned {} client{}, {} had tm3_id, {} skipped (no tm3_id), {} errored",
        result.scanned,
        if result.scanned == 1 { "" } else { "s" },
        result.ran + result.errored,
        result.no_tm3_id,
        result.errored,
    );

    if verbose {
        for item in &result.per_client {
            match &item.outcome {
                SyncOutcome::NoTm3Id => {
                    eprintln!("  {} — skip (no tm3_id)", item.client_id);
                }
                SyncOutcome::Ran { stderr, success } => {
                    let tag = if *success { "ok" } else { "fail" };
                    eprintln!("  {} — {}", item.client_id, tag);
                    for line in stderr.lines() {
                        if !line.trim().is_empty() {
                            eprintln!("      {}", line);
                        }
                    }
                }
                SyncOutcome::Error(e) => {
                    eprintln!("  {} — error: {}", item.client_id, e);
                }
            }
        }
    }
}
