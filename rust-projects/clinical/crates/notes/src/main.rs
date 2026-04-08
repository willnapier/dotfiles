// Shared types live in the clinical-core crate; re-export them as
// `crate::client` and `crate::identity` so the rest of the bin source
// keeps using the existing paths unchanged.
use clinical_core::client;
use clinical_core::identity;

mod auth;
mod deidentify;
mod finalise;
mod letter;
mod markdown;
mod note;
mod populate;
mod portal_client;
mod prepare;
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

    /// One-shot session note: prepare → LLM → validate → confirm → append → finalise
    Note {
        /// Client ID (e.g. CT71)
        id: String,

        /// William's session observation
        observation: String,

        /// Skip confirmation prompt
        #[arg(long, short)]
        yes: bool,
    },

    /// Save a pre-drafted note from stdin: validate, append, finalise
    #[command(name = "note-save")]
    NoteSave {
        /// Client ID (e.g. CT71)
        id: String,
    },

    /// Update session count and print alerts after a note has been appended
    #[command(name = "note-finalise")]
    NoteFinalise {
        /// Client ID (e.g. CT71)
        id: String,
    },

    /// Pre-compute deterministic fields and template for a clinical note
    #[command(name = "note-prepare")]
    NotePrepare {
        /// Client ID (e.g. CT71)
        id: String,

        /// Number of recent sessions to include for context
        #[arg(long, default_value = "3")]
        sessions: usize,
    },

    /// Upload a built letter PDF and email a secure link to the recipient
    Share {
        /// Client ID (defaults to last clinical-letter-build state)
        client_id: Option<String>,

        /// Path to the PDF (defaults to last clinical-letter-build state)
        #[arg(long)]
        pdf: Option<String>,

        /// Recipient email (defaults to identity.yaml referrer.email)
        #[arg(long)]
        to: Option<String>,

        /// Recipient name (defaults to identity.yaml referrer.name)
        #[arg(long)]
        name: Option<String>,

        /// Link expiry in days
        #[arg(long, default_value = "14")]
        expiry_days: u32,

        /// Override the portal base URL
        #[arg(long)]
        portal_url: Option<String>,

        /// Print resolved values without uploading
        #[arg(long)]
        dry_run: bool,
    },

    /// List all shared documents and their status
    Status {
        /// Override the portal base URL
        #[arg(long)]
        portal_url: Option<String>,
    },

    /// Revoke a previously shared document by token
    Revoke {
        /// The token from the share link (UUID after /d/)
        token: String,

        /// Override the portal base URL
        #[arg(long)]
        portal_url: Option<String>,
    },

    /// Show recently changed files in ~/Clinical/clients
    Changes {
        /// Number of days to look back
        #[arg(long, default_value = "7")]
        days: u32,
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
        } => deidentify::run(&id, file.as_deref(), dry_run, list),
        Commands::ReIdentify {
            id,
            file,
            dry_run,
            name_form,
        } => reidentify::run(&id, &file, dry_run, &name_form),
        Commands::Auth { command } => match command {
            AuthCommands::Status { verbose } => auth::status(verbose),
            AuthCommands::Check { append } => auth::check(append),
            AuthCommands::Letter { id, dry_run } => auth::letter(&id, dry_run),
        },
        Commands::UpdateLetter { id, dry_run } => letter::run(&id, dry_run),
        Commands::Populate { apply } => populate::run(apply),
        Commands::Note {
            id,
            observation,
            yes,
        } => note::run(&id, &observation, yes),
        Commands::NoteSave { id } => note::save(&id),
        Commands::NoteFinalise { id } => finalise::run(&id),
        Commands::NotePrepare { id, sessions } => prepare::run(&id, sessions),
        Commands::Share {
            client_id,
            pdf,
            to,
            name,
            expiry_days,
            portal_url,
            dry_run,
        } => portal_client::share(client_id, pdf, to, name, expiry_days, portal_url, dry_run),
        Commands::Status { portal_url } => portal_client::status(portal_url),
        Commands::Revoke { token, portal_url } => portal_client::revoke(token, portal_url),
        Commands::Changes { days } => portal_client::changes(days),
    }
}
