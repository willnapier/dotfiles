use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

use tm3_diary_capture::client_map::ClientMap;

#[derive(Parser)]
#[command(about = "Add a TM3 client mapping and scaffold client files")]
struct Cli {
    /// TM3 client name exactly as it appears (e.g. "Surname, Firstname")
    name: String,

    /// Client ID (e.g. BB88, IR)
    id: String,

    /// Override client mapping file path
    #[arg(long)]
    map_file: Option<PathBuf>,

    /// Skip scaffolding client files
    #[arg(long)]
    no_scaffold: bool,

    /// Skip re-running tm3-diary-capture
    #[arg(long)]
    no_recapture: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let map_path = cli.map_file.unwrap_or_else(ClientMap::default_path);

    // 1. Check if already mapped
    if let Ok(map) = ClientMap::load(&map_path) {
        if let Some(existing_id) = map.lookup(&cli.name) {
            eprintln!(
                "\"{}\" is already mapped to \"{}\"",
                cli.name, existing_id
            );
            return Ok(());
        }
    }

    // 2. Append to client map toml
    append_to_map(&map_path, &cli.name, &cli.id)?;
    eprintln!("Added: \"{}\" = \"{}\"", cli.name, cli.id);

    // 3. Scaffold client files (idempotent)
    if !cli.no_scaffold {
        eprintln!();
        eprintln!("Scaffolding client files for {}...", cli.id);
        let status = std::process::Command::new("clinical-scaffold-client")
            .arg(&cli.id)
            .status()
            .context("Failed to run clinical-scaffold-client")?;
        if !status.success() {
            eprintln!("Warning: clinical-scaffold-client exited with error");
        }
    }

    // 4. Re-run tm3-diary-capture on any retained HTML
    if !cli.no_recapture {
        let downloads = dirs::download_dir().context("Could not find Downloads directory")?;
        let tm3_files: Vec<_> = std::fs::read_dir(&downloads)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_lowercase();
                name.ends_with(".html") && name.contains("tm3")
            })
            .collect();

        if tm3_files.is_empty() {
            eprintln!();
            eprintln!("No TM3 HTML files in Downloads — nothing to re-process.");
        } else {
            eprintln!();
            eprintln!("Re-running tm3-diary-capture --latest --include-past...");
            let status = std::process::Command::new("tm3-diary-capture")
                .args(["--latest", "--include-past"])
                .status()
                .context("Failed to run tm3-diary-capture")?;
            if !status.success() {
                eprintln!("Warning: tm3-diary-capture exited with error");
            }
        }
    }

    Ok(())
}

/// Append a new entry to the [clients] section of the toml file.
fn append_to_map(path: &PathBuf, name: &str, id: &str) -> Result<()> {
    use std::io::Write;

    // Ensure the file exists with a [clients] header
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, "[clients]\n")?;
    }

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .with_context(|| format!("Failed to open: {}", path.display()))?;

    writeln!(file, "\"{}\" = \"{}\"", name, id)
        .with_context(|| format!("Failed to write to: {}", path.display()))?;

    Ok(())
}
