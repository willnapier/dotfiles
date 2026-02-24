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
    Deferred,
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
        let line = line.trim_start();
        if line == "clinic::" {
            in_block = true;
            continue;
        }

        if !in_block {
            continue;
        }

        // Skip meta-lines (reminders about the tool itself)
        if line.contains("clinic-attendance-report") {
            continue;
        }

        // Deferred: line without list marker but containing ->
        if !line.starts_with("- ") && line.contains("->") {
            entries.push(Entry {
                status: Status::Deferred,
                content: line.trim().to_string(),
            });
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
        } else if line.contains("->") {
            // Bare dash with -> = also deferred
            entries.push(Entry {
                status: Status::Deferred,
                content: line[2..].to_string(),
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
    let mut deferred = 0u32;
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
            Status::Deferred => {
                deferred += 1;
                "\u{2192}"
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

    // Deferred excluded from total — slot didn't exist that day
    let total = attended + dna_lc + pending;
    let mut summary = vec![format!("{}/{} attended", attended, total)];

    if dna_lc > 0 {
        summary.push(format!("{} DNA/LC", dna_lc));
    }
    if deferred > 0 {
        summary.push(format!("{} deferred", deferred));
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
    fn test_parse_deferred_no_prefix() {
        // Actual syntax: plain text (no - prefix) with ->
        let content = "clinic::\n- [x] EB88 07:50\nCC71 11:05 insurer ->\n- [x] BA90 13:20\n";
        let entries = extract_and_parse(content).unwrap();
        assert_eq!(entries.len(), 3);
        assert!(matches!(entries[1].status, Status::Deferred));
        assert_eq!(entries[1].content, "CC71 11:05 insurer ->");
        // Entries after deferred line are still parsed
        assert!(matches!(entries[2].status, Status::Attended));
    }

    #[test]
    fn test_parse_deferred_with_prefix() {
        // Also support - prefix with -> (alternative toggle state)
        let content = "clinic::\n- [x] EB88 07:50\n- CC71 11:05 insurer ->\n- [x] BA90 13:20\n";
        let entries = extract_and_parse(content).unwrap();
        assert_eq!(entries.len(), 3);
        assert!(matches!(entries[1].status, Status::Deferred));
    }

    #[test]
    fn test_deferred_excluded_from_total() {
        let date = NaiveDate::from_ymd_opt(2026, 2, 5).unwrap();
        let entries = vec![
            Entry {
                status: Status::Attended,
                content: "EB88 07:50".to_string(),
            },
            Entry {
                status: Status::Deferred,
                content: "CC71 11:05 insurer ->".to_string(),
            },
            Entry {
                status: Status::Attended,
                content: "BA90 13:20".to_string(),
            },
        ];
        let msg = format_message(&date, &entries);
        // 2 attended out of 2 (deferred excluded from denominator)
        assert!(msg.contains("2/2 attended"));
        assert!(msg.contains("1 deferred"));
    }

    #[test]
    fn test_real_daypage_with_deferred() {
        // Mirrors 2026-02-05 actual data
        let content = "\
clinic::
- [x] AB79 07:45 insurer
- [x] ER92 08:35
- [x] SZ84 09:35
- EA 11:10
CC71 12:00 insurer ->
- [x] JH91 12:45
- [x] HH92 13:35 insurer
- [x] VM78 14:25
- FH 15:15 insurer
- [ ] BT07 16:05

## Backlinks
";
        let entries = extract_and_parse(content).unwrap();
        assert_eq!(entries.len(), 10);

        let attended: Vec<_> = entries.iter().filter(|e| matches!(e.status, Status::Attended)).collect();
        let dna: Vec<_> = entries.iter().filter(|e| matches!(e.status, Status::DnaLc)).collect();
        let deferred: Vec<_> = entries.iter().filter(|e| matches!(e.status, Status::Deferred)).collect();
        let pending: Vec<_> = entries.iter().filter(|e| matches!(e.status, Status::Pending)).collect();

        assert_eq!(attended.len(), 6);
        assert_eq!(dna.len(), 2);
        assert_eq!(deferred.len(), 1);
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn test_indented_lines_still_parsed() {
        let content = "\
clinic::
- [x] EB88 07:50 insurer
- [x] AO+AO 09:20
- [x] BA90 10:10 insurer
- [x] MPS94 11:30 insurer
- [x] MA93 12:20 insurer
 - BT07 15:50 cancelled
- [x] CT71 17:30 insurer
- [x] DV91 18:15
- [x] NP+AP 19:05

dev:: some other block
";
        let entries = extract_and_parse(content).unwrap();
        assert_eq!(entries.len(), 9);

        let attended: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e.status, Status::Attended))
            .collect();
        let dna: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e.status, Status::DnaLc))
            .collect();

        assert_eq!(attended.len(), 8);
        assert_eq!(dna.len(), 1);
        assert!(dna[0].content.contains("BT07"));
    }

    #[test]
    fn test_no_clinic_block() {
        let content = "# 2026-02-03\n\ndev:: some stuff\n";
        let result = extract_and_parse(content);
        assert!(result.is_err());
    }
}
