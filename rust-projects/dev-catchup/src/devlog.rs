use anyhow::{Context, Result};
use chrono::{Datelike, NaiveDate};
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const DEVLOG_PROMPT: &str = r#"You are writing ONE entry for a developer's work log, describing one project's work on a single day. You are given the project name, the files touched, and the commit subjects. Write 1-3 plain past-tense sentences naming the PROBLEM addressed and WHAT was done, grounded ONLY in the provided commits and files — do not speculate beyond them, and do not mention anything not present here. Use specific names (files, features, modules). Output ONLY the sentences — no preamble, no meta-commentary, no "dev::", no counts, no headers. Then a final line: TOPICS: tag-one, tag-two"#;

/// A single DevLog entry: one project's work on one day.
#[derive(Debug, Clone)]
pub struct DevLogEntry {
    pub date: NaiveDate,
    pub primary_project: String,
    pub all_projects: Vec<String>,
    pub prs: Vec<u32>,        // sorted unique
    pub commits: Vec<String>, // short shas, dedup, in log order
    pub topics: Vec<String>,  // kebab tags from the AI draft, may be empty
    pub prose: String,
    pub files_count: usize, // distinct files touched in this project that day
    pub edits: u32,         // summed edit count across those files
}

/// Path to the per-key prose/topics cache file.
fn cache_path(cache_key: &str) -> PathBuf {
    cache_dir().join(format!("{}.md", cache_key))
}

/// Prose cache dir — `~/.local/share/devlog-cache` (matches the daypage-pending
/// convention, not the macOS Application Support dir).
fn cache_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/share/devlog-cache")
}

/// Resolve prose + topics for a (date, project) bucket, using the cache when
/// present.
///
/// - If the cache file exists, parse a trailing `TOPICS:` line (if any); the
///   rest is the prose.
/// - Otherwise, unless `no_ai`, call `claude -p` with the bucket detail and
///   cache the result.
/// - If `no_ai`, prose = "(no AI draft)", topics = [], and no cache is written.
pub fn resolve_prose(cache_key: &str, detail: &str, no_ai: bool) -> (String, Vec<String>) {
    let path = cache_path(cache_key);

    if let Ok(contents) = fs::read_to_string(&path) {
        return parse_claude_response(&contents);
    }

    if no_ai {
        return ("(no AI draft)".to_string(), vec![]);
    }

    match call_claude(detail) {
        Ok((prose, topics)) => {
            let _ = write_cache(&path, &prose, &topics);
            (prose, topics)
        }
        Err(e) => {
            eprintln!("Warning: AI draft failed for {}: {}", cache_key, e);
            ("(no AI draft)".to_string(), vec![])
        }
    }
}

/// Split a claude/cache response into prose + topics by the last `TOPICS:` line.
fn parse_claude_response(text: &str) -> (String, Vec<String>) {
    let trimmed = text.trim_end();
    let mut lines: Vec<&str> = trimmed.lines().collect();

    let mut topics: Vec<String> = vec![];
    if let Some(last) = lines.last() {
        let last_trim = last.trim();
        if let Some(rest) = last_trim.strip_prefix("TOPICS:") {
            topics = rest
                .split(',')
                .map(|t| kebab(t.trim()))
                .filter(|t| !t.is_empty())
                .collect();
            lines.pop();
        }
    }

    let prose = lines.join("\n").trim().to_string();
    (prose, topics)
}

/// Normalize a topic tag to kebab-case (lowercase, spaces/underscores → '-').
fn kebab(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if c == '-' || c == '_' || c.is_whitespace() {
            if !out.ends_with('-') && !out.is_empty() {
                out.push('-');
            }
        }
    }
    out.trim_matches('-').to_string()
}

/// Call `claude -p` with the DevLog prompt and the bucket detail on stdin.
fn call_claude(detail: &str) -> Result<(String, Vec<String>)> {
    let mut child = Command::new("claude");
    child.env_remove("ANTHROPIC_API_KEY");
    let mut child = child
        .args(["-p", DEVLOG_PROMPT])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn claude -p")?;

    child
        .stdin
        .take()
        .expect("stdin not captured")
        .write_all(detail.as_bytes())
        .context("Failed to write to claude stdin")?;

    let output = child
        .wait_with_output()
        .context("Failed to wait for claude -p")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("claude -p failed: {}", stderr.trim());
    }

    let response =
        String::from_utf8(output.stdout).context("claude -p output is not valid UTF-8")?;

    Ok(parse_claude_response(&response))
}

/// Write prose + topics to the cache file (creating the cache dir if missing).
fn write_cache(path: &PathBuf, prose: &str, topics: &[String]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("Failed to create devlog-cache dir")?;
    }
    let mut contents = prose.to_string();
    if !topics.is_empty() {
        contents.push_str(&format!("\nTOPICS: {}", topics.join(", ")));
    }
    fs::write(path, contents).context("Failed to write devlog cache")?;
    Ok(())
}

/// Path to the week file: ~/Forge/NapierianLogs/DevLog/{iso_year}-W{week:02}.md
pub fn week_path(d: NaiveDate) -> PathBuf {
    let iso = d.iso_week();
    let label = format!("{}-W{:02}", iso.year(), iso.week());
    devlog_dir().join(format!("{}.md", label))
}

/// The DevLog output directory: ~/Forge/NapierianLogs/DevLog
pub fn devlog_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Forge")
        .join("NapierianLogs")
        .join("DevLog")
}

/// Render a full week file (Format B) for the given entries.
pub fn render_week(week_label: &str, entries: &[DevLogEntry]) -> String {
    let mut out = format!("# DevLog {}\n\n", week_label);

    let mut sorted: Vec<&DevLogEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| {
        a.date
            .cmp(&b.date)
            .then_with(|| a.primary_project.cmp(&b.primary_project))
    });

    for e in sorted {
        out.push_str(&render_entry(e));
    }

    out
}

/// "1 file" / "2 files" — pluralize a count.
fn plural(n: usize, noun: &str) -> String {
    format!("{} {}{}", n, noun, if n == 1 { "" } else { "s" })
}

/// Render a single entry block (Format B).
fn render_entry(e: &DevLogEntry) -> String {
    let date_str = e.date.format("%Y-%m-%d").to_string();

    // Heading: ## {date} · {primary_project}{ · #{first_pr} if any}
    let mut heading = format!("## {} · {}", date_str, e.primary_project);
    if let Some(first_pr) = e.prs.first() {
        heading.push_str(&format!(" · #{}", first_pr));
    }

    // Tag line: #project/… (one — the entry is a single project), #pr/…, #topic/…
    let mut tags: Vec<String> = e
        .all_projects
        .iter()
        .map(|p| format!("#project/{}", p))
        .collect();
    for pr in &e.prs {
        tags.push(format!("#pr/{}", pr));
    }
    for t in &e.topics {
        tags.push(format!("#topic/{}", t));
    }
    let tag_line = tags.join(" ");

    // Meta line: reliable facts only — commits (file-scoped) + edit volume.
    let size = format!(
        "{} · {}",
        plural(e.files_count, "file"),
        plural(e.edits as usize, "edit")
    );
    let meta_line = if e.commits.is_empty() {
        size
    } else {
        format!("commits: {} · {}", e.commits.join(" "), size)
    };

    format!(
        "{}\n\n{}\n\n{}\n{}\n\n",
        heading, e.prose, tag_line, meta_line
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn test_render_week_with_pr_and_commits() {
        let entry = DevLogEntry {
            date: date(2026, 6, 17),
            primary_project: "dev-catchup".to_string(),
            all_projects: vec!["dev-catchup".to_string()],
            prs: vec![124],
            commits: vec!["abc1234".to_string(), "def5678".to_string()],
            topics: vec!["devlog".to_string()],
            prose: "Added a DevLog generation mode.".to_string(),
            files_count: 3,
            edits: 14,
        };

        let rendered = render_week("2026-W25", &[entry]);
        let expected = "# DevLog 2026-W25\n\
\n\
## 2026-06-17 · dev-catchup · #124\n\
\n\
Added a DevLog generation mode.\n\
\n\
#project/dev-catchup #pr/124 #topic/devlog\n\
commits: abc1234 def5678 · 3 files · 14 edits\n\
\n";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn test_render_entry_no_pr_no_commits() {
        let entry = DevLogEntry {
            date: date(2026, 6, 16),
            primary_project: "misc".to_string(),
            all_projects: vec!["misc".to_string()],
            prs: vec![],
            commits: vec![],
            topics: vec![],
            prose: "(no AI draft)".to_string(),
            files_count: 1,
            edits: 1,
        };

        let rendered = render_week("2026-W25", &[entry]);
        let expected = "# DevLog 2026-W25\n\
\n\
## 2026-06-16 · misc\n\
\n\
(no AI draft)\n\
\n\
#project/misc\n\
1 file · 1 edit\n\
\n";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn test_render_week_sorts_by_date_then_project() {
        let later = DevLogEntry {
            date: date(2026, 6, 17),
            primary_project: "zzz".to_string(),
            all_projects: vec!["zzz".to_string()],
            prs: vec![],
            commits: vec![],
            topics: vec![],
            prose: "second".to_string(),
            files_count: 1,
            edits: 1,
        };
        let earlier = DevLogEntry {
            date: date(2026, 6, 16),
            primary_project: "aaa".to_string(),
            all_projects: vec!["aaa".to_string()],
            prs: vec![],
            commits: vec![],
            topics: vec![],
            prose: "first".to_string(),
            files_count: 1,
            edits: 1,
        };

        let rendered = render_week("2026-W25", &[later, earlier]);
        let first_pos = rendered.find("first").unwrap();
        let second_pos = rendered.find("second").unwrap();
        assert!(first_pos < second_pos);
    }

    #[test]
    fn test_parse_claude_response_with_topics() {
        let resp = "Fixed the parser.\nIt now handles edge cases.\nTOPICS: parser, edge-cases";
        let (prose, topics) = parse_claude_response(resp);
        assert_eq!(prose, "Fixed the parser.\nIt now handles edge cases.");
        assert_eq!(topics, vec!["parser".to_string(), "edge-cases".to_string()]);
    }

    #[test]
    fn test_parse_claude_response_no_topics() {
        let resp = "Did a thing.";
        let (prose, topics) = parse_claude_response(resp);
        assert_eq!(prose, "Did a thing.");
        assert!(topics.is_empty());
    }

    #[test]
    fn test_kebab_normalization() {
        assert_eq!(kebab("Edge Cases"), "edge-cases");
        assert_eq!(kebab("foo_bar"), "foo-bar");
        assert_eq!(kebab("already-kebab"), "already-kebab");
    }

    #[test]
    fn test_week_path_iso_week() {
        // 2026-06-17 is in ISO week 25 of 2026.
        let p = week_path(date(2026, 6, 17));
        assert!(p.ends_with("2026-W25.md"));
    }
}
