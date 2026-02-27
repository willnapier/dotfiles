use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct SessionMeta {
    id: String,
    assistant: String,
    start_time: Option<String>,
    end_time: Option<String>,
    message_count: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct Message {
    role: String,
    content: String,
}

struct SessionInfo {
    path: PathBuf,
    meta: SessionMeta,
}

struct SessionMatch {
    session: SessionInfo,
    cleaned_text: String,
    approx_tokens: usize,
    snippet: String,
    relevance: Relevance,
}

struct Relevance {
    /// Total occurrences of the query in the session
    match_count: usize,
    /// Matches per 1000 tokens — how focused the session is on the topic
    density: f64,
    /// Whether the user (not just the assistant) mentions the query
    user_initiated: bool,
    /// Classification
    tag: RelevanceTag,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RelevanceTag {
    /// High density or user-initiated with multiple matches — core discussion
    Focused,
    /// Moderate engagement — topic is substantive but not the main thread
    Relevant,
    /// Low density — passing mention in a session about something else
    Mention,
}

// ANSI colour codes
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const WHITE: &str = "\x1b[37m";
const BRIGHT_GREEN: &str = "\x1b[92m";
const BRIGHT_YELLOW: &str = "\x1b[93m";

impl RelevanceTag {
    #[allow(dead_code)]
    fn label(&self) -> &'static str {
        match self {
            RelevanceTag::Focused => "FOCUSED",
            RelevanceTag::Relevant => "relevant",
            RelevanceTag::Mention => "mention",
        }
    }

    fn coloured_label(&self) -> String {
        match self {
            RelevanceTag::Focused => format!("{BOLD}{BRIGHT_GREEN}FOCUSED{RESET}"),
            RelevanceTag::Relevant => format!("{BRIGHT_YELLOW}relevant{RESET}"),
            RelevanceTag::Mention => format!("{DIM}mention{RESET}"),
        }
    }
}

fn estimate_tokens(text: &str) -> usize {
    (text.len() + 3) / 4
}

fn compute_relevance(cleaned_text: &str, query_lower: &str) -> Relevance {
    let text_lower = cleaned_text.to_lowercase();
    let match_count = text_lower.matches(query_lower).count();
    let tokens = estimate_tokens(cleaned_text).max(1);
    let density = (match_count as f64 / tokens as f64) * 1000.0;

    // Check if user messages contain the query
    let user_initiated = cleaned_text
        .split("[User]\n")
        .skip(1) // skip text before first [User]
        .any(|block| {
            // Take text up to next role marker
            let user_text = block.split("[Assistant]\n").next().unwrap_or(block);
            user_text.to_lowercase().contains(query_lower)
        });

    let tag = if density >= 1.0 || (user_initiated && match_count >= 3) {
        RelevanceTag::Focused
    } else if (user_initiated && match_count >= 1) || match_count >= 3 || density >= 0.3 {
        RelevanceTag::Relevant
    } else {
        RelevanceTag::Mention
    };

    Relevance {
        match_count,
        density,
        user_initiated,
        tag,
    }
}

pub fn load_session(
    session_id: Option<&str>,
    last: bool,
    assistant_filter: Option<&str>,
    search: Option<&str>,
    all: bool,
) -> Result<()> {
    let base_dir = dirs::home_dir()
        .context("No home directory")?
        .join("Assistants/continuum-logs");

    if !base_dir.exists() {
        bail!("Continuum logs directory not found: {}", base_dir.display());
    }

    if let Some(query) = search {
        return search_and_load(&base_dir, query, assistant_filter, all);
    }

    let session = if last {
        find_last_session(&base_dir, assistant_filter)?
    } else if let Some(id) = session_id {
        find_session_by_id(&base_dir, id)?
    } else {
        bail!("Specify --last, --search, or provide a session ID");
    };

    let text = build_cleaned_text(&session)?;
    let tokens = estimate_tokens(&text);
    eprintln!(
        "Session: {} | {} | approx {}k tokens",
        session.meta.assistant,
        format_time_range(&session.meta.start_time, &session.meta.end_time),
        (tokens + 500) / 1000,
    );
    print!("{}", text);

    Ok(())
}

fn search_and_load(
    base_dir: &Path,
    query: &str,
    assistant_filter: Option<&str>,
    all: bool,
) -> Result<()> {
    let sessions = collect_sessions(base_dir, assistant_filter)?;
    let query_lower = query.to_lowercase();

    let mut matches: Vec<SessionMatch> = Vec::new();

    for session in sessions {
        let messages_path = session.path.join("messages.jsonl");
        if !messages_path.exists() {
            continue;
        }

        let raw = std::fs::read_to_string(&messages_path).unwrap_or_default();
        let raw_lower = raw.to_lowercase();

        if !raw_lower.contains(&query_lower) {
            continue;
        }

        let snippet = extract_snippet(&raw, &query_lower);
        let cleaned_text = build_cleaned_text(&session)?;
        let approx_tokens = estimate_tokens(&cleaned_text);
        let relevance = compute_relevance(&cleaned_text, &query_lower);

        matches.push(SessionMatch {
            session,
            cleaned_text,
            approx_tokens,
            snippet,
            relevance,
        });
    }

    if matches.is_empty() {
        bail!("No sessions found matching '{}'", query);
    }

    // Sort by relevance tier first (Focused → Relevant → Mention), then recency within tier
    matches.sort_by(|a, b| {
        a.relevance
            .tag
            .cmp(&b.relevance.tag)
            .then_with(|| {
                b.session
                    .meta
                    .start_time
                    .cmp(&a.session.meta.start_time)
            })
    });

    if all {
        return output_all_matches(&matches);
    }

    // Build recommended set: all Focused + Relevant sessions
    let recommended_indices: Vec<usize> = matches
        .iter()
        .enumerate()
        .filter(|(_, m)| m.relevance.tag != RelevanceTag::Mention)
        .map(|(i, _)| i)
        .collect();
    let recommended_tokens: usize = recommended_indices
        .iter()
        .map(|&i| matches[i].approx_tokens)
        .sum();

    // Build results display
    let total_tokens: usize = matches.iter().map(|m| m.approx_tokens).sum();

    let mut display = format!(
        "\n{BOLD}Found {} sessions matching '{CYAN}{}{RESET}{BOLD}'{RESET} {DIM}(total approx {}k tokens){RESET}\n\n",
        matches.len(),
        query,
        (total_tokens + 500) / 1000,
    );

    let mut current_tier: Option<RelevanceTag> = None;
    for (i, m) in matches.iter().enumerate() {
        // Insert tier separator when the relevance tier changes
        if current_tier != Some(m.relevance.tag) {
            if current_tier.is_some() {
                display.push('\n');
            }
            let tier_label = match m.relevance.tag {
                RelevanceTag::Focused => format!("  {BOLD}{BRIGHT_GREEN}── Focused ──{RESET}\n"),
                RelevanceTag::Relevant => format!("  {BRIGHT_YELLOW}── Relevant ──{RESET}\n"),
                RelevanceTag::Mention => format!("  {DIM}── Passing mentions ──{RESET}\n"),
            };
            display.push_str(&tier_label);
            current_tier = Some(m.relevance.tag);
        }

        let time = format_time_range(
            &m.session.meta.start_time,
            &m.session.meta.end_time,
        );
        let msgs = m
            .session
            .meta
            .message_count
            .map(|c| format!("{} msgs", c))
            .unwrap_or_else(|| "? msgs".to_string());
        let coloured_tag = m.relevance.tag.coloured_label();
        let user_flag = if m.relevance.user_initiated {
            format!("{GREEN}+{RESET}")
        } else {
            " ".to_string()
        };
        display.push_str(&format!(
            "  {BOLD}{WHITE}[{}]{RESET} {} {}{BOLD}{}{RESET} {DIM}|{RESET} {} {DIM}|{RESET} {} {DIM}| approx {}k tokens{RESET}\n",
            i + 1,
            coloured_tag,
            user_flag,
            m.session.meta.assistant,
            time,
            msgs,
            (m.approx_tokens + 500) / 1000,
        ));
        display.push_str(&format!(
            "      {DIM}({} matches, {:.1}/1k density) \"{}\"{RESET}\n",
            m.relevance.match_count, m.relevance.density, m.snippet,
        ));
    }

    // Show options
    if !recommended_indices.is_empty() && recommended_indices.len() < matches.len() {
        let rec_list: Vec<String> = recommended_indices.iter().map(|i| format!("{}", i + 1)).collect();
        display.push_str(&format!(
            "\n  {BOLD}{GREEN}[r]{RESET} Recommended: sessions {GREEN}{}{RESET} ({} sessions, approx {}k tokens)\n",
            rec_list.join(","),
            recommended_indices.len(),
            (recommended_tokens + 500) / 1000,
        ));
    }
    display.push_str(&format!(
        "  {BOLD}{YELLOW}[a]{RESET} Load all ({} sessions, approx {}k tokens)\n",
        matches.len(),
        (total_tokens + 500) / 1000,
    ));

    // Display through pager (less -RFX: ANSI passthrough, quit-if-one-screen, no clear)
    display_with_pager(&display);

    eprint!(
        "\n{BOLD}Select [1-{}/a] (Enter = recommended):{RESET} ",
        matches.len()
    );
    std::io::stderr().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let input = input.trim();

    // Empty input or 'r' → load recommended set
    if input.is_empty() || input.eq_ignore_ascii_case("r") {
        if recommended_indices.is_empty() {
            // No recommended set — fall back to all
            return output_all_matches(&matches);
        }
        let recommended: Vec<&SessionMatch> = recommended_indices
            .iter()
            .map(|&i| &matches[i])
            .collect();
        return output_selected_matches(&recommended);
    }

    if input.eq_ignore_ascii_case("a") {
        return output_all_matches(&matches);
    }

    // Support comma-separated selection: "3,4,10"
    let indices: Result<Vec<usize>, _> = input
        .split(',')
        .map(|s| {
            s.trim()
                .parse::<usize>()
                .ok()
                .and_then(|n| if n >= 1 && n <= matches.len() { Some(n - 1) } else { None })
                .ok_or_else(|| anyhow::anyhow!("Invalid selection: {}", s.trim()))
        })
        .collect();

    let indices = indices?;

    if indices.len() == 1 {
        let m = &matches[indices[0]];
        eprintln!(
            "\nLoading: {} | {} | approx {}k tokens",
            m.session.meta.assistant,
            format_time_range(&m.session.meta.start_time, &m.session.meta.end_time),
            (m.approx_tokens + 500) / 1000,
        );
        print!("{}", m.cleaned_text);
    } else {
        let selected: Vec<&SessionMatch> = indices.iter().map(|&i| &matches[i]).collect();
        output_selected_matches(&selected)?;
    }

    Ok(())
}

fn output_all_matches(matches: &[SessionMatch]) -> Result<()> {
    let all_refs: Vec<&SessionMatch> = matches.iter().collect();
    output_selected_matches(&all_refs)
}

fn output_selected_matches(matches: &[&SessionMatch]) -> Result<()> {
    let total_tokens: usize = matches.iter().map(|m| m.approx_tokens).sum();
    eprintln!(
        "\nLoading {} sessions (approx {}k tokens total)",
        matches.len(),
        (total_tokens + 500) / 1000,
    );

    for (i, m) in matches.iter().enumerate() {
        let time = format_time_range(
            &m.session.meta.start_time,
            &m.session.meta.end_time,
        );
        if i > 0 {
            println!();
        }
        println!(
            "--- Session: {} | {} ---\n",
            m.session.meta.assistant, time,
        );
        print!("{}", m.cleaned_text);
    }

    Ok(())
}

/// Display text through a pager (less) for scrollable output.
/// Always writes to /dev/tty so the pager displays on the terminal
/// even when stdout is piped (e.g. `continuum-activity ... | gemini`).
/// Falls back to direct stderr output if less or /dev/tty is unavailable.
fn display_with_pager(text: &str) {
    // Open /dev/tty for writing so less displays on terminal, not into the pipe
    let tty = std::fs::OpenOptions::new().write(true).open("/dev/tty");
    let stdout_cfg = match tty {
        Ok(f) => std::process::Stdio::from(f),
        Err(_) => std::process::Stdio::inherit(),
    };

    // less -R: ANSI passthrough, -F: quit if fits on one screen, -X: don't clear on exit
    if let Ok(mut child) = std::process::Command::new("less")
        .args(["-RX"])
        .stdin(std::process::Stdio::piped())
        .stdout(stdout_cfg)
        .stderr(std::process::Stdio::inherit())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
            drop(stdin);
        }
        let _ = child.wait();
    } else {
        // Fallback: print directly to stderr
        eprint!("{}", text);
    }
}

fn extract_snippet(raw: &str, query_lower: &str) -> String {
    let raw_lower = raw.to_lowercase();
    if let Some(pos) = raw_lower.find(query_lower) {
        let start = pos.saturating_sub(40);
        let end = (pos + query_lower.len() + 60).min(raw.len());
        let start = if start > 0 {
            raw[start..].find(' ').map(|p| start + p + 1).unwrap_or(start)
        } else {
            start
        };
        let snippet: String = raw[start..end]
            .chars()
            .map(|c| if c == '\n' { ' ' } else { c })
            .collect();
        let snippet = snippet.trim();
        if start > 0 {
            format!("...{}", snippet)
        } else {
            snippet.to_string()
        }
    } else {
        String::new()
    }
}

fn build_cleaned_text(session: &SessionInfo) -> Result<String> {
    let messages_path = session.path.join("messages.jsonl");
    if !messages_path.exists() {
        bail!("No messages file found for session {}", session.meta.id);
    }

    let content = std::fs::read_to_string(&messages_path)?;
    let mut output = String::new();

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }

        if let Ok(msg) = serde_json::from_str::<Message>(line) {
            let cleaned = clean_content(&msg.content);
            if cleaned.is_empty() {
                continue;
            }
            let role_label = match msg.role.as_str() {
                "user" => "User",
                "assistant" => "Assistant",
                _ => &msg.role,
            };
            output.push_str(&format!("[{}]\n{}\n\n", role_label, cleaned));
        }
    }

    Ok(output)
}

fn collect_sessions(base_dir: &Path, assistant_filter: Option<&str>) -> Result<Vec<SessionInfo>> {
    let mut all_sessions = Vec::new();

    for assistant_entry in std::fs::read_dir(base_dir)?.flatten() {
        let assistant_dir = assistant_entry.path();
        if !assistant_dir.is_dir() {
            continue;
        }

        let assistant_name = assistant_entry.file_name().to_string_lossy().to_string();
        if let Some(filter) = assistant_filter {
            if assistant_name != filter {
                continue;
            }
        }

        for date_entry in std::fs::read_dir(&assistant_dir)?.flatten() {
            let date_dir = date_entry.path();
            if !date_dir.is_dir() {
                continue;
            }

            for session_entry in std::fs::read_dir(&date_dir)?.flatten() {
                let session_dir = session_entry.path();
                let session_json = session_dir.join("session.json");
                if !session_json.exists() {
                    continue;
                }

                if let Ok(content) = std::fs::read_to_string(&session_json) {
                    if let Ok(meta) = serde_json::from_str::<SessionMeta>(&content) {
                        all_sessions.push(SessionInfo {
                            path: session_dir,
                            meta,
                        });
                    }
                }
            }
        }
    }

    Ok(all_sessions)
}

fn find_last_session(base_dir: &Path, assistant_filter: Option<&str>) -> Result<SessionInfo> {
    let mut sessions = collect_sessions(base_dir, assistant_filter)?;

    if sessions.is_empty() {
        bail!(
            "No sessions found{}",
            assistant_filter
                .map(|a| format!(" for assistant '{}'", a))
                .unwrap_or_default()
        );
    }

    sessions.sort_by(|a, b| b.meta.start_time.cmp(&a.meta.start_time));

    Ok(sessions.into_iter().next().unwrap())
}

fn find_session_by_id(base_dir: &Path, id: &str) -> Result<SessionInfo> {
    let sessions = collect_sessions(base_dir, None)?;

    for session in sessions {
        if session.meta.id == id || session.meta.id.starts_with(id) {
            return Ok(session);
        }
        if let Some(dir_name) = session.path.file_name().and_then(|n| n.to_str()) {
            if dir_name == id || dir_name.starts_with(id) {
                return Ok(session);
            }
        }
    }

    bail!("No session found matching ID '{}'", id);
}

/// Strip system scaffolding, tool XML, and command noise from message content.
fn clean_content(content: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    let mut tag_name = String::new();

    let skip_tags = [
        "local-command-caveat",
        "command-name",
        "command-args",
        "local-command-stdout",
        "system-reminder",
        "antml:function_calls",
        "antml:invoke",
        "antml:parameter",
        "function_results",
    ];

    for line in content.lines() {
        let trimmed = line.trim();

        if let Some(rest) = trimmed.strip_prefix('<') {
            if let Some(rest) = rest.strip_prefix('/') {
                if let Some(name) = rest.split('>').next() {
                    let name = name.split_whitespace().next().unwrap_or(name);
                    if skip_tags.iter().any(|t| *t == name) {
                        in_tag = false;
                        tag_name.clear();
                        continue;
                    }
                }
            } else if let Some(name) =
                rest.split('>').next().or_else(|| rest.split_whitespace().next())
            {
                let name = name.trim_end_matches('/');
                if skip_tags.iter().any(|t| *t == name) {
                    in_tag = true;
                    tag_name = name.to_string();
                    continue;
                }
            }
        }

        if in_tag {
            continue;
        }

        if trimmed.starts_with("<command-message>") {
            continue;
        }

        result.push_str(line);
        result.push('\n');
    }

    let mut collapsed = String::new();
    let mut blank_count = 0;
    for line in result.lines() {
        if line.trim().is_empty() {
            blank_count += 1;
            if blank_count <= 2 {
                collapsed.push('\n');
            }
        } else {
            blank_count = 0;
            collapsed.push_str(line);
            collapsed.push('\n');
        }
    }

    collapsed.trim().to_string()
}

fn format_time_range(start: &Option<String>, end: &Option<String>) -> String {
    let parse = |s: &str| -> Option<String> {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
    };

    match (
        start.as_deref().and_then(parse),
        end.as_deref().and_then(parse),
    ) {
        (Some(s), Some(e)) => format!("{}–{}", s, e),
        (Some(s), None) => s,
        _ => "unknown time".to_string(),
    }
}
