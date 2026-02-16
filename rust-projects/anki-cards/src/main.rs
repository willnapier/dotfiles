mod anki;
mod extract;
mod preview;

use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::io::Read;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    pub front: String,
    pub back: String,
}

#[derive(Parser)]
#[command(name = "anki-cards")]
#[command(about = "Extract flashcards from text via LLM, push to Anki")]
struct Cli {
    /// Input file (reads stdin if omitted)
    file: Option<String>,

    /// Target Anki deck name
    #[arg(long, default_value = "Continuum")]
    deck: String,

    /// Skip preview, push immediately
    #[arg(long)]
    yes: bool,

    /// Extract and display only, don't push to Anki
    #[arg(long)]
    dry_run: bool,

    /// Output extracted cards as JSON (no Anki interaction)
    #[arg(long)]
    json: bool,
}

fn read_input(file: Option<&str>) -> Result<String> {
    match file {
        Some(path) => std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read file: {}", path)),
        None => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("Failed to read stdin")?;
            Ok(buf)
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let input = read_input(cli.file.as_deref())?;
    if input.trim().is_empty() {
        anyhow::bail!("Input is empty — nothing to extract");
    }

    eprintln!("Extracting cards via claude...");
    let cards = extract::extract_cards(&input)?;

    if cards.is_empty() {
        eprintln!("No cards extracted from input.");
        return Ok(());
    }

    // --json: output JSON and exit
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&cards)?);
        return Ok(());
    }

    // --dry-run or interactive: display cards
    preview::display_cards(&cards);

    if cli.dry_run {
        return Ok(());
    }

    // Decide which cards to push
    let cards_to_push = if cli.yes {
        cards
    } else {
        match preview::confirm_push(&cli.deck, &cards)? {
            preview::Confirmation::Yes => cards,
            preview::Confirmation::No => {
                eprintln!("Aborted.");
                return Ok(());
            }
            preview::Confirmation::Edit(edited) => edited,
        }
    };

    if cards_to_push.is_empty() {
        eprintln!("No cards to push.");
        return Ok(());
    }

    // Push to Anki
    anki::create_deck(&cli.deck)?;
    let added = anki::add_notes(&cli.deck, &cards_to_push)?;

    let skipped = cards_to_push.len() - added;
    eprint!("Pushed {} card{} to deck \"{}\"", added, if added == 1 { "" } else { "s" }, cli.deck);
    if skipped > 0 {
        eprint!(" ({} duplicate{} skipped)", skipped, if skipped == 1 { "" } else { "s" });
    }
    eprintln!(".");

    Ok(())
}
