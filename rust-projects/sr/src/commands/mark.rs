use anyhow::{bail, Context, Result};
use chrono::Utc;
use std::path::{Path, PathBuf};

use crate::card::Card;
use crate::config;
use crate::scheduler::{schedule, Rating};

pub fn run(card_id: &str, pass: Option<bool>, rating: Option<u8>) -> Result<()> {
    // Resolve rating
    let rating = match (pass, rating) {
        (_, Some(r)) => Rating::from_u8(r)
            .ok_or_else(|| anyhow::anyhow!("invalid rating: {} (must be 1–4)", r))?,
        (Some(true), None) => Rating::Good,   // pass = Good (3)
        (Some(false), None) => Rating::Again, // fail = Again (1)
        (None, None) => bail!("specify pass/fail or --rating"),
    };

    // Find card file — search all decks
    let path = find_card(card_id)?;
    let mut card = Card::load(&path).with_context(|| format!("loading card {}", card_id))?;

    let now = Utc::now();
    let elapsed_days = card
        .last_review
        .map(|lr| now.signed_duration_since(lr).num_seconds() as f64 / 86400.0)
        .unwrap_or(0.0);

    let result = schedule(
        card.stability,
        card.difficulty,
        card.reps,
        elapsed_days,
        rating,
    );

    card.stability = result.stability;
    card.difficulty = result.difficulty;
    card.reps = result.reps;
    card.last_review = Some(now);
    card.due = now + chrono::Duration::days(result.interval_days as i64);

    card.save()?;

    println!(
        "{}: {} → next review in {} day(s) ({})",
        card.id,
        rating,
        result.interval_days,
        card.due.format("%Y-%m-%d"),
    );

    Ok(())
}

/// Search all deck directories for a card by ID.
fn find_card(card_id: &str) -> Result<PathBuf> {
    find_card_in(&config::sr_dir(), card_id)
}

/// The typed ID is never joined into a path: each deck listing is searched by
/// NFC comparison (forge-names) and the entry's own path is returned, so an
/// NFD-named card file written on the Mac is found from an NFC ID.
fn find_card_in(root: &Path, card_id: &str) -> Result<PathBuf> {
    if !root.exists() {
        bail!("sr directory does not exist: {}", root.display());
    }
    let file = format!("{}.md", card_id);

    // Try {deck}/{id}.md for all subdirectories
    for entry in std::fs::read_dir(root).with_context(|| format!("reading {}", root.display()))? {
        let deck_path = entry?.path();
        if deck_path.is_dir() {
            if let Some(found) = forge_names::find_in_dir(&deck_path, &file) {
                return Ok(found);
            }
        }
    }

    // Also try direct: {root}/{card_id}.md
    if let Some(found) = forge_names::find_in_dir(root, &file) {
        return Ok(found);
    }

    bail!("card not found: {}", card_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_finds_an_nfd_named_card_file_from_an_nfc_id() {
        let root = tempfile::tempdir().unwrap();
        let deck = root.path().join("names");
        std::fs::create_dir(&deck).unwrap();
        let on_disk = deck.join("Zoe\u{0308}-0000abcd.md");
        std::fs::write(&on_disk, "").unwrap();
        std::fs::write(deck.join("other-00000000.md"), "").unwrap();

        let found = find_card_in(root.path(), "Zoë-0000abcd").unwrap();
        assert_eq!(found.parent().unwrap(), deck);
        assert_eq!(forge_names::file_name(&found), "Zoë-0000abcd.md");
        assert!(found.exists());
        assert!(find_card_in(root.path(), "Zoë-ffffffff").is_err());
    }
}
