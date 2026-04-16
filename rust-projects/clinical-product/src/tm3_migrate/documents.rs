//! Document export — thin wrapper around `tm3-download get-all`.
//!
//! For each client in the registry that has a TM3 ID, invokes
//! the existing tm3_download binary to fetch documents into the
//! registry's correspondence directory.

use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;

use crate::registry::config::RegistryConfig;

/// Result of a document export run.
#[derive(Debug, Clone, Default)]
pub struct DocReport {
    pub downloaded: usize,
    pub already_present: usize,
    pub failed: Vec<String>,
}

impl DocReport {
    pub fn total_processed(&self) -> usize {
        self.downloaded + self.already_present + self.failed.len()
    }
}

impl std::fmt::Display for DocReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Documents: {} downloaded, {} already present, {} failed",
            self.downloaded,
            self.already_present,
            self.failed.len()
        )
    }
}

/// Export documents for all clients in the registry that have TM3 IDs.
///
/// Uses the `tm3-download` binary (expected on PATH) to download
/// documents from TM3 into each client's `correspondence/` directory
/// in the registry.
pub fn export_documents(
    registry_config: &RegistryConfig,
    dry_run: bool,
) -> Result<DocReport> {
    let mut report = DocReport::default();

    let clients = crate::registry::list_clients(registry_config)?;
    let clients_with_tm3: Vec<_> = clients.iter().filter(|c| c.tm3_id.is_some()).collect();

    eprintln!(
        "[tm3-migrate] {} clients with TM3 IDs in registry",
        clients_with_tm3.len()
    );

    for client in &clients_with_tm3 {
        let tm3_id = client.tm3_id.unwrap();
        let correspondence_dir = registry_config
            .client_dir(&client.client_id)
            .join("correspondence");

        // Check if correspondence already has files
        let existing_count = if correspondence_dir.exists() {
            std::fs::read_dir(&correspondence_dir)?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_file())
                .count()
        } else {
            0
        };

        if existing_count > 0 {
            report.already_present += 1;
            continue;
        }

        if dry_run {
            println!(
                "  Would download documents for {} (TM3 #{}) -> {}",
                client.client_id,
                tm3_id,
                correspondence_dir.display()
            );
            report.downloaded += 1;
            continue;
        }

        std::fs::create_dir_all(&correspondence_dir)?;

        // Invoke tm3-download binary
        match run_tm3_download(tm3_id, &correspondence_dir) {
            Ok(count) => {
                if count > 0 {
                    println!(
                        "  {} (TM3 #{}): {} document(s) downloaded",
                        client.client_id, tm3_id, count
                    );
                    report.downloaded += 1;
                } else {
                    report.already_present += 1;
                }
            }
            Err(e) => {
                report.failed.push(format!(
                    "{} (TM3 #{}): {}",
                    client.client_id, tm3_id, e
                ));
            }
        }
    }

    Ok(report)
}

/// Run the `tm3-download` binary for a specific TM3 client.
/// Returns the number of documents downloaded.
fn run_tm3_download(tm3_id: u64, output_dir: &PathBuf) -> Result<usize> {
    let output = Command::new("tm3-download")
        .args(["get-all", "--client-id", &tm3_id.to_string()])
        .arg("--output-dir")
        .arg(output_dir)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("tm3-download failed: {}", stderr.trim());
    }

    // Count files in output directory after download
    let count = std::fs::read_dir(output_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .count();

    Ok(count)
}
