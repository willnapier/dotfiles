use crate::types::*;

/// Render the full report for all days.
pub fn render_report(reports: &[DayReport], apply: bool) {
    if reports.is_empty() {
        println!("No activity found for the date range.");
        return;
    }

    let first = reports.first().unwrap().date;
    let last = reports.last().unwrap().date;
    println!("# dev-catchup: {} to {}\n", first, last);

    let mut any_unmatched = false;

    for report in reports {
        let total = report
            .sessions
            .iter()
            .filter(|(_, m)| !matches!(m, MatchResult::Trivial | MatchResult::Clinical))
            .count();
        let matched = report
            .sessions
            .iter()
            .filter(|(_, m)| matches!(m, MatchResult::Matched { .. }))
            .count();
        let unmatched = report
            .sessions
            .iter()
            .filter(|(_, m)| matches!(m, MatchResult::Unmatched))
            .count();
        let clinical = report
            .sessions
            .iter()
            .filter(|(_, m)| matches!(m, MatchResult::Clinical))
            .count();
        let trivial = report
            .sessions
            .iter()
            .filter(|(_, m)| matches!(m, MatchResult::Trivial))
            .count();

        if total == 0 && trivial == 0 {
            continue; // skip days with no sessions at all
        }

        let mut header = format!(
            "## {} ({} session{}",
            report.date,
            total,
            if total == 1 { "" } else { "s" }
        );
        if matched > 0 {
            header.push_str(&format!(", {} matched", matched));
        }
        if unmatched > 0 {
            header.push_str(&format!(", {} unmatched", unmatched));
            any_unmatched = true;
        }
        if clinical > 0 {
            header.push_str(&format!(", {} clinical", clinical));
        }
        if trivial > 0 {
            header.push_str(&format!(", {} trivial", trivial));
        }
        header.push(')');
        println!("{}", header);

        for (session, result) in &report.sessions {
            let source_tag = match &session.source {
                SessionSource::Cc => "[CC]".to_string(),
                SessionSource::Continuum(a) => format!("[{}]", a),
            };
            let time_range = format!(
                "{}-{}",
                session.start_time.as_deref().unwrap_or("?"),
                session.end_time.as_deref().unwrap_or("?"),
            );
            let short_id = &session.session_id[..8.min(session.session_id.len())];

            match result {
                MatchResult::Matched {
                    entry_raw,
                    overlap_terms,
                } => {
                    println!(
                        "  MATCHED: {} Session {} ({}, {} msgs)",
                        source_tag, short_id, time_range, session.message_count,
                    );
                    // Truncate entry for display
                    let display: String = entry_raw.chars().take(100).collect();
                    let suffix = if entry_raw.len() > 100 { "..." } else { "" };
                    println!("    -> \"{}{}\"\n    overlap: {}", display, suffix, overlap_terms.join(", "));
                }
                MatchResult::Unmatched => {
                    println!(
                        "  UNMATCHED: {} Session {} ({}, {} msgs)",
                        source_tag, short_id, time_range, session.message_count,
                    );
                    let mut extras = vec![];
                    if !session.files_summary.is_empty() {
                        extras.push(format!("Files: {}", session.files_summary));
                    }
                    if !session.skills_summary.is_empty() {
                        extras.push(format!("Skills: {}", session.skills_summary));
                    }
                    if !extras.is_empty() {
                        println!("    {}", extras.join(" | "));
                    }
                }
                MatchResult::Clinical => {
                    println!(
                        "  CLINICAL: {} Session {} ({}, {} msgs) — skipped",
                        source_tag, short_id, time_range, session.message_count,
                    );
                }
                MatchResult::Trivial => {
                    println!(
                        "  TRIVIAL: {} Session {} ({}, {} msgs) — skipped",
                        source_tag, short_id, time_range, session.message_count,
                    );
                }
            }
        }

        // Show drafts for this day
        for draft in &report.drafts {
            println!("\n  DRAFT: {}", draft.entry);
        }

        println!();
    }

    if !apply && any_unmatched {
        println!("To apply drafts: dev-catchup --apply");
    }
}
