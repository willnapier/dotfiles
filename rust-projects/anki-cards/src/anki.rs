use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::Card;

const ANKI_CONNECT_URL: &str = "http://localhost:8765";

fn anki_request(action: &str, params: Value) -> Result<Value> {
    let body = json!({
        "action": action,
        "version": 6,
        "params": params,
    });

    let response: Value = ureq::post(ANKI_CONNECT_URL)
        .send_json(&body)
        .context("Failed to connect to AnkiConnect — is Anki running with AnkiConnect installed?")?
        .into_json()
        .context("Invalid JSON response from AnkiConnect")?;

    if let Some(err) = response.get("error").and_then(|e| e.as_str()) {
        anyhow::bail!("AnkiConnect error: {}", err);
    }

    Ok(response)
}

pub fn create_deck(deck: &str) -> Result<()> {
    anki_request("createDeck", json!({ "deck": deck }))?;
    Ok(())
}

pub fn add_notes(deck: &str, cards: &[Card]) -> Result<usize> {
    let notes: Vec<Value> = cards
        .iter()
        .map(|c| {
            json!({
                "deckName": deck,
                "modelName": "Basic",
                "fields": {
                    "Front": c.front,
                    "Back": c.back,
                },
                "options": {
                    "allowDuplicate": false,
                },
            })
        })
        .collect();

    let response = anki_request("addNotes", json!({ "notes": notes }))?;

    // addNotes returns an array of note IDs (null for failures)
    let results = response
        .get("result")
        .and_then(|r| r.as_array())
        .context("Unexpected response format from addNotes")?;

    let added = results.iter().filter(|r| !r.is_null()).count();
    Ok(added)
}
