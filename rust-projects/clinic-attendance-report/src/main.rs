use anyhow::{bail, Context, Result};
use chrono::{Local, NaiveDate};
use clap::Parser;
use regex::Regex;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Parse DayPage clinic block and open WhatsApp attendance report")]
struct Cli {
    /// Override date (YYYY-MM-DD), defaults to today
    #[arg(long)]
    date: Option<NaiveDate>,

    /// Print message but don't open WhatsApp
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug)]
enum Status {
    Attended,
    DnaLc,
    Pending,
}

#[derive(Debug)]
struct Entry {
    status: Status,
    content: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let date = cli.date.unwrap_or_else(|| Local::now().date_naive());
    let daypage_path = get_daypage_path(&date);

    let content = std::fs::read_to_string(&daypage_path)
        .with_context(|| format!("Failed to read DayPage: {}", daypage_path.display()))?;

    let entries = extract_and_parse(&content)?;

    if entries.is_empty() {
        bail!("No clinic entries found for {}", date);
    }

    let message = format_message(&date, &entries);

    println!("{}", message);

    if !cli.dry_run {
        copy_and_notify(&message)?;
    }

    Ok(())
}

fn get_daypage_path(date: &NaiveDate) -> PathBuf {
    dirs::home_dir()
        .expect("Could not find home directory")
        .join(format!(
            "Forge/NapierianLogs/DayPages/{}.md",
            date.format("%Y-%m-%d")
        ))
}

/// Extract the clinic:: block from the DayPage and parse each line.
fn extract_and_parse(content: &str) -> Result<Vec<Entry>> {
    let attended_re = Regex::new(r"^- \[x\] (.+)$").unwrap();
    let pending_re = Regex::new(r"^- \[ \] (.+)$").unwrap();

    let mut in_block = false;
    let mut entries = Vec::new();

    for line in content.lines() {
        if line.trim() == "clinic::" {
            in_block = true;
            continue;
        }

        if !in_block {
            continue;
        }

        // End of block: non-dash line after we've collected entries
        if !line.starts_with("- ") {
            if !entries.is_empty() || line.trim().is_empty() {
                break;
            }
            continue;
        }

        if let Some(caps) = attended_re.captures(line) {
            entries.push(Entry {
                status: Status::Attended,
                content: caps[1].to_string(),
            });
        } else if let Some(caps) = pending_re.captures(line) {
            entries.push(Entry {
                status: Status::Pending,
                content: caps[1].to_string(),
            });
        } else if line.starts_with("- ") {
            // Bare dash, no checkbox = DNA/late-cancel
            entries.push(Entry {
                status: Status::DnaLc,
                content: line[2..].to_string(),
            });
        }
    }

    if entries.is_empty() {
        bail!("No clinic:: block found");
    }

    Ok(entries)
}

fn format_message(date: &NaiveDate, entries: &[Entry]) -> String {
    let day_str = date.format("%a %-e %b").to_string();

    let mut lines = vec![format!("{} — Attendance", day_str)];
    lines.push(String::new());

    let mut attended = 0u32;
    let mut dna_lc = 0u32;
    let mut pending = 0u32;
    let mut insurer_count = 0u32;

    for entry in entries {
        let marker = match entry.status {
            Status::Attended => {
                attended += 1;
                "\u{2713}"
            }
            Status::DnaLc => {
                dna_lc += 1;
                "\u{2717}"
            }
            Status::Pending => {
                pending += 1;
                "?"
            }
        };

        if entry.content.contains("insurer") {
            insurer_count += 1;
        }

        lines.push(format!("{} {}", marker, entry.content));
    }

    lines.push(String::new());

    let total = attended + dna_lc + pending;
    let mut summary = vec![format!("{}/{} attended", attended, total)];

    if dna_lc > 0 {
        summary.push(format!("{} DNA/LC", dna_lc));
    }
    if pending > 0 {
        summary.push(format!("{} unresolved", pending));
    }
    if insurer_count > 0 {
        summary.push(format!("{} insurer", insurer_count));
    }

    lines.push(summary.join(" \u{00b7} "));

    lines.join("\n")
}

fn copy_and_notify(message: &str) -> Result<()> {
    // Copy readable message to clipboard
    #[cfg(target_os = "macos")]
    {
        let mut child = std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .context("Failed to run pbcopy")?;
        if let Some(stdin) = child.stdin.as_mut() {
            use std::io::Write;
            stdin.write_all(message.as_bytes()).context("Failed to write to pbcopy")?;
        }
        child.wait().context("pbcopy failed")?;

        std::process::Command::new("osascript")
            .arg("-e")
            .arg("display notification \"Attendance report copied to clipboard — paste in WhatsApp\" with title \"Clinic Attendance Report\"")
            .spawn()
            .context("Failed to send notification")?;
    }

    #[cfg(target_os = "linux")]
    {
        let mut child = std::process::Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .context("Failed to run wl-copy")?;
        if let Some(stdin) = child.stdin.as_mut() {
            use std::io::Write;
            stdin.write_all(message.as_bytes()).context("Failed to write to wl-copy")?;
        }
        child.wait().context("wl-copy failed")?;

        std::process::Command::new("notify-send")
            .arg("Clinic Attendance Report")
            .arg("Attendance report copied to clipboard — paste in WhatsApp")
            .spawn()
            .context("Failed to send notification")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn test_parse_attended() {
        let content = "clinic::\n- [x] EB88 07:50 insurer\n- [x] BA90 13:20 insurer\n";
        let entries = extract_and_parse(content).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0].status, Status::Attended));
        assert_eq!(entries[0].content, "EB88 07:50 insurer");
    }

    #[test]
    fn test_parse_dna() {
        let content = "clinic::\n- AO+AO 09:20 missed again\n- AA 20:00 insurer\n";
        let entries = extract_and_parse(content).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0].status, Status::DnaLc));
        assert_eq!(entries[0].content, "AO+AO 09:20 missed again");
    }

    #[test]
    fn test_parse_pending() {
        let content = "clinic::\n- [ ] PD60 10:00\n- [ ] SM60 12:30\n";
        let entries = extract_and_parse(content).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(matches!(entries[0].status, Status::Pending));
    }

    #[test]
    fn test_mixed_statuses() {
        let content = "\
clinic::
- [x] EB88 07:50 insurer
- AO+AO 09:20
- [x] BA90 13:20 insurer
- [x] CT71 17:30 insurer
- AA 20:00 insurer

dev:: some other block
";
        let entries = extract_and_parse(content).unwrap();
        assert_eq!(entries.len(), 5);

        let attended: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e.status, Status::Attended))
            .collect();
        let dna: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e.status, Status::DnaLc))
            .collect();

        assert_eq!(attended.len(), 3);
        assert_eq!(dna.len(), 2);
    }

    #[test]
    fn test_format_message() {
        let date = NaiveDate::from_ymd_opt(2026, 2, 3).unwrap();
        let entries = vec![
            Entry {
                status: Status::Attended,
                content: "EB88 07:50 insurer".to_string(),
            },
            Entry {
                status: Status::DnaLc,
                content: "AO+AO 09:20".to_string(),
            },
            Entry {
                status: Status::Attended,
                content: "BA90 13:20 insurer".to_string(),
            },
        ];

        let msg = format_message(&date, &entries);
        assert!(msg.contains("Tue 3 Feb"));
        assert!(msg.contains("2/3 attended"));
        assert!(msg.contains("1 DNA/LC"));
        assert!(msg.contains("2 insurer"));
    }

    #[test]
    fn test_no_clinic_block() {
        let content = "# 2026-02-03\n\ndev:: some stuff\n";
        let result = extract_and_parse(content);
        assert!(result.is_err());
    }
}
