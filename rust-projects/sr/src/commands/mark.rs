use anyhow::{bail, Context, Result};
use chrono::Utc;

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
fn find_card(card_id: &str) -> Result<std::path::PathBuf> {
    let root = config::sr_dir();

    if !root.exists() {
        bail!("sr directory does not exist: {}", root.display());
    }

    // Try {deck}/{id}.md for all subdirectories
    for entry in std::fs::read_dir(&root).with_context(|| format!("reading {}", root.display()))? {
        let entry = entry?;
        let deck_path = entry.path();
        if deck_path.is_dir() {
            let candidate = deck_path.join(format!("{}.md", card_id));
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    // Also try direct: {root}/{card_id}.md
    let direct = root.join(format!("{}.md", card_id));
    if direct.exists() {
        return Ok(direct);
    }

    bail!("card not found: {}", card_id)
}
