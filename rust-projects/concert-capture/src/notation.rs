use regex::Regex;

/// Canonical work info from Open Opus API
#[derive(Debug)]
pub struct CanonicalWork {
    pub composer_name: String,
    pub catalogue: Option<String>,
    pub catalogue_number: Option<String>,
    pub key: Option<String>,
}

/// Generate work notation following the music-work-notation-system spec.
/// Pattern: Composer-WorkType-Key-Catalog-Nickname
/// e.g., Corelli-ConcertoGrosso-D-Op6No1, Vivaldi-TrioSonata-Dmin-Op1No12-LaFollia
pub fn generate_notation(
    composer: &str,
    title: &str,
    canonical: Option<&CanonicalWork>,
) -> String {
    let composer_tag = composer_to_tag(composer);

    // Strip composition year from title (e.g., "(2010)" or "(1967)")
    let title = Regex::new(r"\s*\(\d{4}\)\s*")
        .map(|re| re.replace(title, "").to_string())
        .unwrap_or_else(|_| title.to_string());
    let title = title.trim();

    // Extract enrichment data from title
    let work_type = extract_work_type(title);
    let work_title = if work_type.is_none() { extract_work_title(title) } else { None };
    let key = extract_key_from_title(title);
    let nickname = extract_nickname(title);

    // Build notation parts
    let mut parts = vec![composer_tag];
    let has_work_identifier = work_type.is_some() || work_title.is_some();

    // Add work type if found, otherwise work title (for operas, etc.)
    if let Some(wt) = work_type {
        parts.push(wt);
    } else if let Some(wn) = work_title {
        parts.push(wn);
    }

    // Add key if found
    let has_key = key.is_some();
    if let Some(k) = key {
        parts.push(k);
    }

    // If we have canonical data from API, use it for catalog
    if let Some(canon) = canonical {
        if let (Some(cat), Some(num)) = (&canon.catalogue, &canon.catalogue_number) {
            parts.push(format!("{}{}", cat, num));
            // Add canonical key if we didn't extract one
            if !has_key {
                if let Some(canon_key) = &canon.key {
                    parts.push(key_to_tag(canon_key));
                }
            }
        }
    } else if let Some(catalog) = extract_catalog_from_title(title) {
        // Fallback: try to extract catalog info from title
        parts.push(catalog);
    } else if !has_work_identifier {
        // Last resort: use slugified title only if we have no other identifier
        parts.push(slug::slugify(title));
    }

    // Add nickname if found
    if let Some(nick) = nickname {
        parts.push(nick);
    }

    parts.join("-")
}

/// Extract musical key from title.
/// Handles: "in D", "D major", "D minor", "in F-sharp minor", "B-flat major", etc.
fn extract_key_from_title(title: &str) -> Option<String> {
    // Pattern for "in X" or "X major/minor"
    // Note order: sharps/flats before simple notes to avoid partial matches
    let key_re = Regex::new(r"(?i)\b(?:in\s+)?([A-G])(?:[- ]?(sharp|flat|#|b))?\s*(major|minor|maj|min)?\b").ok()?;

    if let Some(caps) = key_re.captures(title) {
        let note = caps.get(1)?.as_str().to_uppercase();
        let accidental = caps.get(2).map(|m| {
            match m.as_str().to_lowercase().as_str() {
                "sharp" | "#" => "#",
                "flat" | "b" => "b",
                _ => ""
            }
        }).unwrap_or("");
        let mode = caps.get(3).map(|m| {
            match m.as_str().to_lowercase().as_str() {
                "minor" | "min" => "min",
                "major" | "maj" => "maj",
                _ => ""
            }
        }).unwrap_or("");

        // Only return if we have at least note + accidental or note + mode
        if !accidental.is_empty() || !mode.is_empty() {
            return Some(format!("{}{}{}", note, accidental, mode));
        }
        // If just "in D" without major/minor, still useful
        if title.to_lowercase().contains(&format!("in {}", note.to_lowercase())) {
            return Some(note);
        }
    }

    None
}

/// Extract work title/name (for operas, oratorios, etc. where the name IS the title).
/// Used when no standard work type is found but there's a proper noun before the catalog.
fn extract_work_title(title: &str) -> Option<String> {
    // Remove catalog numbers and anything after them
    let title_clean = Regex::new(r"(?i)\s*(hwv|bwv|op\.?|k\.?|rv|d\.?|s|woo)\s*\d.*$")
        .ok()?
        .replace(title, "")
        .trim()
        .to_string();

    // If what remains looks like a title (starts with capital, reasonable length)
    if !title_clean.is_empty()
        && title_clean.len() < 50
        && title_clean.chars().next()?.is_uppercase()
    {
        // Don't return if it's just a work type we'd detect anyway
        let lower = title_clean.to_lowercase();
        let work_types = ["concerto", "sonata", "quartet", "trio", "symphony", "suite", "prelude", "fugue"];
        for wt in work_types {
            if lower.contains(wt) {
                return None;
            }
        }
        return Some(to_pascal_case_multi(&title_clean));
    }
    None
}

/// Extract work type from title.
/// Handles common classical music forms.
fn extract_work_type(title: &str) -> Option<String> {
    let types = [
        ("concerto grosso", "ConcertoGrosso"),
        ("piano concerto", "PianoConcerto"),
        ("violin concerto", "ViolinConcerto"),
        ("cello concerto", "CelloConcerto"),
        ("concerto", "Concerto"),
        ("string quartet", "StringQuartet"),
        ("piano quartet", "PianoQuartet"),
        ("quartet", "Quartet"),
        ("string trio", "StringTrio"),
        ("piano trio", "PianoTrio"),
        ("trio sonata", "TrioSonata"),
        ("trio", "Trio"),
        ("piano sonata", "PianoSonata"),
        ("violin sonata", "ViolinSonata"),
        ("cello sonata", "CelloSonata"),
        ("sonata", "Sonata"),
        ("symphony", "Symphony"),
        ("sinfonia", "Sinfonia"),
        ("serenade", "Serenade"),
        ("suite", "Suite"),
        ("partita", "Partita"),
        ("prelude", "Prelude"),
        ("fugue", "Fugue"),
        ("fantasia", "Fantasia"),
        ("fantasy", "Fantasy"),
        ("variations", "Variations"),
        ("nocturne", "Nocturne"),
        ("etude", "Etude"),
        ("ballade", "Ballade"),
        ("scherzo", "Scherzo"),
        ("impromptu", "Impromptu"),
        ("mazurka", "Mazurka"),
        ("polonaise", "Polonaise"),
        ("waltz", "Waltz"),
        ("rhapsody", "Rhapsody"),
        ("overture", "Overture"),
        ("mass", "Mass"),
        ("requiem", "Requiem"),
        ("cantata", "Cantata"),
        ("oratorio", "Oratorio"),
        ("motet", "Motet"),
        ("aria", "Aria"),
    ];

    let lower = title.to_lowercase();
    for (pattern, tag) in types {
        if lower.contains(pattern) {
            return Some(tag.to_string());
        }
    }

    None
}

/// Extract nickname or subtitle from title.
/// Handles: 'La follia', "Moonlight", (Pathétique), etc.
fn extract_nickname(title: &str) -> Option<String> {
    // Try single quotes first: 'La follia'
    let single_quote_re = Regex::new(r"'([^']+)'").ok()?;
    if let Some(caps) = single_quote_re.captures(title) {
        let nickname = caps.get(1)?.as_str();
        return Some(to_pascal_case_multi(nickname));
    }

    // Try double quotes: "Moonlight"
    let double_quote_re = Regex::new(r#""([^"]+)""#).ok()?;
    if let Some(caps) = double_quote_re.captures(title) {
        let nickname = caps.get(1)?.as_str();
        return Some(to_pascal_case_multi(nickname));
    }

    // Try parenthetical nicknames (but not years or catalog numbers)
    let paren_re = Regex::new(r"\(([A-Za-z][^)]*)\)").ok()?;
    for caps in paren_re.captures_iter(title) {
        if let Some(m) = caps.get(1) {
            let content = m.as_str();
            // Skip if it looks like a catalog number or year
            if content.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            if Regex::new(r"(?i)^(op|bwv|hwv|k|rv|d|s|woo|hob)\.?\s*\d").ok()?.is_match(content) {
                continue;
            }
            return Some(to_pascal_case_multi(content));
        }
    }

    None
}

/// Convert multi-word string to PascalCase, stripping punctuation
fn to_pascal_case_multi(s: &str) -> String {
    s.split_whitespace()
        .map(|word| {
            // Strip punctuation from word before converting
            let cleaned: String = word.chars().filter(|c| c.is_alphanumeric()).collect();
            to_pascal_case(&cleaned)
        })
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("")
}

/// Convert composer name to PascalCase tag.
/// Handles common classical music name conventions.
pub fn composer_to_tag(name: &str) -> String {
    // Strip arrangement/orchestration annotations
    let name = Regex::new(r"(?i)\s*\((?:arranged?|arr\.?|orch\.?|orchestrated?)[^)]*\)")
        .map(|re| re.replace(name, "").to_string())
        .unwrap_or_else(|_| name.to_string());
    let name = name.trim();

    // Known composer mappings
    let known: &[(&str, &str)] = &[
        ("johann sebastian bach", "JSBach"),
        ("j.s. bach", "JSBach"),
        ("j s bach", "JSBach"),
        ("bach", "Bach"),
        ("ludwig van beethoven", "Beethoven"),
        ("wolfgang amadeus mozart", "Mozart"),
        ("george frideric handel", "Handel"),
        ("george frederick handel", "Handel"),
        ("händel", "Handel"),
        ("antonio vivaldi", "Vivaldi"),
        ("arcangelo corelli", "Corelli"),
        ("franz schubert", "Schubert"),
        ("robert schumann", "Schumann"),
        ("clara schumann", "ClaraSchumann"),
        ("johannes brahms", "Brahms"),
        ("frédéric chopin", "Chopin"),
        ("frederic chopin", "Chopin"),
        ("franz liszt", "Liszt"),
        ("claude debussy", "Debussy"),
        ("maurice ravel", "Ravel"),
        ("sergei rachmaninoff", "Rachmaninoff"),
        ("sergei rachmaninov", "Rachmaninoff"),
        ("dmitri shostakovich", "Shostakovich"),
        ("pyotr ilyich tchaikovsky", "Tchaikovsky"),
        ("igor stravinsky", "Stravinsky"),
        ("béla bartók", "Bartok"),
        ("bela bartok", "Bartok"),
    ];

    let lower = name.to_lowercase();
    for (pattern, tag) in known {
        if lower.contains(pattern) {
            return tag.to_string();
        }
    }

    // Default: PascalCase the last name
    let parts: Vec<&str> = name.split_whitespace().collect();
    if let Some(last) = parts.last() {
        to_pascal_case(last)
    } else {
        to_pascal_case(name)
    }
}

/// Convert performer name to PascalCase tag for concert entry.
pub fn performer_tag(name: &str) -> String {
    // Remove role descriptions like "piano", "violin", "director", etc.
    let role_re = Regex::new(r"(?i)\s+(piano|violin|viola|cello|soprano|mezzo-soprano|alto|tenor|baritone|bass|conductor|director|guitar|flute|oboe|clarinet|bassoon|horn|trumpet|trombone|tuba|percussion|harp|organ|harpsichord)\s*$").unwrap();
    let cleaned = role_re.replace(name, "").to_string();

    // Handle ensemble names (keep as-is but PascalCase)
    if cleaned.contains("Quartet")
        || cleaned.contains("Orchestra")
        || cleaned.contains("Ensemble")
        || cleaned.contains("Concert")
        || cleaned.contains("Consort")
    {
        return cleaned
            .split_whitespace()
            .map(to_pascal_case)
            .collect::<Vec<_>>()
            .join("");
    }

    // Individual performers: FirstLast
    cleaned
        .split_whitespace()
        .map(to_pascal_case)
        .collect::<Vec<_>>()
        .join("")
}

fn to_pascal_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars.flat_map(|c| c.to_lowercase())).collect(),
    }
}

fn key_to_tag(key: &str) -> String {
    // Convert key descriptions to compact form
    // "C-sharp minor" -> "C#min", "D major" -> "Dmaj"
    let key = key
        .replace("sharp", "#")
        .replace("flat", "b")
        .replace(" minor", "min")
        .replace(" major", "maj")
        .replace(" ", "");

    key.chars()
        .filter(|c| c.is_alphanumeric() || *c == '#' || *c == 'b')
        .collect()
}

fn extract_catalog_from_title(title: &str) -> Option<String> {
    // Try specific patterns in order of specificity

    // Handel HWV catalog (must come before Hob to avoid confusion)
    let hwv_re = Regex::new(r"(?i)hwv\s*(\d+)").ok()?;
    if let Some(caps) = hwv_re.captures(title) {
        return Some(format!("HWV{}", &caps[1]));
    }

    // Haydn Hoboken catalog: HXVI/49 or Hob. XVI:52
    // Must have explicit Hob or H followed by Roman numerals
    let hob_re = Regex::new(r"(?i)(?:hob\.?\s*)?H([XVI]+)[:/]?(\d+)").ok()?;
    if let Some(caps) = hob_re.captures(title) {
        let category = caps.get(1).map(|m| m.as_str().to_uppercase()).unwrap_or_default();
        let num = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        if !category.is_empty() && !num.is_empty() {
            return Some(format!("Hob{}{}", category, num));
        }
    }

    // Op. X No. Y format
    let op_no_re = Regex::new(r"(?i)op\.?\s*(\d+)\s*no\.?\s*(\d+)").ok()?;
    if let Some(caps) = op_no_re.captures(title) {
        let op = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let no = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        return Some(format!("Op{}No{}", op, no));
    }

    // Simple Op. X format
    let op_re = Regex::new(r"(?i)op\.?\s*(\d+)").ok()?;
    if let Some(caps) = op_re.captures(title) {
        let op = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        return Some(format!("Op{}", op));
    }

    // BWV (handles BWV 846, BWV.547, BWV-123)
    let bwv_re = Regex::new(r"(?i)bwv[.\s-]*(\d+)").ok()?;
    if let Some(caps) = bwv_re.captures(title) {
        return Some(format!("BWV{}", &caps[1]));
    }

    // Mozart K numbers
    let k_re = Regex::new(r"(?i)k\.?\s*(\d+)").ok()?;
    if let Some(caps) = k_re.captures(title) {
        return Some(format!("K{}", &caps[1]));
    }

    // Vivaldi RV
    let rv_re = Regex::new(r"(?i)rv\s*(\d+)").ok()?;
    if let Some(caps) = rv_re.captures(title) {
        return Some(format!("RV{}", &caps[1]));
    }

    // Schubert D numbers
    let d_re = Regex::new(r"(?i)\bD\.?\s*(\d+)").ok()?;
    if let Some(caps) = d_re.captures(title) {
        return Some(format!("D{}", &caps[1]));
    }

    // WoO
    let woo_re = Regex::new(r"(?i)woo\s*(\d+)").ok()?;
    if let Some(caps) = woo_re.captures(title) {
        return Some(format!("WoO{}", &caps[1]));
    }

    // Liszt S numbers (handles "S163", "S 163", "S. 163")
    let s_re = Regex::new(r"(?i)\bS\.?\s*(\d+)").ok()?;
    if let Some(caps) = s_re.captures(title) {
        return Some(format!("S{}", &caps[1]));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_composer_to_tag() {
        assert_eq!(composer_to_tag("Johann Sebastian Bach"), "JSBach");
        assert_eq!(composer_to_tag("Ludwig van Beethoven"), "Beethoven");
        assert_eq!(composer_to_tag("Arcangelo Corelli"), "Corelli");
    }

    #[test]
    fn test_performer_tag() {
        assert_eq!(performer_tag("Harry Bicket director"), "HarryBicket");
        assert_eq!(performer_tag("Kate Lindsey mezzo-soprano"), "KateLindsey");
        assert_eq!(performer_tag("The English Concert"), "TheEnglishConcert");
        assert_eq!(performer_tag("Jerusalem Quartet"), "JerusalemQuartet");
    }

    #[test]
    fn test_extract_catalog() {
        assert_eq!(extract_catalog_from_title("Sonata Op. 27 No. 2"), Some("Op27No2".to_string()));
        assert_eq!(extract_catalog_from_title("Well-Tempered Clavier BWV 846"), Some("BWV846".to_string()));
        assert_eq!(extract_catalog_from_title("Gloria RV 589"), Some("RV589".to_string()));
    }

    #[test]
    fn test_key_to_tag() {
        assert_eq!(key_to_tag("C-sharp minor"), "C#min");
        assert_eq!(key_to_tag("D major"), "Dmaj");
    }
}
