use anyhow::Result;
use chrono::Utc;

use crate::card::{load_all_cards, load_deck, Card};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Table,
    Voice,
    Json,
}

pub fn run(deck: Option<&str>, limit: Option<usize>, format: Format) -> Result<()> {
    let mut cards: Vec<Card> = if let Some(d) = deck {
        load_deck(d)?
    } else {
        load_all_cards()?
    };

    // Filter to due cards
    let now = Utc::now();
    cards.retain(|c| c.due <= now);

    // Sort by most overdue first
    cards.sort_by(|a, b| {
        a.due
            .partial_cmp(&b.due)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Apply limit
    if let Some(n) = limit {
        cards.truncate(n);
    }

    if cards.is_empty() {
        match format {
            Format::Json => println!("[]"),
            _ => println!("No cards due."),
        }
        return Ok(());
    }

    match format {
        Format::Table => print_table(&cards),
        Format::Voice => print_voice(&cards),
        Format::Json => print_json(&cards)?,
    }

    Ok(())
}

fn print_table(cards: &[Card]) {
    println!("{:<28}  {:<15}  {:<19}  QUESTION", "ID", "DECK", "DUE");
    println!("{}", "-".repeat(85));
    for card in cards {
        println!(
            "{:<28}  {:<15}  {:<19}  {}",
            card.id,
            card.deck,
            card.due.format("%Y-%m-%d %H:%M"),
            card.question_preview(),
        );
    }
    println!("\n{} card(s) due.", cards.len());
}

fn print_voice(cards: &[Card]) {
    println!("# Spaced Retrieval Session\n");
    println!(
        "Quiz me on these {} cards. When I get one wrong, return it to the queue",
        cards.len()
    );
    println!("and re-test after 5-10 other cards. Keep cycling failed items until I get");
    println!(
        "each one right twice. End with a structured results block: PASSED and FAILED lists.\n"
    );
    println!("---\n");
    for (i, card) in cards.iter().enumerate() {
        println!("{}.", i + 1);
        println!("Q: {}", card.question);
        println!("A: {}", card.answer);
        println!();
    }
}

fn print_json(cards: &[Card]) -> Result<()> {
    // Hand-roll JSON to avoid serde_json dependency
    print!("[");
    for (i, card) in cards.iter().enumerate() {
        if i > 0 {
            print!(",");
        }
        let q = card.question.replace('"', "\\\"").replace('\n', "\\n");
        let a = card.answer.replace('"', "\\\"").replace('\n', "\\n");
        print!(
            "\n  {{\"id\":\"{}\",\"deck\":\"{}\",\"due\":\"{}\",\"reps\":{},\"question\":\"{}\",\"answer\":\"{}\"}}",
            card.id,
            card.deck,
            card.due.to_rfc3339(),
            card.reps,
            q,
            a,
        );
    }
    println!("\n]");
    Ok(())
}
