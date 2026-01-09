use anyhow::Result;
use clap::{Parser, Subcommand};

mod changelog;
mod export;
mod import;
mod scrolls;

#[derive(Parser)]
#[command(name = "module")]
#[command(about = "Cross-platform module/scroll management for AI advisor sessions")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Export scrolls for a specific advisor session
    Export {
        /// Advisor name (seneca, geoff, or custom)
        advisor: String,

        /// Output directory (default: ~/Downloads)
        #[arg(short, long)]
        output: Option<String>,

        /// Create zip bundle instead of directory
        #[arg(short, long)]
        zip: bool,
    },

    /// Import and apply module updates from conversation JSON
    Import {
        /// Path to conversation JSON file
        file: String,

        /// Dry run - show what would be updated without applying
        #[arg(short, long)]
        dry_run: bool,
    },

    /// Verify scroll consistency and completeness
    Verify,

    /// List current scroll state
    List {
        /// Show full content, not just summary
        #[arg(short, long)]
        full: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Export { advisor, output, zip } => {
            export::run(&advisor, output.as_deref(), zip)
        }
        Commands::Import { file, dry_run } => {
            import::run(&file, dry_run)
        }
        Commands::Verify => {
            scrolls::verify()
        }
        Commands::List { full } => {
            scrolls::list(full)
        }
    }
}
