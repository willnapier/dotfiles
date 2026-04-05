mod card;
mod commands;
mod config;
mod scheduler;

use anyhow::Result;
use clap::{Parser, Subcommand};

use commands::{due::Format as DueFormat, import::ImportFormat};

#[derive(Debug, Parser)]
#[command(
    name = "sr",
    about = "Spaced retrieval — FSRS-4 flashcard scheduler with plain-text markdown cards",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Create a new card
    Add {
        /// Deck name (e.g. german, anatomy)
        #[arg(long, short)]
        deck: String,

        /// Question text (omit to prompt interactively, or pipe Q:/A: via stdin)
        #[arg(long, short)]
        question: Option<String>,

        /// Answer text
        #[arg(long, short)]
        answer: Option<String>,
    },

    /// List cards due for review
    Due {
        /// Filter to specific deck
        #[arg(long, short)]
        deck: Option<String>,

        /// Maximum number of cards to show
        #[arg(long, short)]
        limit: Option<usize>,

        /// Output format: table (default), voice, json
        #[arg(long, short, default_value = "table")]
        format: String,
    },

    /// Interactive terminal review session
    Review {
        /// Filter to specific deck
        #[arg(long, short)]
        deck: Option<String>,

        /// Maximum number of cards per session
        #[arg(long, short)]
        limit: Option<usize>,
    },

    /// Mark a card as passed or failed (for AI/programmatic use)
    Mark {
        /// Card ID
        card_id: String,

        /// Pass (Good=3) or fail (Again=1)
        result: Option<String>,

        /// Explicit rating (1=Again, 2=Hard, 3=Good, 4=Easy)
        #[arg(long, short)]
        rating: Option<u8>,
    },

    /// Import cards from stdin (plain text or anki-cards JSON)
    Import {
        /// Target deck
        #[arg(long, short)]
        deck: String,

        /// Input format: text (default) or anki-cards
        #[arg(long, short, default_value = "text")]
        format: String,
    },

    /// Show review statistics
    Stats {
        /// Filter to specific deck
        #[arg(long, short)]
        deck: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Add {
            deck,
            question,
            answer,
        } => {
            commands::add::run(&deck, question.as_deref(), answer.as_deref())?;
        }

        Commands::Due {
            deck,
            limit,
            format,
        } => {
            let fmt = parse_due_format(&format)?;
            commands::due::run(deck.as_deref(), limit, fmt)?;
        }

        Commands::Review { deck, limit } => {
            commands::review::run(deck.as_deref(), limit)?;
        }

        Commands::Mark {
            card_id,
            result,
            rating,
        } => {
            let pass: Option<bool> = match result.as_deref() {
                Some("pass") | Some("passed") => Some(true),
                Some("fail") | Some("failed") => Some(false),
                Some(other) => {
                    anyhow::bail!("unknown result '{}': use 'pass' or 'fail'", other)
                }
                None => None,
            };
            commands::mark::run(&card_id, pass, rating)?;
        }

        Commands::Import { deck, format } => {
            let fmt = parse_import_format(&format)?;
            commands::import::run(&deck, fmt)?;
        }

        Commands::Stats { deck } => {
            commands::stats::run(deck.as_deref())?;
        }
    }

    Ok(())
}

fn parse_due_format(s: &str) -> Result<DueFormat> {
    match s {
        "table" => Ok(DueFormat::Table),
        "voice" => Ok(DueFormat::Voice),
        "json" => Ok(DueFormat::Json),
        other => anyhow::bail!("unknown format '{}': use table, voice, or json", other),
    }
}

fn parse_import_format(s: &str) -> Result<ImportFormat> {
    match s {
        "text" => Ok(ImportFormat::Text),
        "anki-cards" => Ok(ImportFormat::AnkiCards),
        other => anyhow::bail!("unknown import format '{}': use text or anki-cards", other),
    }
}
