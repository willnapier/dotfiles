mod capture;
mod check;
mod config;
mod preset;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process;

#[derive(Parser)]
#[command(name = "state-capture", about = "Config-driven system state capture and drift detection")]
struct Cli {
    /// Path to config file
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Run all captures and write output files
    Capture {
        /// Show what would be captured without writing
        #[arg(long)]
        dry_run: bool,
    },
    /// Compare live state against saved baselines
    Check {
        /// Suppress output, exit code only (0=clean, 1=drift)
        #[arg(short, long)]
        quiet: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Create a config file with distro-specific presets
    Init {
        /// Distro preset (auto-detects if omitted)
        #[arg(long, value_enum)]
        preset: Option<preset::Preset>,
    },
    /// List configured captures
    List,
    /// Show a capture's baseline content
    Show {
        /// Capture name
        name: String,
    },
}

fn main() {
    let cli = Cli::parse();

    let result = match &cli.command {
        Some(Commands::Init { preset }) => cmd_init(preset.as_ref()),
        _ => {
            let config_path = cli
                .config
                .clone()
                .unwrap_or_else(config::default_config_path);
            run_with_config(&cli, &config_path)
        }
    };

    if let Err(e) = result {
        eprintln!("error: {:#}", e);
        process::exit(1);
    }
}

fn run_with_config(cli: &Cli, config_path: &PathBuf) -> Result<()> {
    let cfg = config::load_config(config_path)?;

    match &cli.command {
        None | Some(Commands::Capture { dry_run: false }) => {
            let ok = capture::run_all(&cfg, false)?;
            if !ok {
                process::exit(1);
            }
        }
        Some(Commands::Capture { dry_run: true }) => {
            capture::run_all(&cfg, true)?;
        }
        Some(Commands::Check { quiet, json }) => {
            let report = check::check_drift(&cfg, *quiet)?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
            if report.has_drift {
                process::exit(1);
            }
        }
        Some(Commands::List) => {
            capture::list_captures(&cfg);
        }
        Some(Commands::Show { name }) => {
            capture::show_capture(&cfg, name)?;
        }
        Some(Commands::Init { .. }) => unreachable!(),
    }

    Ok(())
}

fn cmd_init(preset: Option<&preset::Preset>) -> Result<()> {
    let chosen = match preset {
        Some(p) => *p,
        None => {
            match preset::detect_distro() {
                Some(p) => {
                    println!("Auto-detected distro: {}", p);
                    p
                }
                None => {
                    anyhow::bail!(
                        "Could not auto-detect distro. Use --preset arch|debian|fedora"
                    );
                }
            }
        }
    };

    let config_path = config::default_config_path();
    if config_path.exists() {
        anyhow::bail!(
            "Config already exists: {} (remove it first to reinitialise)",
            config_path.display()
        );
    }

    let cfg = preset::preset_config(chosen);
    let toml_str = toml::to_string_pretty(&cfg)?;

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&config_path, &toml_str)?;

    println!("Created config: {}", config_path.display());
    println!("Preset: {} ({} captures)", chosen, cfg.captures.len());
    Ok(())
}
