mod auth;
mod batch;
mod deidentify;
mod faithfulness;
mod import;
mod finalise;
mod letter;
mod markdown;
mod migrate;
mod note;
mod populate;
mod portal_client;
mod prepare;
mod reidentify;
mod scaffold;
mod session;
mod training;

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
    /// Replace 'Client' with first name in notes (reverses de-identification)
    Personalize {
        /// Client ID, or omit for --all
        id: Option<String>,
        /// Process all clients
        #[arg(long)]
        all: bool,
        /// Preview changes without writing
        #[arg(long)]
        dry_run: bool,
    },

    /// Migrate client(s) from Route A (private/) to Route C (flat) layout
    Migrate {
        /// Client ID, or omit for --all
        id: Option<String>,
        /// Migrate all Route A clients
        #[arg(long)]
        all: bool,
        /// Preview changes without writing
        #[arg(long)]
        dry_run: bool,
    },

    /// Create a new client directory with all required files (Route C by default)
    Scaffold {
        /// Client ID (e.g. PM84)
        id: String,
        /// Use legacy Route A layout (private/ directory, de-identification)
        #[arg(long)]
        route_a: bool,
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

        /// Exclude this note from future voice fine-tuning training data
        #[arg(long)]
        no_train: bool,

        /// Override the model name for this run (e.g. clinical-voice or clinical-voice-q4)
        #[arg(long)]
        model_override: Option<String>,

        /// Generate and display the note but do NOT append to client file or finalise
        #[arg(long)]
        no_save: bool,

        /// Skip confirmation prompt
        #[arg(long, short)]
        yes: bool,
    },

    /// Save a pre-drafted note from stdin: validate, append, finalise
    #[command(name = "note-save")]
    NoteSave {
        /// Client ID (e.g. CT71)
        id: String,

        /// Exclude this note from future voice fine-tuning training data
        #[arg(long)]
        no_train: bool,
    },

    /// Retroactively mark a session note to exclude from (or include in) training data
    #[command(name = "note-mark")]
    NoteMark {
        /// Client ID
        id: String,

        /// Session date (YYYY-MM-DD)
        date: String,

        /// Exclude this note from training data
        #[arg(long, conflicts_with = "include")]
        exclude: bool,

        /// Include this note in training data (remove previous exclusion)
        #[arg(long, conflicts_with = "exclude")]
        include: bool,
    },

    /// Training data corpus commands
    Training {
        #[command(subcommand)]
        command: TrainingCommands,
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

    /// Import referral documents from TM3 or local PDFs into the client directory.
    #[command(name = "import-doc")]
    ImportDoc {
        /// Client ID
        id: String,

        /// Path to a local PDF file to import (bypasses TM3)
        #[arg(long)]
        pdf: Option<String>,

        /// Document type override (referral, patient-info, gp-letter, assessment)
        #[arg(long, name = "type")]
        doc_type: Option<String>,

        /// Date for the document (YYYY-MM-DD, defaults to today or filename date)
        #[arg(long)]
        date: Option<String>,

        /// Preview without downloading or saving
        #[arg(long)]
        dry_run: bool,
    },

    /// Batch-process multiple session notes: write observations, cook, review, save.
    #[command(name = "notes-batch")]
    NotesBatch {
        /// Path to the observations file (markdown with # CLIENT_ID headings)
        file: String,

        /// Generate and display but do NOT save to client files
        #[arg(long)]
        no_save: bool,
    },

    /// Send a letter: secure link to recipient + file to TM3
    #[command(alias = "share")]
    Send {
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

    /// Generate a compressed clinical summary for a client (saves to summary.md)
    Summarise {
        /// Client ID (e.g. CT71), or omit for --all
        id: Option<String>,

        /// Summarise all clients
        #[arg(long)]
        all: bool,

        /// Preview without writing
        #[arg(long)]
        dry_run: bool,

        /// Override the model name
        #[arg(long)]
        model_override: Option<String>,
    },
}

#[derive(Subcommand)]
enum TrainingCommands {
    /// Count notes eligible for training (default: since last fine-tune)
    Count {
        /// Count all eligible notes (ignore last fine-tune date)
        #[arg(long)]
        all: bool,
    },

    /// List all notes with their training inclusion status
    List {
        /// Only show excluded notes
        #[arg(long)]
        excluded: bool,
    },

    /// Export eligible notes as JSONL for voice model fine-tuning
    Export {
        /// Output file path (default: stdout)
        #[arg(long, short)]
        output: Option<String>,

        /// Export all eligible notes (ignore last fine-tune date)
        #[arg(long)]
        all: bool,
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

    /// Extract form data for insurer authorisation (JSON payload for Healthcode)
    Form {
        /// Client ID
        id: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Personalize { id, all, dry_run } => {
            if all {
                migrate::personalize_all(dry_run)
            } else if let Some(id) = id {
                migrate::personalize(&id, dry_run)
            } else {
                anyhow::bail!("Provide a client ID or use --all")
            }
        }
        Commands::Migrate { id, all, dry_run } => {
            if all {
                migrate::run_all(dry_run)
            } else if let Some(id) = id {
                migrate::run(&id, dry_run)
            } else {
                anyhow::bail!("Provide a client ID or use --all")
            }
        }
        Commands::Scaffold { id, route_a } => scaffold::run(&id, route_a),
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
            AuthCommands::Form { id } => auth::form(&id),
        },
        Commands::UpdateLetter { id, dry_run } => letter::run(&id, dry_run),
        Commands::Populate { apply } => populate::run(apply),
        Commands::Note {
            id,
            observation,
            no_train,
            model_override,
            no_save,
            yes,
        } => note::run(
            &id,
            &observation,
            no_train,
            model_override.as_deref(),
            no_save,
            yes,
        ),
        Commands::NoteSave { id, no_train } => note::save(&id, no_train),
        Commands::NoteMark {
            id,
            date,
            exclude,
            include,
        } => note::mark(&id, &date, exclude, include),
        Commands::Training { command } => match command {
            TrainingCommands::Count { all } => training::count(all),
            TrainingCommands::List { excluded } => training::list(excluded),
            TrainingCommands::Export { output, all } => {
                training::export(output.as_deref(), all)
            }
        },
        Commands::NoteFinalise { id } => finalise::run(&id),
        Commands::NotePrepare { id, sessions } => prepare::run(&id, sessions),
        Commands::ImportDoc {
            id,
            pdf,
            doc_type,
            date,
            dry_run,
        } => {
            if let Some(pdf_path) = pdf {
                import::import_local_pdf(
                    &id,
                    &pdf_path,
                    doc_type.as_deref(),
                    date.as_deref(),
                )
            } else {
                import::import_from_tm3(&id, dry_run)
            }
        }
        Commands::NotesBatch { file, no_save } => batch::run(&file, no_save),
        Commands::Send {
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
        Commands::Summarise {
            id,
            all,
            dry_run,
            model_override,
        } => note::summarise(id.as_deref(), all, dry_run, model_override.as_deref()),
    }
}
