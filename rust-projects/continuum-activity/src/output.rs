use crate::types::{CcSession, DayActivity};

/// Replace home directory prefix with ~/
fn tilde_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

/// Format a time range like "09:09–11:33 UTC"
fn format_time_range(session: &CcSession) -> String {
    match (&session.start_time, &session.end_time) {
        (Some(start), Some(end)) => {
            format!(
                "{}\u{2013}{} UTC",
                start.format("%H:%M"),
                end.format("%H:%M")
            )
        }
        (Some(start), None) => format!("{} UTC", start.format("%H:%M")),
        _ => "unknown time".to_string(),
    }
}

/// Format a continuum session time range from ISO strings.
fn format_continuum_time_range(start: &Option<String>, end: &Option<String>) -> String {
    let parse = |s: &str| -> Option<String> {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.format("%H:%M").to_string())
    };

    match (start.as_deref().and_then(parse), end.as_deref().and_then(parse)) {
        (Some(s), Some(e)) => format!("{s}\u{2013}{e} UTC"),
        (Some(s), None) => format!("{s} UTC"),
        _ => "unknown time".to_string(),
    }
}

/// Render the activity report as markdown.
pub fn render_markdown(activity: &DayActivity) -> String {
    let mut out = String::new();

    out.push_str(&format!("# AI Activity: {}\n", activity.date));

    if !activity.cc_sessions.is_empty() {
        out.push_str("\n## Claude Code Sessions\n");

        for session in &activity.cc_sessions {
            let name = if session.slug.is_empty() {
                &session.session_id
            } else {
                &session.slug
            };
            out.push_str(&format!(
                "\n### Session: {} ({})\n",
                name,
                format_time_range(session)
            ));

            if !session.skills.is_empty() {
                out.push_str(&format!("Skills: {}\n", session.skills.join(", ")));
            }

            if !session.files_modified.is_empty() {
                out.push_str("Files Modified:\n");
                for (path, count) in &session.files_modified {
                    out.push_str(&format!("- {} ({} edits)\n", tilde_path(path), count));
                }
            }

            if !session.user_messages.is_empty() {
                out.push_str("User Requests (chronological):\n");
                for (ts, msg) in &session.user_messages {
                    out.push_str(&format!("- {}: \"{}\"\n", ts.format("%H:%M"), msg));
                }
            }

            if !session.tool_usage.is_empty() {
                let tools: Vec<String> = session
                    .tool_usage
                    .iter()
                    .map(|(name, count)| format!("{name}: {count}"))
                    .collect();
                out.push_str(&format!("Tool Usage: {}\n", tools.join(", ")));
            }
        }
    }

    if !activity.continuum_sessions.is_empty() {
        out.push_str("\n## Other AI Sessions\n");

        for session in &activity.continuum_sessions {
            let title_part = match &session.title {
                Some(t) => format!(": \"{}\"", t),
                None => String::new(),
            };
            let time = format_continuum_time_range(&session.start_time, &session.end_time);
            let msg_count = session
                .message_count
                .map(|c| format!(", {} messages", c))
                .unwrap_or_default();
            let assistant = capitalize(&session.assistant);
            out.push_str(&format!(
                "\n### {}{} ({}{})  \n",
                assistant, title_part, time, msg_count
            ));
        }
    }

    out
}

/// Render the activity report as JSON.
pub fn render_json(activity: &DayActivity) -> String {
    serde_json::to_string_pretty(activity).unwrap_or_else(|_| "{}".to_string())
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}
