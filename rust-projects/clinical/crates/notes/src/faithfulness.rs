//! Multi-level faithfulness filter for clinical note generation.
//!
//! Inspired by MLIR: each layer handles what the one above couldn't,
//! cheapest first. Layer 1 (string match) → Layer 2 (NLP structural) →
//! Layer 3 (sentence embeddings). Layer 4 (LLM judge) is deferred.

use anyhow::Result;
use regex::Regex;
use rust_stemmers::{Algorithm, Stemmer};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroundingLevel {
    /// Traceable to observation or client context.
    Grounded,
    /// Not clearly traceable — flag for human review.
    Uncertain,
    /// Contains fabricated content not in any source material.
    Ungrounded,
}

#[derive(Debug, Clone)]
pub struct SentenceAssessment {
    pub sentence: String,
    pub assessed_by_layer: u8,
    pub level: GroundingLevel,
    pub reason: String,
    pub best_match: Option<String>,
    pub score: f64,
}

#[derive(Debug)]
pub struct FaithfulnessResult {
    pub assessments: Vec<SentenceAssessment>,
}

impl FaithfulnessResult {
    /// Hard failures: sentences assessed as Ungrounded by Layer 1.
    /// These should trigger regeneration.
    pub fn hard_failures(&self) -> Vec<&SentenceAssessment> {
        self.assessments
            .iter()
            .filter(|a| a.level == GroundingLevel::Ungrounded && a.assessed_by_layer == 1)
            .collect()
    }

    /// Soft flags: sentences assessed as Uncertain or Ungrounded by Layers 2-3.
    /// These should be highlighted for human review.
    pub fn soft_flags(&self) -> Vec<&SentenceAssessment> {
        self.assessments
            .iter()
            .filter(|a| a.level != GroundingLevel::Grounded && a.assessed_by_layer > 1)
            .collect()
    }

    /// True if there are no hard failures.
    pub fn passed_hard(&self) -> bool {
        self.hard_failures().is_empty()
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct FaithfulnessConfig {
    pub min_phrase_words: usize,
    pub overlap_threshold: f64,
    pub embedding_threshold: f64,
    pub embedding_model: String,
    pub embedding_endpoint: Option<String>,
}

impl Default for FaithfulnessConfig {
    fn default() -> Self {
        Self {
            min_phrase_words: 4,
            overlap_threshold: 0.35,
            embedding_threshold: 0.55,
            embedding_model: "nomic-embed-text".to_string(),
            embedding_endpoint: None,
        }
    }
}

/// Load faithfulness config from config.toml, falling back to defaults.
pub fn load_config() -> FaithfulnessConfig {
    let mut config = FaithfulnessConfig::default();

    let config_path = dirs::config_dir()
        .map(|d| d.join("clinical-product/config.toml"))
        .unwrap_or_default();

    if let Ok(content) = std::fs::read_to_string(&config_path) {
        if let Ok(table) = content.parse::<toml::Table>() {
            if let Some(faith) = table.get("faithfulness").and_then(|v| v.as_table()) {
                if let Some(v) = faith.get("min_phrase_words").and_then(|v| v.as_integer()) {
                    config.min_phrase_words = v as usize;
                }
                if let Some(v) = faith.get("overlap_threshold").and_then(|v| v.as_float()) {
                    config.overlap_threshold = v;
                }
                if let Some(v) = faith.get("embedding_threshold").and_then(|v| v.as_float()) {
                    config.embedding_threshold = v;
                }
                if let Some(v) = faith.get("embedding_model").and_then(|v| v.as_str()) {
                    config.embedding_model = v.to_string();
                }
                if let Some(v) = faith.get("embedding_endpoint").and_then(|v| v.as_str()) {
                    config.embedding_endpoint = Some(v.to_string());
                }
            }
        }
    }

    config
}

// ---------------------------------------------------------------------------
// Sentence extraction
// ---------------------------------------------------------------------------

/// Extract sentences from the note's narrative body and formulation.
/// Skips: ### date header, brief **Risk** lines, blank lines.
pub fn extract_checkable_sentences(note: &str) -> Vec<String> {
    let mut checkable_text = String::new();
    let mut in_risk = false;

    for line in note.lines() {
        let trimmed = line.trim();

        // Skip session header
        if trimmed.starts_with("### ") {
            continue;
        }

        // Handle **Risk** line — skip if brief (< 15 words)
        if trimmed.starts_with("**Risk**:") {
            let risk_content = trimmed.trim_start_matches("**Risk**:").trim();
            if risk_content.split_whitespace().count() < 15 {
                in_risk = false;
                continue;
            }
            // Long risk section — include it
            checkable_text.push_str(risk_content);
            checkable_text.push(' ');
            in_risk = true;
            continue;
        }

        // Handle **Formulation** line — include content after the prefix
        if trimmed.starts_with("**Formulation**:") {
            in_risk = false;
            let form_content = trimmed.trim_start_matches("**Formulation**:").trim();
            if !form_content.is_empty() {
                checkable_text.push_str(form_content);
                checkable_text.push(' ');
            }
            continue;
        }

        // Blank line resets risk continuation
        if trimmed.is_empty() {
            in_risk = false;
            continue;
        }

        // If we were in a multi-line risk section that was long, continue it
        if in_risk {
            checkable_text.push_str(trimmed);
            checkable_text.push(' ');
            continue;
        }

        // Regular narrative line
        checkable_text.push_str(trimmed);
        checkable_text.push(' ');
    }

    split_sentences(&checkable_text)
}

/// Split observation text into sentences.
pub fn extract_observation_sentences(observation: &str) -> Vec<String> {
    split_sentences(observation)
}

/// Split text into sentences. Uses a simple heuristic: split on
/// `. ` / `! ` / `? ` followed by a capital letter, but not after
/// common abbreviations (Dr., Mr., Ms., Mrs., e.g., i.e.).
fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        current.push(chars[i]);

        // Check for sentence boundary: punctuation + space + uppercase
        if (chars[i] == '.' || chars[i] == '!' || chars[i] == '?')
            && i + 2 < len
            && chars[i + 1].is_whitespace()
            && chars[i + 2].is_uppercase()
        {
            // Check for abbreviations ending at this period
            let before = current.trim_end_matches('.');
            let last_word = before.split_whitespace().last().unwrap_or("");
            let abbrevs = ["Dr", "Mr", "Ms", "Mrs", "e.g", "i.e", "etc", "vs"];
            if chars[i] == '.' && abbrevs.iter().any(|a| last_word.ends_with(a)) {
                i += 1;
                continue;
            }
            // This is a real sentence boundary
            let s = current.trim().to_string();
            if s.split_whitespace().count() >= 5 {
                sentences.push(s);
            }
            current = String::new();
            // Skip the whitespace
            i += 1;
            while i < len && chars[i].is_whitespace() {
                i += 1;
            }
            continue;
        }
        i += 1;
    }

    // Don't forget the last sentence
    let s = current.trim().to_string();
    if s.split_whitespace().count() >= 5 {
        sentences.push(s);
    }

    sentences
}

// ---------------------------------------------------------------------------
// Normalisation helpers
// ---------------------------------------------------------------------------

fn normalize(text: &str) -> String {
    text.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Extract quoted strings (text between double quotes) from a sentence.
fn extract_quotes(text: &str) -> Vec<String> {
    let re = Regex::new(r#""([^"]+)""#).unwrap();
    re.captures_iter(text)
        .map(|c| c[1].to_string())
        .collect()
}

/// Generate word n-grams of a given size from text.
fn ngrams(text: &str, n: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < n {
        return vec![];
    }
    words
        .windows(n)
        .map(|w| w.join(" "))
        .collect()
}

// ---------------------------------------------------------------------------
// ACT clinical terms allow-list
// ---------------------------------------------------------------------------

/// Standard ACT/CBS vocabulary that appears in every well-formed note.
/// These should not trigger Layer 1 fabrication flags.
const CLINICAL_ALLOWLIST: &[&str] = &[
    "defusion",
    "cognitive defusion",
    "acceptance",
    "willingness",
    "present-moment awareness",
    "present moment awareness",
    "self-as-context",
    "self as context",
    "values",
    "committed action",
    "psychological flexibility",
    "experiential avoidance",
    "cognitive fusion",
    "values clarification",
    "functional analysis",
    "relational frame",
    "verbal behaviour",
    "verbal behavior",
    "no immediate concerns noted",
    "no immediate concerns",
    "therapeutic relationship",
];

fn is_clinical_term(phrase: &str) -> bool {
    let lower = phrase.to_lowercase();
    CLINICAL_ALLOWLIST.iter().any(|t| lower.contains(t))
}

// ---------------------------------------------------------------------------
// Layer 1: String match
// ---------------------------------------------------------------------------

/// Layer 1: Check for fabricated verbatim phrases.
///
/// Extracts significant phrases from each note sentence and checks whether
/// they appear in the observation or client context.
pub fn layer1_string_match(
    note_sentences: &[String],
    observation: &str,
    client_context: &str,
    config: &FaithfulnessConfig,
) -> Vec<Option<SentenceAssessment>> {
    let norm_obs = normalize(observation);
    let norm_ctx = normalize(client_context);
    let source = format!("{} {}", norm_obs, norm_ctx);

    note_sentences
        .iter()
        .map(|sentence| {
            let norm_sent = normalize(sentence);

            // Check quoted text first — fabricated quotes are hard failures
            let quotes = extract_quotes(sentence);
            for quote in &quotes {
                let norm_quote = normalize(quote);
                if norm_quote.split_whitespace().count() >= 3
                    && !source.contains(&norm_quote)
                    && !is_clinical_term(&norm_quote)
                {
                    return Some(SentenceAssessment {
                        sentence: sentence.clone(),
                        assessed_by_layer: 1,
                        level: GroundingLevel::Ungrounded,
                        reason: format!(
                            "Fabricated quote not in source: \"{}\"",
                            quote
                        ),
                        best_match: None,
                        score: 0.0,
                    });
                }
            }

            // Check n-grams for significant phrases not in source
            let max_n = norm_sent.split_whitespace().count();

            for n in (config.min_phrase_words..=max_n).rev() {
                for gram in ngrams(&norm_sent, n) {
                    if is_clinical_term(&gram) {
                        continue;
                    }
                    if source.contains(&gram) {
                        // Found a grounding match at this size — sentence is grounded
                        return Some(SentenceAssessment {
                            sentence: sentence.clone(),
                            assessed_by_layer: 1,
                            level: GroundingLevel::Grounded,
                            reason: format!("Verbatim match: \"{}\"", gram),
                            best_match: Some(gram),
                            score: 1.0,
                        });
                    }
                }
            }

            // No verbatim match found — pass to Layer 2
            // (don't hard-fail — only fabricated quotes are hard failures at Layer 1)
            None
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Layer 2: NLP structural
// ---------------------------------------------------------------------------

static STOPWORDS: &[&str] = &[
    "a", "an", "the", "is", "was", "were", "are", "be", "been", "being",
    "in", "on", "at", "to", "for", "of", "and", "or", "but", "with",
    "that", "this", "it", "he", "she", "they", "her", "his", "their",
    "has", "had", "have", "do", "does", "did", "will", "would", "could",
    "should", "may", "might", "can", "shall", "not", "no", "by", "from",
    "as", "if", "when", "than", "so", "also", "very", "just", "about",
    "which", "what", "who", "how", "where", "there", "then", "more",
    // Clinical-specific stopwords (appear in every note, not informative)
    "client", "session", "noted", "discussed", "explored",
];

fn is_stopword(word: &str) -> bool {
    STOPWORDS.contains(&word.to_lowercase().as_str())
}

/// Stem a word using the Snowball English stemmer.
fn stem_word(word: &str) -> String {
    let stemmer = Stemmer::create(Algorithm::English);
    stemmer.stem(&word.to_lowercase()).to_string()
}

/// Compute stemmed content-word overlap between a sentence and source text.
/// Returns fraction of sentence's content words found (stemmed) in source.
fn stemmed_overlap(sentence: &str, source: &str) -> f64 {
    let source_stems: HashSet<String> = source
        .split_whitespace()
        .filter(|w| !is_stopword(w))
        .map(|w| stem_word(w))
        .collect();

    let sentence_words: Vec<String> = sentence
        .split_whitespace()
        .filter(|w| !is_stopword(w))
        .map(|w| stem_word(w))
        .collect();

    if sentence_words.is_empty() {
        return 1.0; // No content words — trivially grounded
    }

    let matched = sentence_words
        .iter()
        .filter(|w| source_stems.contains(w.as_str()))
        .count();

    matched as f64 / sentence_words.len() as f64
}

/// Extract likely named entities from text using simple heuristics:
/// capitalised multi-word sequences, quoted strings, specific numbers.
fn extract_entities(text: &str) -> Vec<String> {
    let mut entities = Vec::new();

    // Capitalised multi-word sequences (e.g., "Sarah Jones", "AXA Health")
    let cap_re = Regex::new(r"\b([A-Z][a-z]+(?:\s+[A-Z][a-z]+)+)\b").unwrap();
    for cap in cap_re.captures_iter(text) {
        entities.push(cap[1].to_string());
    }

    // Single capitalised words that aren't sentence-initial
    // (heuristic: skip first word of each sentence)
    let words: Vec<&str> = text.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        if i == 0 {
            continue;
        }
        let clean = word.trim_matches(|c: char| !c.is_alphanumeric());
        if clean.len() > 1
            && clean.chars().next().map_or(false, |c| c.is_uppercase())
            && clean.chars().skip(1).any(|c| c.is_lowercase())
        {
            // Check it's not a common sentence-starter after period
            let prev = if i > 0 { words[i - 1] } else { "" };
            if !prev.ends_with('.') && !prev.ends_with('!') && !prev.ends_with('?') {
                entities.push(clean.to_string());
            }
        }
    }

    // Quoted strings
    for q in extract_quotes(text) {
        entities.push(q);
    }

    entities
}

/// Check for entities in a sentence that don't appear in source material.
fn find_novel_entities(sentence: &str, observation: &str, client_context: &str) -> Vec<String> {
    let sent_entities = extract_entities(sentence);
    let obs_lower = observation.to_lowercase();
    let ctx_lower = client_context.to_lowercase();

    sent_entities
        .into_iter()
        .filter(|e| {
            let lower = e.to_lowercase();
            !obs_lower.contains(&lower) && !ctx_lower.contains(&lower)
        })
        .collect()
}

/// Layer 2: NLP structural checks — stemming, entity comparison, word overlap.
pub fn layer2_nlp_structural(
    note_sentences: &[String],
    observation: &str,
    client_context: &str,
    config: &FaithfulnessConfig,
) -> Vec<SentenceAssessment> {
    let combined_source = format!("{} {}", observation, client_context);

    note_sentences
        .iter()
        .map(|sentence| {
            // Stemmed word overlap
            let overlap = stemmed_overlap(sentence, &combined_source);

            // Novel entity check
            let novel = find_novel_entities(sentence, observation, client_context);

            if overlap >= config.overlap_threshold && novel.is_empty() {
                SentenceAssessment {
                    sentence: sentence.clone(),
                    assessed_by_layer: 2,
                    level: GroundingLevel::Grounded,
                    reason: format!("Stemmed overlap {:.0}%", overlap * 100.0),
                    best_match: None,
                    score: overlap,
                }
            } else if !novel.is_empty() {
                SentenceAssessment {
                    sentence: sentence.clone(),
                    assessed_by_layer: 2,
                    level: GroundingLevel::Uncertain,
                    reason: format!(
                        "Novel entities not in source: {}",
                        novel.join(", ")
                    ),
                    best_match: None,
                    score: overlap,
                }
            } else {
                SentenceAssessment {
                    sentence: sentence.clone(),
                    assessed_by_layer: 2,
                    level: GroundingLevel::Uncertain,
                    reason: format!(
                        "Low stemmed overlap ({:.0}%) with observation",
                        overlap * 100.0
                    ),
                    best_match: None,
                    score: overlap,
                }
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Layer 3: Sentence embeddings via Ollama /api/embed
// ---------------------------------------------------------------------------

/// Cosine similarity between two vectors.
fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Call Ollama /api/embed to get embeddings for a batch of texts.
fn embed_batch(
    client: &reqwest::blocking::Client,
    endpoint: &str,
    model: &str,
    texts: &[String],
) -> Result<Vec<Vec<f64>>> {
    // Ollama /api/embed accepts {"model": "...", "input": ["..."]}
    let body = serde_json::json!({
        "model": model,
        "input": texts,
    });

    let resp = client
        .post(format!("{}/api/embed", endpoint))
        .json(&body)
        .timeout(std::time::Duration::from_secs(30))
        .send()?;

    if !resp.status().is_success() {
        anyhow::bail!("Ollama embed failed: {}", resp.status());
    }

    let json: serde_json::Value = resp.json()?;
    let embeddings = json["embeddings"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("No embeddings in response"))?
        .iter()
        .map(|e| {
            e.as_array()
                .unwrap_or(&vec![])
                .iter()
                .map(|v| v.as_f64().unwrap_or(0.0))
                .collect()
        })
        .collect();

    Ok(embeddings)
}

/// Layer 3: Sentence embeddings via Ollama /api/embed.
/// Degrades gracefully if Ollama is unavailable (returns empty vec).
pub fn layer3_embeddings(
    note_sentences: &[String],
    observation_sentences: &[String],
    config: &FaithfulnessConfig,
) -> Vec<SentenceAssessment> {
    if note_sentences.is_empty() || observation_sentences.is_empty() {
        return vec![];
    }

    let endpoint = config
        .embedding_endpoint
        .as_deref()
        .unwrap_or("http://localhost:11434");

    // Build client with short connect timeout for health check
    let client = match reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    // Health check — try to reach Ollama
    if client
        .get(format!("{}/api/tags", endpoint))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .is_err()
    {
        eprintln!("  (Layer 3 skipped — embedding endpoint unreachable)");
        return vec![];
    }

    eprintln!("  Checking semantic similarity...");

    // Embed observation sentences
    let obs_embeddings = match embed_batch(
        &client,
        endpoint,
        &config.embedding_model,
        observation_sentences,
    ) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("  (Layer 3 skipped — embed failed: {})", e);
            return vec![];
        }
    };

    // Embed note sentences
    let note_embeddings = match embed_batch(
        &client,
        endpoint,
        &config.embedding_model,
        note_sentences,
    ) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("  (Layer 3 skipped — embed failed: {})", e);
            return vec![];
        }
    };

    // For each note sentence, find max similarity to any observation sentence
    note_sentences
        .iter()
        .zip(note_embeddings.iter())
        .map(|(sentence, note_emb)| {
            let mut max_sim = 0.0_f64;
            let mut best_obs = None;

            for (obs_sent, obs_emb) in observation_sentences.iter().zip(obs_embeddings.iter()) {
                let sim = cosine_similarity(note_emb, obs_emb);
                if sim > max_sim {
                    max_sim = sim;
                    best_obs = Some(obs_sent.clone());
                }
            }

            if max_sim >= config.embedding_threshold {
                SentenceAssessment {
                    sentence: sentence.clone(),
                    assessed_by_layer: 3,
                    level: GroundingLevel::Grounded,
                    reason: format!("Semantic similarity {:.0}%", max_sim * 100.0),
                    best_match: best_obs,
                    score: max_sim,
                }
            } else {
                SentenceAssessment {
                    sentence: sentence.clone(),
                    assessed_by_layer: 3,
                    level: GroundingLevel::Uncertain,
                    reason: format!(
                        "Low semantic similarity ({:.0}%) with observation",
                        max_sim * 100.0
                    ),
                    best_match: best_obs,
                    score: max_sim,
                }
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Coordinator
// ---------------------------------------------------------------------------

/// Truncate a string for display.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.min(s.len())])
    }
}

/// Run all faithfulness layers, cheapest first.
///
/// Layer 1 (string match) runs on all sentences.
/// Layer 2 (NLP structural) runs on sentences not yet resolved by Layer 1.
/// Layer 3 (embeddings) runs on sentences still Uncertain after Layer 2.
pub fn check_faithfulness(
    note: &str,
    observation: &str,
    client_context: &str,
) -> FaithfulnessResult {
    let config = load_config();
    let note_sentences = extract_checkable_sentences(note);
    let obs_sentences = extract_observation_sentences(observation);

    if note_sentences.is_empty() {
        return FaithfulnessResult {
            assessments: vec![],
        };
    }

    let mut final_assessments: Vec<SentenceAssessment> = Vec::new();
    let mut remaining: Vec<String> = Vec::new();

    // Layer 1: string match
    let l1_results = layer1_string_match(&note_sentences, observation, client_context, &config);
    for (assessment_opt, sentence) in l1_results.into_iter().zip(note_sentences.iter()) {
        match assessment_opt {
            Some(a) => final_assessments.push(a),
            None => remaining.push(sentence.clone()),
        }
    }

    if remaining.is_empty() {
        return FaithfulnessResult {
            assessments: final_assessments,
        };
    }

    // Layer 2: NLP structural
    let l2_results =
        layer2_nlp_structural(&remaining, observation, client_context, &config);

    let mut still_uncertain: Vec<String> = Vec::new();
    for assessment in l2_results {
        if assessment.level == GroundingLevel::Uncertain {
            still_uncertain.push(assessment.sentence.clone());
        }
        final_assessments.push(assessment);
    }

    if still_uncertain.is_empty() {
        return FaithfulnessResult {
            assessments: final_assessments,
        };
    }

    // Layer 3: embeddings (may be skipped if Ollama unavailable)
    let l3_results = layer3_embeddings(&still_uncertain, &obs_sentences, &config);

    // Replace Layer 2 uncertain assessments with Layer 3 results where available
    if !l3_results.is_empty() {
        let l3_map: std::collections::HashMap<String, SentenceAssessment> = l3_results
            .into_iter()
            .map(|a| (a.sentence.clone(), a))
            .collect();

        for assessment in &mut final_assessments {
            if assessment.level == GroundingLevel::Uncertain && assessment.assessed_by_layer == 2 {
                if let Some(l3) = l3_map.get(&assessment.sentence) {
                    *assessment = l3.clone();
                }
            }
        }
    }

    FaithfulnessResult {
        assessments: final_assessments,
    }
}

/// Format faithfulness flags for display in a batch review file.
pub fn format_flags_for_review(result: &FaithfulnessResult) -> Option<String> {
    let flags: Vec<&SentenceAssessment> = result
        .assessments
        .iter()
        .filter(|a| a.level != GroundingLevel::Grounded)
        .collect();

    if flags.is_empty() {
        return None;
    }

    let mut out = String::from("<!-- Faithfulness flags:\n");
    for flag in &flags {
        let icon = match flag.level {
            GroundingLevel::Ungrounded => "\u{1f6ab}",
            GroundingLevel::Uncertain => "\u{26a0}\u{fe0f}",
            GroundingLevel::Grounded => unreachable!(),
        };
        out.push_str(&format!(
            "  - {} \"{}\"\n    {}\n",
            icon,
            truncate(&flag.sentence, 80),
            flag.reason
        ));
    }
    out.push_str("-->\n");
    Some(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Sentence extraction --

    #[test]
    fn test_extract_checkable_sentences_skips_header_and_brief_risk() {
        let note = "\
### 2026-04-14

**Risk**: No immediate concerns noted.

Emma explored her avoidance pattern around dating. She identified a pull toward waiting for certainty before acting. We discussed this as experiential avoidance.

**Formulation**: Continued defusion from the rule that safety must precede action and increasing willingness to act with discomfort present.
";
        let sentences = extract_checkable_sentences(note);
        assert!(!sentences.iter().any(|s| s.contains("###")));
        assert!(!sentences.iter().any(|s| s.contains("No immediate concerns")));
        assert!(sentences.iter().any(|s| s.contains("avoidance pattern")));
        assert!(sentences.iter().any(|s| s.contains("defusion")));
    }

    #[test]
    fn test_extract_drops_short_fragments() {
        let note = "### 2026-04-14\n\n**Risk**: Low.\n\nOk then. She explored avoidance around dating and values work.\n\n**Formulation**: Ongoing.";
        let sentences = extract_checkable_sentences(note);
        assert!(!sentences.iter().any(|s| s.trim() == "Ok then"));
        assert!(!sentences.iter().any(|s| s.trim() == "Ongoing"));
    }

    // -- Layer 1: string match --

    #[test]
    fn test_layer1_catches_fabricated_quote() {
        let note_sentences = vec![
            "Emma described \"a long-standing pattern of waiting for total safety\" as central to her avoidance.".to_string(),
        ];
        let observation =
            "Emma discussed feeling stuck. She said she has nothing to show for 16 years.";
        let config = FaithfulnessConfig::default();
        let results = layer1_string_match(&note_sentences, observation, "", &config);
        assert!(results[0].is_some());
        let a = results[0].as_ref().unwrap();
        assert_eq!(a.level, GroundingLevel::Ungrounded);
        assert!(a.reason.contains("Fabricated quote"));
    }

    #[test]
    fn test_layer1_passes_grounded_content() {
        let note_sentences = vec![
            "Emma discussed feeling stuck after 16 years at her firm.".to_string(),
        ];
        let observation =
            "Emma discussed feeling stuck after 16 years at her firm. Low mood.";
        let config = FaithfulnessConfig::default();
        let results = layer1_string_match(&note_sentences, observation, "", &config);
        assert!(results[0].is_some());
        let a = results[0].as_ref().unwrap();
        assert_eq!(a.level, GroundingLevel::Grounded);
    }

    #[test]
    fn test_layer1_allows_clinical_terms() {
        let note_sentences = vec![
            "This was explored as a moment of cognitive fusion with the narrative of stagnation.".to_string(),
        ];
        let observation = "Emma feels stuck and fused with a story about failure.";
        let config = FaithfulnessConfig::default();
        let results = layer1_string_match(&note_sentences, observation, "", &config);
        // Should NOT be flagged as Ungrounded — "cognitive fusion" is an allowed clinical term
        if let Some(a) = &results[0] {
            assert_ne!(a.level, GroundingLevel::Ungrounded);
        }
    }

    // -- Layer 2: NLP structural --

    #[test]
    fn test_stemmed_overlap_high() {
        let score = stemmed_overlap(
            "She described waiting for safety before dating",
            "She said she keeps waiting for things to feel safe before acting",
        );
        assert!(score > 0.3, "Expected overlap > 0.3, got {}", score);
    }

    #[test]
    fn test_stemmed_overlap_low() {
        let score = stemmed_overlap(
            "A long-standing pattern of waiting for total safety",
            "Emma feels stuck at work after sixteen years",
        );
        assert!(score < 0.35, "Expected overlap < 0.35, got {}", score);
    }

    #[test]
    fn test_novel_entities_detected() {
        let novel = find_novel_entities(
            "She mentioned her colleague Sarah from the marketing team",
            "She discussed workplace stress",
            "",
        );
        assert!(novel.iter().any(|e| e.contains("Sarah")));
    }

    #[test]
    fn test_layer2_flags_low_overlap() {
        let note_sentences = vec![
            "The therapeutic relationship provided a corrective emotional experience rooted in early attachment patterns.".to_string(),
        ];
        let observation = "We talked about how she responds when I am late to session.";
        let config = FaithfulnessConfig::default();
        let results = layer2_nlp_structural(&note_sentences, observation, "", &config);
        assert_eq!(results[0].level, GroundingLevel::Uncertain);
    }

    // -- Layer 3: graceful degradation --

    #[test]
    fn test_layer3_graceful_degradation() {
        let mut config = FaithfulnessConfig::default();
        config.embedding_endpoint = Some("http://localhost:99999".to_string());
        let results = layer3_embeddings(
            &["test sentence with enough words here".to_string()],
            &["another test sentence with enough words".to_string()],
            &config,
        );
        assert!(results.is_empty());
    }

    // -- Integration --

    #[test]
    fn test_check_faithfulness_catches_fabricated_quote() {
        let observation = "Emma discussed feeling stuck. She said she has nothing to show for 16 years at her firm. We discussed the distinction between suffering from pursuing values and suffering maintained by avoidance.";

        let note = "\
### 2026-04-14

**Risk**: No immediate concerns noted.

Emma reported that she has nothing to show for her 16 years at her firm. She described \"a deep-seated terror of rejection rooted in early playground exclusion\" as the driver of her avoidance. We collaboratively considered the distinction between the suffering that accompanies pursuing values and the suffering maintained by avoidance.

**Formulation**: Emma is experiencing heightened distress and fusion with a narrative of perceived failure, potentially triggered by the news of limited therapeutic coverage.
";

        let result = check_faithfulness(note, observation, "");
        assert!(
            !result.passed_hard(),
            "Should have hard failures for fabricated quote"
        );
        assert!(result
            .hard_failures()
            .iter()
            .any(|f| f.sentence.contains("deep-seated terror")));
    }

    #[test]
    fn test_check_faithfulness_passes_clean_note() {
        let observation = "Emma discussed feeling stuck after 16 years at her firm. Low mood. We explored the distinction between values-pursuit suffering and avoidance-maintained suffering.";

        let note = "\
### 2026-04-14

**Risk**: No immediate concerns noted.

Emma discussed feeling stuck after 16 years at her firm. We explored the distinction between the suffering that comes from pursuing values and the suffering maintained through avoidance.

**Formulation**: Low mood with fusion around a narrative of stagnation.
";

        let result = check_faithfulness(note, observation, "");
        assert!(
            result.passed_hard(),
            "Clean note should pass: {:?}",
            result.hard_failures()
        );
    }
}
