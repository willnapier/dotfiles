use anyhow::Result;
use chrono::Utc;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind},
    execute,
    terminal::{self, ClearType},
};
use std::io::{self, Write};

use crate::card::{load_all_cards, load_deck, Card};
use crate::scheduler::{schedule, Rating};

pub fn run(deck: Option<&str>, limit: Option<usize>) -> Result<()> {
    let mut cards: Vec<Card> = if let Some(d) = deck {
        load_deck(d)?
    } else {
        load_all_cards()?
    };

    // Filter to due cards only
    let now = Utc::now();
    cards.retain(|c| c.due <= now);

    // Sort: most overdue first
    cards.sort_by(|a, b| a.due.cmp(&b.due));

    if let Some(n) = limit {
        cards.truncate(n);
    }

    if cards.is_empty() {
        println!("No cards due. Enjoy your day!");
        return Ok(());
    }

    let total = cards.len();
    println!("Starting review: {} card(s)\n", total);
    println!(
        "Controls: [Space/Enter] reveal  |  [1] Again  [2] Hard  [3] Good  [4] Easy  |  [q] quit\n"
    );

    let mut reviewed = 0;
    let mut again_count = 0;
    let mut skipped = false;

    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();

    let result = review_loop(
        &mut cards,
        total,
        &mut reviewed,
        &mut again_count,
        &mut skipped,
        &mut stdout,
    );

    terminal::disable_raw_mode()?;
    // Always print a newline after leaving raw mode
    println!();

    result?;

    if !skipped {
        // Summary
        println!("\n─── Session summary ───────────────────────────────");
        println!("  Reviewed : {}", reviewed);
        println!("  Again    : {}", again_count);
        println!("  Passed   : {}", reviewed - again_count);
        println!("────────────────────────────────────────────────────");
    }

    Ok(())
}

fn review_loop(
    cards: &mut [Card],
    total: usize,
    reviewed: &mut usize,
    again_count: &mut usize,
    skipped: &mut bool,
    stdout: &mut io::Stdout,
) -> Result<()> {
    let mut i = 0;

    while i < cards.len() {
        let card = &cards[i];

        // ── Show question ──────────────────────────────────────
        execute!(stdout, terminal::Clear(ClearType::All))?;
        write!(
            stdout,
            "\r[{}/{}]  Deck: {}\r\n\r\n",
            i + 1,
            total,
            card.deck
        )?;
        write!(stdout, "\rQ: {}\r\n\r\n", card.question)?;
        write!(stdout, "\r  (press Space or Enter to reveal)\r\n")?;
        stdout.flush()?;

        // Wait for reveal keypress
        loop {
            if let Event::Key(KeyEvent {
                code,
                kind: KeyEventKind::Press,
                ..
            }) = event::read()?
            {
                match code {
                    KeyCode::Char(' ') | KeyCode::Enter => break,
                    KeyCode::Char('q') | KeyCode::Esc => {
                        *skipped = true;
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }

        // ── Show answer ────────────────────────────────────────
        let card = &cards[i];
        execute!(stdout, terminal::Clear(ClearType::All))?;
        write!(
            stdout,
            "\r[{}/{}]  Deck: {}\r\n\r\n",
            i + 1,
            total,
            card.deck
        )?;
        write!(stdout, "\rQ: {}\r\n\r\n", card.question)?;
        write!(stdout, "\rA: {}\r\n\r\n", card.answer.replace('\n', "\r\n"))?;
        write!(
            stdout,
            "\r  Rate: [1] Again  [2] Hard  [3] Good  [4] Easy\r\n"
        )?;
        stdout.flush()?;

        // Wait for rating keypress
        let rating = loop {
            if let Event::Key(KeyEvent {
                code,
                kind: KeyEventKind::Press,
                ..
            }) = event::read()?
            {
                match code {
                    KeyCode::Char('1') => break Rating::Again,
                    KeyCode::Char('2') => break Rating::Hard,
                    KeyCode::Char('3') => break Rating::Good,
                    KeyCode::Char('4') => break Rating::Easy,
                    KeyCode::Char('q') | KeyCode::Esc => {
                        *skipped = true;
                        return Ok(());
                    }
                    _ => {}
                }
            }
        };

        // ── Apply scheduling ───────────────────────────────────
        let now = Utc::now();
        {
            let card = &mut cards[i];

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

            if rating == Rating::Again {
                *again_count += 1;
            }
        }

        // Show brief feedback
        {
            let card = &cards[i];
            let next_in = (card.due - now).num_days();
            write!(
                stdout,
                "\r  → {} · next in {} day(s)\r\n",
                rating,
                next_in.max(1),
            )?;
            stdout.flush()?;
        }

        *reviewed += 1;
        i += 1;

        // Brief pause so the user can see the feedback
        std::thread::sleep(std::time::Duration::from_millis(600));
    }

    Ok(())
}
