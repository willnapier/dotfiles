use anyhow::Result;
use serde::Deserialize;

use crate::notation::CanonicalWork;

const OPEN_OPUS_BASE: &str = "https://api.openopus.org";

#[derive(Debug, Deserialize)]
struct ComposerSearchResponse {
    status: ComposerSearchStatus,
    composers: Option<Vec<Composer>>,
}

#[derive(Debug, Deserialize)]
struct ComposerSearchStatus {
    success: String,
}

#[derive(Debug, Deserialize)]
struct Composer {
    id: String,
    name: String,
    complete_name: String,
}

#[derive(Debug, Deserialize)]
struct WorkSearchResponse {
    status: WorkSearchStatus,
    works: Option<Vec<WorkResult>>,
}

#[derive(Debug, Deserialize)]
struct WorkSearchStatus {
    success: String,
}

#[derive(Debug, Deserialize)]
struct WorkResult {
    title: String,
    subtitle: Option<String>,
    genre: Option<String>,
}

/// Look up a work in the Open Opus API to get canonical notation.
/// Returns None if the work cannot be found or API is unavailable.
pub fn lookup_work(composer: &str, title: &str) -> Result<Option<CanonicalWork>> {
    // First, find the composer
    let composer_id = match find_composer(composer)? {
        Some(id) => id,
        None => return Ok(None),
    };

    // Then search for the work
    let work = find_work(&composer_id, title)?;

    Ok(work)
}

fn find_composer(name: &str) -> Result<Option<String>> {
    let search_term = extract_search_name(name);
    let url = format!(
        "{}/composer/list/search/{}.json",
        OPEN_OPUS_BASE,
        urlencoding::encode(&search_term)
    );

    let response: ComposerSearchResponse = reqwest::blocking::Client::new()
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()?
        .json()?;

    if response.status.success != "true" {
        return Ok(None);
    }

    let composers = response.composers.unwrap_or_default();

    // Find best match
    let search_lower = search_term.to_lowercase();
    for composer in &composers {
        if composer.name.to_lowercase().contains(&search_lower)
            || composer.complete_name.to_lowercase().contains(&search_lower)
        {
            return Ok(Some(composer.id.clone()));
        }
    }

    // Return first result if any
    Ok(composers.first().map(|c| c.id.clone()))
}

fn find_work(composer_id: &str, title: &str) -> Result<Option<CanonicalWork>> {
    let search_term = simplify_title(title);
    let url = format!(
        "{}/work/list/composer/{}/search/{}.json",
        OPEN_OPUS_BASE,
        composer_id,
        urlencoding::encode(&search_term)
    );

    let response: WorkSearchResponse = reqwest::blocking::Client::new()
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()?
        .json()?;

    if response.status.success != "true" {
        return Ok(None);
    }

    let works = response.works.unwrap_or_default();

    if works.is_empty() {
        return Ok(None);
    }

    // Use first matching work
    let work = &works[0];

    // Parse catalog info from the work title/subtitle
    let (catalogue, catalogue_number, key) = parse_work_info(&work.title, work.subtitle.as_deref());

    Ok(Some(CanonicalWork {
        composer_name: String::new(), // We don't need this from API
        catalogue,
        catalogue_number,
        key,
    }))
}

fn extract_search_name(full_name: &str) -> String {
    // Extract last name for search
    let parts: Vec<&str> = full_name.split_whitespace().collect();

    // Skip common prefixes like "van", "von", "de"
    let skip = ["van", "von", "de", "di", "da"];

    for part in parts.iter().rev() {
        let lower = part.to_lowercase();
        if !skip.contains(&lower.as_str()) {
            return part.to_string();
        }
    }

    full_name.to_string()
}

fn simplify_title(title: &str) -> String {
    // Remove catalog numbers and keep core title for search
    let title = title
        .replace("Op.", "")
        .replace("No.", "")
        .replace("BWV", "")
        .replace("K.", "")
        .replace("RV", "");

    // Keep first significant word
    title
        .split_whitespace()
        .filter(|w| w.len() > 3)
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_work_info(
    title: &str,
    subtitle: Option<&str>,
) -> (Option<String>, Option<String>, Option<String>) {
    let full_text = format!("{} {}", title, subtitle.unwrap_or(""));

    // Extract opus/catalog
    let catalogue_patterns = [
        (r"(?i)op\.?\s*(\d+)", "Op"),
        (r"(?i)bwv\s*(\d+)", "BWV"),
        (r"(?i)k\.?\s*(\d+)", "K"),
        (r"(?i)rv\s*(\d+)", "RV"),
        (r"(?i)d\.?\s*(\d+)", "D"),
    ];

    let mut catalogue = None;
    let mut catalogue_number = None;

    for (pattern, cat_name) in catalogue_patterns {
        if let Ok(re) = regex::Regex::new(pattern) {
            if let Some(caps) = re.captures(&full_text) {
                catalogue = Some(cat_name.to_string());
                catalogue_number = caps.get(1).map(|m| m.as_str().to_string());
                break;
            }
        }
    }

    // Extract key
    let key_re = regex::Regex::new(
        r"(?i)in\s+([A-G](?:[-\s]?(?:sharp|flat|#|b))?)\s*(major|minor|maj|min)?",
    )
    .ok();

    let key = key_re.and_then(|re| {
        re.captures(&full_text).map(|caps| {
            let note = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let mode = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            format!("{} {}", note, mode).trim().to_string()
        })
    });

    (catalogue, catalogue_number, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_search_name() {
        assert_eq!(extract_search_name("Ludwig van Beethoven"), "Beethoven");
        assert_eq!(extract_search_name("Johann Sebastian Bach"), "Bach");
        assert_eq!(extract_search_name("Arcangelo Corelli"), "Corelli");
    }

    #[test]
    fn test_simplify_title() {
        assert_eq!(simplify_title("Sonata Op. 27 No. 2"), "Sonata");
        assert_eq!(simplify_title("Concerto Grosso Op. 6 No. 4"), "Concerto Grosso");
    }
}
