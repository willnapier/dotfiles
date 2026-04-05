use anyhow::{bail, Result};
use std::io::{self, BufRead, IsTerminal, Write};

use crate::card::Card;

pub fn run(deck: &str, question: Option<&str>, answer: Option<&str>) -> Result<()> {
    let (q, a) = match (question, answer) {
        (Some(q), Some(a)) => (q.to_string(), a.to_string()),
        (Some(q), None) => {
            // Have question, prompt for answer
            let a = prompt_field("Answer")?;
            (q.to_string(), a)
        }
        (None, _) => {
            if io::stdin().is_terminal() {
                // Interactive: prompt for both
                let q = prompt_field("Question")?;
                let a = prompt_field("Answer")?;
                (q, a)
            } else {
                // Pipe mode: read Q: A: from stdin
                read_from_stdin()?
            }
        }
    };

    let q = q.trim().to_string();
    let a = a.trim().to_string();

    if q.is_empty() {
        bail!("question cannot be empty");
    }
    if a.is_empty() {
        bail!("answer cannot be empty");
    }

    let card = Card::create(deck, &q, &a)?;
    println!("Added: {} ({})", card.id, card.path.display());
    Ok(())
}

fn prompt_field(label: &str) -> Result<String> {
    print!("{}: ", label);
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

/// Read Q: and A: from stdin (pipe mode).
/// Accepts:
///   Q: ...\nA: ...
///   or just two lines (first = Q, second = A)
fn read_from_stdin() -> Result<(String, String)> {
    let stdin = io::stdin();
    let mut lines: Vec<String> = stdin.lock().lines().collect::<Result<Vec<_>, _>>()?;

    // Strip empty lines
    lines.retain(|l| !l.trim().is_empty());

    let mut question: Option<String> = None;
    let mut answer_lines: Vec<String> = Vec::new();
    let mut in_answer = false;

    for line in &lines {
        if let Some(q) = line.strip_prefix("Q: ") {
            question = Some(q.to_string());
            in_answer = false;
        } else if let Some(a) = line.strip_prefix("A: ") {
            answer_lines.clear();
            answer_lines.push(a.to_string());
            in_answer = true;
        } else if in_answer {
            answer_lines.push(line.to_string());
        }
    }

    // Fallback: if no Q:/A: prefixes, treat first line as Q and rest as A
    if question.is_none() && !lines.is_empty() {
        question = Some(lines[0].clone());
        if lines.len() > 1 {
            answer_lines = lines[1..].to_vec();
        }
    }

    let q = question.ok_or_else(|| anyhow::anyhow!("no question found in input"))?;
    if answer_lines.is_empty() {
        bail!("no answer found in input");
    }

    Ok((q, answer_lines.join("\n")))
}
