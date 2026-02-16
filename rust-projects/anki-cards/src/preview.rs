use anyhow::{Context, Result};
use std::io::{self, BufRead, Write};

use crate::Card;

pub enum Confirmation {
    Yes,
    No,
    Edit(Vec<Card>),
}

pub fn display_cards(cards: &[Card]) {
    eprintln!("\nExtracted {} card{}:\n", cards.len(), if cards.len() == 1 { "" } else { "s" });
    for (i, card) in cards.iter().enumerate() {
        eprintln!("  {}. Q: {}", i + 1, card.front);
        eprintln!("     A: {}\n", card.back);
    }
}

pub fn confirm_push(deck: &str, cards: &[Card]) -> Result<Confirmation> {
    eprint!(
        "Push {} card{} to deck \"{}\"? [Y/n/e(dit)] ",
        cards.len(),
        if cards.len() == 1 { "" } else { "s" },
        deck
    );
    io::stderr().flush()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    let choice = input.trim().to_lowercase();

    match choice.as_str() {
        "" | "y" | "yes" => Ok(Confirmation::Yes),
        "n" | "no" => Ok(Confirmation::No),
        "e" | "edit" => {
            let edited = edit_cards(cards)?;
            Ok(Confirmation::Edit(edited))
        }
        _ => {
            eprintln!("Unknown choice '{}', aborting.", choice);
            Ok(Confirmation::No)
        }
    }
}

fn edit_cards(cards: &[Card]) -> Result<Vec<Card>> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());

    let tmp = std::env::temp_dir().join("anki-cards-edit.json");
    let json = serde_json::to_string_pretty(cards)?;
    std::fs::write(&tmp, &json).context("Failed to write temp file for editing")?;

    let status = std::process::Command::new(&editor)
        .arg(&tmp)
        .status()
        .context(format!("Failed to open editor '{}'", editor))?;

    if !status.success() {
        anyhow::bail!("Editor exited with non-zero status");
    }

    let edited = std::fs::read_to_string(&tmp).context("Failed to read edited file")?;
    let edited_cards: Vec<Card> =
        serde_json::from_str(&edited).context("Failed to parse edited JSON — is it valid?")?;

    let _ = std::fs::remove_file(&tmp);

    Ok(edited_cards)
}
