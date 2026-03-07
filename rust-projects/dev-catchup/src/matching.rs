use std::collections::HashSet;
use std::path::Path;

use crate::types::*;

const MATCH_THRESHOLD: f64 = 0.15;

/// Basic normalization: strip trailing 's' for plural matching.
pub fn normalize_term(term: &str) -> String {
    let t = term.to_lowercase();
    // Strip trailing 's' if word is long enough (avoid "bus" -> "bu")
    if t.len() > 4 && t.ends_with('s') && !t.ends_with("ss") {
        t[..t.len() - 1].to_string()
    } else {
        t
    }
}

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "this", "that", "from", "into", "was", "were", "has", "had",
    "have", "not", "but", "are", "can", "its", "all", "also", "been", "more", "when", "will",
    "each", "then", "than", "use", "used", "using", "via", "after", "before", "new", "please",
    "yes", "yeah", "sure", "okay", "thanks", "let", "make", "want", "need", "would", "could",
    "should", "think", "know", "look", "see", "try", "like", "just", "did", "does", "done",
    "good", "file", "code", "work", "here", "now", "how", "out", "about", "some", "one", "two",
];

/// Convert a CcSession into a UnifiedSession with extracted terms.
pub fn unify_cc_session(session: &CcSession) -> UnifiedSession {
    let mut terms = HashSet::new();
    let stopwords: HashSet<&str> = STOPWORDS.iter().copied().collect();

    // File basenames + parent dir names (strongest signal)
    for path in session.files_modified.keys() {
        let p = Path::new(path);
        if let Some(name) = p.file_stem() {
            let n = normalize_term(&name.to_string_lossy());
            if n.len() >= 3 {
                terms.insert(n);
            }
        }
        if let Some(parent) = p.parent().and_then(|p| p.file_name()) {
            let n = normalize_term(&parent.to_string_lossy());
            if n.len() >= 3 && !["src", "bin", "lib", "config", "home"].contains(&n.as_str()) {
                terms.insert(n);
            }
        }
    }

    // Skill names
    for skill in &session.skills {
        let s = normalize_term(skill);
        if s.len() >= 3 {
            terms.insert(s);
        }
    }

    // Significant words from user messages
    let real_messages = count_real_messages(&session.user_messages);
    for (_ts, msg) in &session.user_messages {
        // Skip system messages
        if msg.starts_with('<') || msg.starts_with("Base directory") {
            continue;
        }
        for word in msg.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_') {
            let w = normalize_term(word);
            if w.len() >= 3 && !stopwords.contains(w.as_str()) {
                terms.insert(w);
            }
        }
    }

    let files_summary = if session.files_modified.is_empty() {
        "(none)".to_string()
    } else {
        session
            .files_modified
            .keys()
            .filter_map(|p| Path::new(p).file_name().map(|f| f.to_string_lossy().to_string()))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let skills_summary = if session.skills.is_empty() {
        String::new()
    } else {
        session.skills.join(", ")
    };

    let start = session
        .start_time
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_default();
    let end = session
        .end_time
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_default();

    // Build detail string for drafting prompt
    let mut detail = format!(
        "[CC] Session {} ({}-{}, {} msgs)\n",
        &session.session_id[..8.min(session.session_id.len())],
        start,
        end,
        real_messages,
    );
    if !session.files_modified.is_empty() {
        detail.push_str(&format!("  Files: {}\n", files_summary));
    }
    if !session.skills.is_empty() {
        detail.push_str(&format!("  Skills: {}\n", skills_summary));
    }
    // Include first few real user messages for context
    let mut msg_count = 0;
    for (_ts, msg) in &session.user_messages {
        if msg.starts_with('<') || msg.starts_with("Base directory") {
            continue;
        }
        if msg_count >= 5 {
            break;
        }
        let truncated: String = msg.chars().take(200).collect();
        detail.push_str(&format!("  User: {}\n", truncated));
        msg_count += 1;
    }

    UnifiedSession {
        source: SessionSource::Cc,
        session_id: session.session_id.clone(),
        start_time: session.start_time.map(|t| t.format("%H:%M").to_string()),
        end_time: session.end_time.map(|t| t.format("%H:%M").to_string()),
        message_count: real_messages,
        terms,
        files_summary,
        skills_summary,
        detail,
    }
}

/// Convert a ContinuumSession into a UnifiedSession.
pub fn unify_continuum_session(session: &ContinuumSession) -> UnifiedSession {
    let mut terms = HashSet::new();
    let stopwords: HashSet<&str> = STOPWORDS.iter().copied().collect();

    // Extract terms from title
    if let Some(title) = &session.title {
        for word in title.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_') {
            let w = normalize_term(word);
            if w.len() >= 3 && !stopwords.contains(w.as_str()) {
                terms.insert(w);
            }
        }
    }

    let msg_count = session.message_count.unwrap_or(0) as usize;

    let detail = format!(
        "[{}] Session {} ({})\n  Title: {}\n  Messages: {}\n",
        session.assistant,
        &session.session_id[..8.min(session.session_id.len())],
        session.start_time.as_deref().unwrap_or("?"),
        session.title.as_deref().unwrap_or("(none)"),
        msg_count,
    );

    UnifiedSession {
        source: SessionSource::Continuum(session.assistant.clone()),
        session_id: session.session_id.clone(),
        start_time: session.start_time.clone(),
        end_time: session.end_time.clone(),
        message_count: msg_count,
        terms,
        files_summary: String::new(),
        skills_summary: String::new(),
        detail,
    }
}

/// Count real user messages (excluding system messages).
fn count_real_messages(messages: &[(chrono::DateTime<chrono::Utc>, String)]) -> usize {
    messages
        .iter()
        .filter(|(_, msg)| !msg.starts_with('<') && !msg.starts_with("Base directory"))
        .count()
}

/// Check if a session is trivial (should be excluded).
pub fn is_trivial(session: &UnifiedSession) -> bool {
    match &session.source {
        SessionSource::Cc => session.message_count < 3,
        SessionSource::Continuum(_) => session.message_count <= 1,
    }
}

/// Match a session against dev:: entries. Returns the best match result.
pub fn match_session(session: &UnifiedSession, entries: &[DevEntry]) -> MatchResult {
    if entries.is_empty() || session.terms.is_empty() {
        return MatchResult::Unmatched;
    }

    let mut best_score = 0.0_f64;
    let mut best_entry = None;
    let mut best_overlap = vec![];

    for entry in entries {
        if entry.terms.is_empty() {
            continue;
        }
        let overlap: Vec<String> = session
            .terms
            .intersection(&entry.terms)
            .cloned()
            .collect();
        let denom = session.terms.len().min(entry.terms.len()) as f64;
        let score = overlap.len() as f64 / denom;

        if score > best_score {
            best_score = score;
            best_entry = Some(entry);
            best_overlap = overlap;
        }
    }

    if best_score >= MATCH_THRESHOLD {
        if let Some(entry) = best_entry {
            return MatchResult::Matched {
                entry_raw: entry.raw.clone(),
                overlap_terms: best_overlap,
            };
        }
    }

    MatchResult::Unmatched
}
