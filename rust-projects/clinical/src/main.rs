mod auth;
mod client;
mod deidentify;
mod identity;
mod letter;
mod markdown;
mod populate;
mod reidentify;
mod scaffold;
mod session;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "clinical", about = "Cross-platform clinical notes toolchain")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new client directory with all required files
    Scaffold {
        /// Client ID (e.g. PM84)
        id: String,
    },

    /// De-identify a file using identity.yaml substitutions
    #[command(name = "de-identify")]
    DeIdentify {
        /// Client ID
        id: String,

        /// File to de-identify (in private/), or omit to list available files
        file: Option<String>,

        /// Preview substitutions without writing
        #[arg(long)]
        dry_run: bool,

        /// List files available in private/
        #[arg(long)]
        list: bool,
    },

    /// Re-identify a de-identified file, restoring real names
    #[command(name = "re-identify")]
    ReIdentify {
        /// Client ID
        id: String,

        /// De-identified file to restore
        file: String,

        /// Preview substitutions without writing
        #[arg(long)]
        dry_run: bool,

        /// Name form to use in body text: full, first, or title
        #[arg(long, default_value = "full")]
        name_form: String,
    },

    /// Authorisation tracking commands
    Auth {
        #[command(subcommand)]
        command: AuthCommands,
    },

    /// Generate a draft update letter to the referring clinician
    #[command(name = "update-letter")]
    UpdateLetter {
        /// Client ID
        id: String,

        /// Preview without writing
        #[arg(long)]
        dry_run: bool,
    },

    /// Populate computed fields in client .md files
    Populate {
        /// Apply changes (default is preview only)
        #[arg(long)]
        apply: bool,
    },
}

#[derive(Subcommand)]
enum AuthCommands {
    /// Report authorisation status for all insured clients
    Status {
        /// Show detailed output
        #[arg(long)]
        verbose: bool,
    },

    /// Check for expiring authorisations
    Check {
        /// Append warnings to today's DayPage
        #[arg(long)]
        append: bool,
    },

    /// Generate a draft re-authorisation letter
    Letter {
        /// Client ID
        id: String,

        /// Preview without writing
        #[arg(long)]
        dry_run: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Scaffold { id } => scaffold::run(&id),
        Commands::DeIdentify {
            id,
            file,
            dry_run,
            list,
        } => {
            eprintln!("clinical de-identify: not yet implemented");
            let _ = (id, file, dry_run, list);
            Ok(())
        }
        Commands::ReIdentify {
            id,
            file,
            dry_run,
            name_form,
        } => {
            eprintln!("clinical re-identify: not yet implemented");
            let _ = (id, file, dry_run, name_form);
            Ok(())
        }
        Commands::Auth { command } => match command {
            AuthCommands::Status { verbose } => {
                eprintln!("clinical auth status: not yet implemented");
                let _ = verbose;
                Ok(())
            }
            AuthCommands::Check { append } => {
                eprintln!("clinical auth check: not yet implemented");
                let _ = append;
                Ok(())
            }
            AuthCommands::Letter { id, dry_run } => {
                eprintln!("clinical auth letter: not yet implemented");
                let _ = (id, dry_run);
                Ok(())
            }
        },
        Commands::UpdateLetter { id, dry_run } => {
            eprintln!("clinical update-letter: not yet implemented");
            let _ = (id, dry_run);
            Ok(())
        }
        Commands::Populate { apply } => {
            eprintln!("clinical populate: not yet implemented");
            let _ = apply;
            Ok(())
        }
    }
}
