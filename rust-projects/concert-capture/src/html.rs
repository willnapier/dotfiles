use anyhow::{Context, Result};
use chrono::NaiveDate;
use regex::Regex;
use scraper::{Html, Selector};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Venue {
    WigmoreHall,
    SouthbankCentre,
    KingsPlace,
    Barbican,
    Unknown,
}

#[derive(Debug)]
pub struct Concert {
    pub date: NaiveDate,
    pub performers: Vec<String>,
    pub works: Vec<Work>,
    pub venue: Venue,
}

#[derive(Debug)]
pub struct Work {
    pub composer: String,
    pub title: String,
}

pub fn parse_concert(html: &str) -> Result<Concert> {
    let venue = detect_venue(html);
    let document = Html::parse_document(html);

    let date = extract_date(html, venue)?;
    let performers = extract_performers(&document, venue);
    let works = extract_works(&document, venue);

    Ok(Concert {
        date,
        performers,
        works,
        venue,
    })
}

fn detect_venue(html: &str) -> Venue {
    if html.contains("wigmore-hall.org.uk") {
        Venue::WigmoreHall
    } else if html.contains("southbankcentre.co.uk") {
        Venue::SouthbankCentre
    } else if html.contains("kingsplace.co.uk") {
        Venue::KingsPlace
    } else if html.contains("barbican.org.uk") {
        Venue::Barbican
    } else {
        Venue::Unknown
    }
}

fn extract_date(html: &str, venue: Venue) -> Result<NaiveDate> {
    match venue {
        Venue::WigmoreHall => extract_date_wigmore(html),
        Venue::SouthbankCentre => extract_date_southbank(html),
        Venue::KingsPlace => extract_date_kingsplace(html),
        Venue::Barbican => extract_date_barbican(html),
        Venue::Unknown => extract_date_fallback(html),
    }
}

fn extract_date_barbican(html: &str) -> Result<NaiveDate> {
    // Barbican URL pattern: barbican.org.uk/whats-on/2026/event/...
    // Also has dates like "Fri 6 Feb 2026"
    extract_date_fallback(html)
}

fn extract_date_kingsplace(html: &str) -> Result<NaiveDate> {
    // Kings Place uses schema.org JSON-LD: "startDate":"2026-03-20T19:30:00+00:00"
    let schema_re = Regex::new(r#""startDate"\s*:\s*"(\d{4})-(\d{2})-(\d{2})T"#)?;
    if let Some(caps) = schema_re.captures(html) {
        let year: i32 = caps[1].parse()?;
        let month: u32 = caps[2].parse()?;
        let day: u32 = caps[3].parse()?;
        return NaiveDate::from_ymd_opt(year, month, day)
            .context("Invalid date from Kings Place schema");
    }

    extract_date_fallback(html)
}

fn extract_date_wigmore(html: &str) -> Result<NaiveDate> {
    // URL pattern: url: https://www.wigmore-hall.org.uk/whats-on/YYYYMMDDHHMM
    let re = Regex::new(r"wigmore-hall\.org\.uk/whats-on/(\d{12})")?;

    if let Some(caps) = re.captures(html) {
        let datetime_str = &caps[1];
        let date_str = &datetime_str[0..8];
        return NaiveDate::parse_from_str(date_str, "%Y%m%d")
            .context("Failed to parse date from Wigmore URL");
    }

    extract_date_fallback(html)
}

fn extract_date_southbank(html: &str) -> Result<NaiveDate> {
    // Try schema.org JSON-LD first: "startDate":"2026-01-27T19:00:00+00:00"
    let schema_re = Regex::new(r#""startDate"\s*:\s*"(\d{4})-(\d{2})-(\d{2})T"#)?;
    if let Some(caps) = schema_re.captures(html) {
        let year: i32 = caps[1].parse()?;
        let month: u32 = caps[2].parse()?;
        let day: u32 = caps[3].parse()?;
        return NaiveDate::from_ymd_opt(year, month, day)
            .context("Invalid date from Southbank schema");
    }

    // Fallback: "Tue 27 Jan 2026" or "27 January 2026"
    extract_date_fallback(html)
}

fn extract_date_fallback(html: &str) -> Result<NaiveDate> {
    // Pattern: "27 Jan 2026" or "27 January 2026"
    let date_re = Regex::new(r"(\d{1,2})\s+(Jan(?:uary)?|Feb(?:ruary)?|Mar(?:ch)?|Apr(?:il)?|May|Jun(?:e)?|Jul(?:y)?|Aug(?:ust)?|Sep(?:tember)?|Oct(?:ober)?|Nov(?:ember)?|Dec(?:ember)?)\s+(\d{4})")?;

    if let Some(caps) = date_re.captures(html) {
        let day: u32 = caps[1].parse()?;
        let month = month_to_num(&caps[2])?;
        let year: i32 = caps[3].parse()?;
        return NaiveDate::from_ymd_opt(year, month, day)
            .context("Invalid date components");
    }

    anyhow::bail!("Could not extract concert date from HTML")
}

fn month_to_num(month: &str) -> Result<u32> {
    match month.to_lowercase().as_str() {
        "jan" | "january" => Ok(1),
        "feb" | "february" => Ok(2),
        "mar" | "march" => Ok(3),
        "apr" | "april" => Ok(4),
        "may" => Ok(5),
        "jun" | "june" => Ok(6),
        "jul" | "july" => Ok(7),
        "aug" | "august" => Ok(8),
        "sep" | "september" => Ok(9),
        "oct" | "october" => Ok(10),
        "nov" | "november" => Ok(11),
        "dec" | "december" => Ok(12),
        _ => anyhow::bail!("Unknown month: {}", month),
    }
}

fn extract_performers(document: &Html, venue: Venue) -> Vec<String> {
    match venue {
        Venue::WigmoreHall => extract_performers_wigmore(document),
        Venue::SouthbankCentre => extract_performers_southbank(document),
        Venue::KingsPlace => extract_performers_kingsplace(document),
        Venue::Barbican => extract_performers_barbican(document),
        Venue::Unknown => extract_performers_wigmore(document), // try Wigmore as default
    }
}

fn extract_performers_barbican(document: &Html) -> Vec<String> {
    // Barbican uses .label-value-list for both Programme and Performers
    // Performers have roles (conductor, violin, etc.) or are ensembles (Orchestra, Chorus)
    // Programme items have work titles in <em> tags
    let list_selector = Selector::parse(".label-value-list").unwrap();
    let li_selector = Selector::parse("li").unwrap();
    let label_selector = Selector::parse(".label-value-list__label").unwrap();
    let value_selector = Selector::parse(".label-value-list__value").unwrap();
    let em_selector = Selector::parse("em").unwrap();

    let mut performers = Vec::new();

    for list in document.select(&list_selector) {
        for li in list.select(&li_selector) {
            let name = li
                .select(&label_selector)
                .next()
                .map(|el| el.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            let value_el = li.select(&value_selector).next();

            // Check if value contains <em> - if so, it's a work title, not a performer role
            let has_em = value_el
                .map(|v| v.select(&em_selector).next().is_some())
                .unwrap_or(false);

            if has_em {
                // This is a Programme entry (composer + work), skip
                continue;
            }

            let role = value_el
                .map(|el| el.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            if !name.is_empty() {
                if !role.is_empty() {
                    performers.push(format!("{} {}", name, role));
                } else {
                    performers.push(name);
                }
            }
        }
    }

    // Deduplicate
    performers.sort();
    performers.dedup();
    performers
}

fn extract_performers_kingsplace(document: &Html) -> Vec<String> {
    // Kings Place often has performer info in "About [Performer]" sections
    // or in the event title. Try multiple approaches.
    let mut performers = Vec::new();

    // Look for "About X" headings which indicate performer sections
    let heading_selector = Selector::parse("h2, h3, h4").unwrap();
    for el in document.select(&heading_selector) {
        let text = el.text().collect::<String>();
        if text.starts_with("About ") {
            let performer = text.trim_start_matches("About ").trim().to_string();
            if !performer.is_empty() && !performers.contains(&performer) {
                performers.push(performer);
            }
        }
    }

    performers
}

fn extract_performers_wigmore(document: &Html) -> Vec<String> {
    let selector = Selector::parse(".performance-title").unwrap();
    let mut performers = Vec::new();

    for el in document.select(&selector) {
        let text = el.text().collect::<String>();
        let text = text.trim();

        if text.is_empty() {
            continue;
        }

        // Split on semicolons (Wigmore uses "Performer1; Performer2; ...")
        for part in text.split(';') {
            let cleaned = part.trim();
            if !cleaned.is_empty() {
                performers.push(cleaned.to_string());
            }
        }
    }

    performers
}

fn extract_performers_southbank(document: &Html) -> Vec<String> {
    let item_selector = Selector::parse(".c-event-performers__item").unwrap();
    let name_selector = Selector::parse(".c-event-performers__name").unwrap();
    let role_selector = Selector::parse(".c-event-performers__role").unwrap();

    let mut performers = Vec::new();

    for item in document.select(&item_selector) {
        let name = item
            .select(&name_selector)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        let role = item
            .select(&role_selector)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        if !name.is_empty() {
            if !role.is_empty() {
                performers.push(format!("{} {}", name, role));
            } else {
                performers.push(name);
            }
        }
    }

    performers
}

fn extract_works(document: &Html, venue: Venue) -> Vec<Work> {
    match venue {
        Venue::WigmoreHall => extract_works_wigmore(document),
        Venue::SouthbankCentre => extract_works_southbank(document),
        Venue::KingsPlace => extract_works_kingsplace(document),
        Venue::Barbican => extract_works_barbican(document),
        Venue::Unknown => extract_works_wigmore(document),
    }
}

fn extract_works_barbican(document: &Html) -> Vec<Work> {
    // Barbican uses .label-value-list with composer in __label and work in __value (with <em>)
    let list_selector = Selector::parse(".label-value-list").unwrap();
    let li_selector = Selector::parse("li").unwrap();
    let label_selector = Selector::parse(".label-value-list__label").unwrap();
    let value_selector = Selector::parse(".label-value-list__value").unwrap();
    let em_selector = Selector::parse("em").unwrap();

    let mut works = Vec::new();

    for list in document.select(&list_selector) {
        for li in list.select(&li_selector) {
            let composer = li
                .select(&label_selector)
                .next()
                .map(|el| el.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            // Work titles are in <em> tags within the value
            let title = li
                .select(&value_selector)
                .next()
                .and_then(|val| {
                    val.select(&em_selector)
                        .next()
                        .map(|el| el.text().collect::<String>().trim().to_string())
                })
                .unwrap_or_default();

            // Only add if we have both composer and title (indicates Programme, not Performers)
            if !composer.is_empty() && !title.is_empty() {
                works.push(Work { composer, title });
            }
        }
    }

    works
}

fn extract_works_kingsplace(document: &Html) -> Vec<Work> {
    // Kings Place uses a table with class "nvtable"
    // <th>Composer</th> <td><em>Work Title</em></td>
    let table_selector = Selector::parse("table.nvtable").unwrap();
    let row_selector = Selector::parse("tr").unwrap();
    let composer_selector = Selector::parse("th").unwrap();
    let title_selector = Selector::parse("td").unwrap();

    let mut works = Vec::new();

    for table in document.select(&table_selector) {
        for row in table.select(&row_selector) {
            let composer = row
                .select(&composer_selector)
                .next()
                .map(|el| el.text().collect::<String>().trim().to_string());

            let title = row
                .select(&title_selector)
                .next()
                .map(|el| el.text().collect::<String>().trim().to_string());

            if let (Some(composer), Some(title)) = (composer, title) {
                if !composer.is_empty() && !title.is_empty() {
                    works.push(Work { composer, title });
                }
            }
        }
    }

    works
}

fn extract_works_wigmore(document: &Html) -> Vec<Work> {
    let item_selector = Selector::parse(".repertoire-work-item").unwrap();
    let composer_selector = Selector::parse("a[href*='/artists/']").unwrap();
    let title_selector = Selector::parse(".rich-text.inline.bold").unwrap();
    let title_fallback = Selector::parse(".type-style-6").unwrap();
    let composer_fallback = Selector::parse(".type-style-4").unwrap();
    let cycle_item_selector = Selector::parse(".cycle-item").unwrap();

    // Regex to detect catalog numbers (HWV, BWV, Op., K., RV, D., etc.)
    let catalog_re = regex::Regex::new(r"(?i)(HWV|BWV|Op\.?\s*\d|K\.?\s*\d|RV\s*\d|D\.?\s*\d|S\d)").unwrap();

    let mut works: Vec<Work> = Vec::new();
    let mut seen_titles: std::collections::HashSet<String> = std::collections::HashSet::new();

    for item in document.select(&item_selector) {
        let composer = item
            .select(&composer_selector)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .or_else(|| {
                item.select(&composer_fallback)
                    .next()
                    .map(|el| el.text().collect::<String>().trim().to_string())
            });

        // Get the FIRST title directly under the item (not nested in cycle-item)
        // This is the main work title
        let main_title = item
            .select(&title_selector)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .or_else(|| {
                item.select(&title_fallback)
                    .next()
                    .map(|el| el.text().collect::<String>().trim().to_string())
            });

        if let (Some(ref comp), Some(ref title)) = (&composer, &main_title) {
            if !comp.is_empty() && !title.is_empty() && !seen_titles.contains(title) {
                seen_titles.insert(title.clone());
                works.push(Work {
                    composer: comp.clone(),
                    title: title.clone(),
                });
            }
        }

        // Also check for nested cycle-items (arias, movements with their own catalog numbers)
        for cycle in item.select(&cycle_item_selector) {
            if let Some(nested_title_el) = cycle.select(&title_selector).next() {
                let nested_title = nested_title_el.text().collect::<String>().trim().to_string();

                // Only include if it has its own catalog number AND is different from main title
                if catalog_re.is_match(&nested_title)
                    && !nested_title.is_empty()
                    && !seen_titles.contains(&nested_title)
                {
                    seen_titles.insert(nested_title.clone());
                    // Use parent composer if available
                    if let Some(ref comp) = composer {
                        if !comp.is_empty() {
                            works.push(Work {
                                composer: comp.clone(),
                                title: nested_title,
                            });
                        }
                    }
                }
            }
        }
    }

    works
}

fn extract_works_southbank(document: &Html) -> Vec<Work> {
    let item_selector = Selector::parse(".c-event-repertoire__item").unwrap();
    let composer_selector = Selector::parse(".c-event-repertoire__composer").unwrap();
    // Note: Southbank uses .c-event-performers__work for work titles (inconsistent naming)
    let title_selector = Selector::parse(".c-event-performers__work").unwrap();

    document
        .select(&item_selector)
        .filter_map(|item| {
            let composer = item
                .select(&composer_selector)
                .next()
                .map(|el| el.text().collect::<String>().trim().to_string())?;

            let title = item
                .select(&title_selector)
                .next()
                .map(|el| el.text().collect::<String>().trim().to_string())?;

            if composer.is_empty() || title.is_empty() {
                return None;
            }

            Some(Work { composer, title })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_venue() {
        assert_eq!(detect_venue("url: https://www.wigmore-hall.org.uk/whats-on/123"), Venue::WigmoreHall);
        assert_eq!(detect_venue("url: https://www.southbankcentre.co.uk/whats-on/test"), Venue::SouthbankCentre);
        assert_eq!(detect_venue("some random html"), Venue::Unknown);
    }

    #[test]
    fn test_extract_date_wigmore() {
        let html = r#"url: https://www.wigmore-hall.org.uk/whats-on/202602041930"#;
        let date = extract_date_wigmore(html).unwrap();
        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 2, 4).unwrap());
    }

    #[test]
    fn test_extract_date_southbank() {
        let html = r#""startDate":"2026-01-27T19:00:00+00:00""#;
        let date = extract_date_southbank(html).unwrap();
        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 1, 27).unwrap());
    }

    #[test]
    fn test_month_to_num() {
        assert_eq!(month_to_num("January").unwrap(), 1);
        assert_eq!(month_to_num("Jan").unwrap(), 1);
        assert_eq!(month_to_num("december").unwrap(), 12);
    }
}
