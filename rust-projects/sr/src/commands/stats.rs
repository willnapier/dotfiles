use anyhow::Result;
use chrono::Utc;
use std::collections::BTreeMap;

use crate::card::{load_all_cards, load_deck, Card};

pub fn run(deck: Option<&str>) -> Result<()> {
    let cards: Vec<Card> = if let Some(d) = deck {
        let loaded = load_deck(d)?;
        if loaded.is_empty() {
            println!("Deck '{}' is empty or does not exist.", d);
            return Ok(());
        }
        loaded
    } else {
        load_all_cards()?
    };

    if cards.is_empty() {
        println!("No cards found.");
        return Ok(());
    }

    if let Some(deck_name) = deck {
        print_deck_stats(deck_name, &cards);
    } else {
        print_overall_stats(&cards);
    }

    Ok(())
}

fn categorise<'a>(cards: impl Iterator<Item = &'a Card>) -> (usize, usize, usize, usize) {
    let now = Utc::now();
    let mut due_today = 0;
    let mut overdue = 0;
    let mut new_cards = 0;
    let mut upcoming = 0;

    for card in cards {
        if card.reps == 0 {
            new_cards += 1;
            if card.due <= now {
                due_today += 1;
            }
        } else if card.due <= now {
            // Reviewed before but due again
            let days_over = (now - card.due).num_days();
            if days_over > 0 {
                overdue += 1;
            } else {
                due_today += 1;
            }
        } else {
            upcoming += 1;
        }
    }

    (due_today, overdue, new_cards, upcoming)
}

fn print_deck_stats(deck_name: &str, cards: &[Card]) {
    let total = cards.len();
    let (due_today, overdue, new_cards, upcoming) = categorise(cards.iter());

    println!("Deck: {}", deck_name);
    println!("─────────────────────────────────");
    println!("  Total      : {}", total);
    println!("  New        : {}", new_cards);
    println!("  Due today  : {}", due_today);
    println!("  Overdue    : {}", overdue);
    println!("  Upcoming   : {}", upcoming);
}

fn print_overall_stats(cards: &[Card]) {
    // Group by deck
    let mut by_deck: BTreeMap<&str, Vec<&Card>> = BTreeMap::new();
    for card in cards {
        by_deck.entry(&card.deck).or_default().push(card);
    }

    let now = Utc::now();
    let total = cards.len();
    let total_due: usize = cards.iter().filter(|c| c.due <= now).count();
    let total_new: usize = cards.iter().filter(|c| c.reps == 0).count();
    let total_overdue: usize = cards
        .iter()
        .filter(|c| c.due <= now && (now - c.due).num_days() > 0 && c.reps > 0)
        .count();

    println!("Overall statistics");
    println!("══════════════════════════════════════════════════");
    println!("  Total cards  : {}", total);
    println!("  New cards    : {}", total_new);
    println!("  Due now      : {}", total_due);
    println!("  Overdue      : {}", total_overdue);
    println!();
    println!("Per-deck breakdown");
    println!("──────────────────────────────────────────────────");
    println!(
        "{:<20}  {:>6}  {:>5}  {:>8}  {:>8}",
        "DECK", "TOTAL", "NEW", "DUE", "OVERDUE"
    );
    println!("{}", "─".repeat(58));

    for (deck_name, deck_cards) in &by_deck {
        let (due_today, overdue, new_cards, _) = categorise(deck_cards.iter().copied());
        println!(
            "{:<20}  {:>6}  {:>5}  {:>8}  {:>8}",
            deck_name,
            deck_cards.len(),
            new_cards,
            due_today,
            overdue,
        );
    }
    println!("{}", "─".repeat(58));
    println!(
        "{:<20}  {:>6}  {:>5}  {:>8}  {:>8}",
        "TOTAL", total, total_new, total_due, total_overdue
    );
}
