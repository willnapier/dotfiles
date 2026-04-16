use anyhow::{bail, Context, Result};
use chrono::{Local, NaiveDate};
use clap::Parser;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Generate attendance report from PracticeForge session data")]
struct Cli {
    /// Override date (YYYY-MM-DD), defaults to today
    #[arg(long)]
    date: Option<NaiveDate>,

    /// Print message but don't save or notify
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug)]
enum Status {
    Attended,
    DnaLc,
    Cancelled,
    Pending,
}

#[derive(Debug)]
struct Entry {
    status: Status,
    content: String,
}

/// PracticeForge session file format.
#[derive(Deserialize)]
struct Session {
    #[serde(default)]
    clients: Vec<SessionClient>,
}

#[derive(Deserialize)]
struct SessionClient {
    id: String,
    #[serde(default)]
    time: String,
    #[serde(default)]
    end_time: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    rate_tag: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let date = cli.date.unwrap_or_else(|| Local::now().date_naive());
    let session_path = get_session_path(&date);

    let content = std::fs::read_to_string(&session_path)
        .with_context(|| format!("No session file for {}: {}", date, session_path.display()))?;

    let session: Session = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse session file: {}", session_path.display()))?;

    let entries = parse_session(&session);

    if entries.is_empty() {
        bail!("No clients in session for {}", date);
    }

    let message = format_message(&date, &entries);

    println!("{}", message);

    if !cli.dry_run {
        save_and_notify(&date, &message)?;
    }

    Ok(())
}

/// PracticeForge session file path.
fn get_session_path(date: &NaiveDate) -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"))
        .join(format!("practiceforge/session-{}.json", date.format("%Y-%m-%d")))
}

/// Parse a PracticeForge session into attendance entries.
fn parse_session(session: &Session) -> Vec<Entry> {
    session.clients.iter().map(|c| {
        let status = match c.status.as_str() {
            "done" => Status::Attended,
            "dna" => Status::DnaLc,
            "cancelled" => Status::Cancelled,
            _ => Status::Pending,
        };

        let mut content = c.id.clone();
        if !c.time.is_empty() {
            content.push(' ');
            content.push_str(&c.time);
        }
        if !c.rate_tag.is_empty() && c.rate_tag != "Private" && c.rate_tag != "self-pay" {
            content.push(' ');
            content.push_str(&c.rate_tag);
        }

        Entry { status, content }
    }).collect()
}

fn format_message(date: &NaiveDate, entries: &[Entry]) -> String {
    let day_str = date.format("%a %-e %b").to_string();

    let mut lines = vec![format!("{} — Attendance", day_str)];
    lines.push(String::new());

    let mut attended = 0u32;
    let mut dna_lc = 0u32;
    let mut cancelled = 0u32;
    let mut pending = 0u32;
    let mut insurer_count = 0u32;

    for entry in entries {
        let marker = match entry.status {
            Status::Attended => { attended += 1; "\u{2713}" }
            Status::DnaLc => { dna_lc += 1; "\u{2717}" }
            Status::Cancelled => { cancelled += 1; continue }  // Skip cancelled from report
            Status::Pending => { pending += 1; "?" }
        };

        if entry.content.contains("insurer") {
            insurer_count += 1;
        }

        lines.push(format!("{} {}", marker, entry.content));
    }

    lines.push(String::new());

    let total = attended + dna_lc + pending;
    let mut summary = vec![format!("{}/{} attended", attended, total)];

    if dna_lc > 0 { summary.push(format!("{} DNA/LC", dna_lc)); }
    if pending > 0 { summary.push(format!("{} unresolved", pending)); }
    if insurer_count > 0 { summary.push(format!("{} insurer", insurer_count)); }

    lines.push(summary.join(" \u{00b7} "));

    lines.join("\n")
}

fn save_and_notify(date: &NaiveDate, message: &str) -> Result<()> {
    let attendance_dir = dirs::home_dir()
        .expect("Could not find home directory")
        .join("Clinical/attendance");

    std::fs::create_dir_all(&attendance_dir)
        .with_context(|| format!("Failed to create {}", attendance_dir.display()))?;

    let filename = format!("{}.txt", date.format("%Y-%m-%d"));
    let path = attendance_dir.join(&filename);

    std::fs::write(&path, message)
        .with_context(|| format!("Failed to write {}", path.display()))?;

    eprintln!("Saved: {}", path.display());

    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg("display notification \"Attendance report saved\" with title \"Clinic Attendance\"")
            .spawn();
    }

    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("notify-send")
            .arg("Clinic Attendance")
            .arg("Attendance report saved")
            .spawn();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_client(id: &str, time: &str, status: &str, rate_tag: &str) -> SessionClient {
        SessionClient {
            id: id.to_string(),
            time: time.to_string(),
            end_time: String::new(),
            status: status.to_string(),
            rate_tag: rate_tag.to_string(),
        }
    }

    #[test]
    fn test_parse_session_attended() {
        let session = Session {
            clients: vec![
                make_client("EB88", "07:50", "done", "insurer"),
                make_client("BA90", "13:20", "done", "insurer"),
            ],
        };
        let entries = parse_session(&session);
        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0].status, Status::Attended));
        assert!(entries[0].content.contains("EB88"));
        assert!(entries[0].content.contains("insurer"));
    }

    #[test]
    fn test_parse_session_dna() {
        let session = Session {
            clients: vec![
                make_client("AO", "09:20", "dna", ""),
            ],
        };
        let entries = parse_session(&session);
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0].status, Status::DnaLc));
    }

    #[test]
    fn test_parse_session_cancelled_excluded() {
        let session = Session {
            clients: vec![
                make_client("EB88", "07:50", "done", "insurer"),
                make_client("JH91", "12:45", "cancelled", ""),
                make_client("BA90", "13:20", "done", ""),
            ],
        };
        let entries = parse_session(&session);
        let msg = format_message(
            &NaiveDate::from_ymd_opt(2026, 4, 16).unwrap(),
            &entries,
        );
        assert!(msg.contains("2/2 attended"));
        assert!(!msg.contains("JH91")); // Cancelled excluded
    }

    #[test]
    fn test_format_message_mixed() {
        let date = NaiveDate::from_ymd_opt(2026, 4, 16).unwrap();
        let entries = vec![
            Entry { status: Status::Attended, content: "AB79 07:45 insurer".to_string() },
            Entry { status: Status::DnaLc, content: "SZ84 09:35".to_string() },
            Entry { status: Status::Attended, content: "CC71 08:35".to_string() },
        ];
        let msg = format_message(&date, &entries);
        assert!(msg.contains("Thu 16 Apr"));
        assert!(msg.contains("2/3 attended"));
        assert!(msg.contains("1 DNA/LC"));
        assert!(msg.contains("1 insurer"));
    }
}
