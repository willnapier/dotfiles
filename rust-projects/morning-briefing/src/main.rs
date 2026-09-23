//! morning-briefing — overnight-state digest written to
//! `~/Assistants/shared/MORNING-BRIEFING.md`. Rust port (2026-09-23) of the
//! bash script of the same name; fired daily at 07:30 by
//! `com.williamnapier.morning-briefing` (Mac only), whose ExecStart path is
//! unchanged. Disable with
//! `launchctl bootout gui/$(id -u)/com.williamnapier.morning-briefing`.
//!
//! The digest is a DERIVED dashboard: every section is a formatted view of a
//! durable source (gmpull.log, package-drift-check, shared/audits/*, gh run
//! list, mailcurator/*.jsonl). It is the sole record of nothing, so the file
//! keeps only the two most recent entries — today's and the first prior
//! `<!-- briefing-entry -->` block.
//!
//! Semantic differences from the script:
//! - A digest that cannot be written is now a failure: exit 1, `last_error`
//!   in the recorded outcome (`~/.local/state/watchers/morning-briefing.json`),
//!   and a `notify-user --tool morning-briefing` alert. The script always
//!   exited 0. Every *gathering* failure stays a bullet inside the digest, as
//!   the script's fallbacks did; it never fails the run.
//! - The gmpull counters count over the current log file, as before. Now that
//!   logs may be capped by `logkeep` (D2-24), that window is "since the log
//!   last rolled", so the bullet says "(since log start)" instead of
//!   "(log lifetime)".
//! - The generated-at stamp prints the UTC offset (`+01:00`) where `date +%Z`
//!   printed the zone abbreviation (`BST`).
//! - The gh JSON is parsed here (serde_json) instead of by a jq expression;
//!   the rendered line is the same.
//!
//! Exit status: 0 = digest written; 1 = digest could not be written.

mod exec;
mod outcome;

use clap::Parser;
use exec::Exec;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "morning-briefing", version, about = "Overnight-state digest → ~/Assistants/shared/MORNING-BRIEFING.md (rolling two-day window)")]
struct Cli {}

const NAME: &str = "morning-briefing";
/// Daily at 07:30.
const INTERVAL_SECS: u64 = 86_400;
const MARKER: &str = "<!-- briefing-entry -->";
const CI_REPOS: [&str; 2] = ["willnapier/practiceforge", "willnapier/tm3-diary-capture"];
const AUDIT_PROJECTS: [&str; 2] = ["mailforge", "practiceforge"];

// ── gmail pull ─────────────────────────────────────────────────────────

/// (pulls, idle-skipped, errors, transient auth-refresh blips) over the whole
/// log text — the four `grep -c` counters of the script.
pub fn gmail_counters(log: &str) -> (usize, usize, usize, usize) {
    let mut pulls = 0;
    let mut skips = 0;
    let mut errs = 0;
    let mut blips = 0;
    for line in log.lines() {
        if line.contains("incremental pull complete") {
            pulls += 1;
        }
        if line.starts_with("[mailcurator] Activity check: skipping") {
            skips += 1;
        }
        if has_nonzero_errored(line) {
            errs += 1;
        }
        if line.contains("pizauth show gmail failed") {
            blips += 1;
        }
    }
    (pulls, skips, errs, blips)
}

/// `errored=[1-9]` — any occurrence on the line.
fn has_nonzero_errored(line: &str) -> bool {
    line.match_indices("errored=").any(|(i, m)| {
        line[i + m.len()..].chars().next().map(|c| ('1'..='9').contains(&c)).unwrap_or(false)
    })
}

/// `du -sh`-style human size: bytes → "512B", "3.4K", "120M", "1.2G".
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes}B")
    } else if v < 10.0 {
        format!("{v:.1}{}", UNITS[u])
    } else {
        format!("{}{}", v.round() as u64, UNITS[u])
    }
}

/// Count and total size of regular files under `dir` (recursive, no symlink following).
fn walk_files(dir: &Path) -> (usize, u64) {
    let mut count = 0;
    let mut bytes = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(e.path());
            } else if ft.is_file() {
                count += 1;
                bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    (count, bytes)
}

fn tail_lines(text: &str, n: usize) -> Vec<&str> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].to_vec()
}

fn indent4(lines: &[&str]) -> String {
    lines.iter().map(|l| format!("    {l}")).collect::<Vec<_>>().join("\n")
}

fn mtime_hm(path: &Path) -> String {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

/// Pure rendering of the Gmail section from already-gathered facts.
pub fn gmail_section(count: usize, size: &str, last_tick: &str, log_text: &str) -> String {
    let (pulls, skips, errs, blips) = gmail_counters(log_text);
    let tail = indent4(&tail_lines(log_text, 4));
    format!(
        "- Maildir: **{count} messages** in {size}\n- Last tick: {last_tick}\n- Soak (since log start): **{pulls}** completed pulls, **{skips}** idle-skipped, **{errs}** errors, **{blips}** transient auth-refresh blips\n- Tail:\n\n```\n{tail}\n```"
    )
}

fn gather_gmail(home: &Path) -> String {
    let dir = home.join("Mail/gmail-rs");
    if !dir.is_dir() {
        return "- gmail-rs dir missing".into();
    }
    let log = home.join("Library/Logs/gmpull.log");
    let (count, bytes) = walk_files(&dir);
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    gmail_section(count, &human_size(bytes), &mtime_hm(&log), &text)
}

// ── package drift ──────────────────────────────────────────────────────

fn gather_drift(exec: &dyn Exec) -> String {
    let r = exec.run("package-drift-check", &[]);
    if r.exit_code == 127 {
        "- package-drift-check not on PATH".into()
    } else if !r.ok() {
        "- drift check exited non-zero".into()
    } else {
        r.stdout.trim_end_matches('\n').to_string()
    }
}

// ── frontend audits ────────────────────────────────────────────────────

fn gather_audits(home: &Path) -> String {
    let dir = home.join("Assistants/shared/audits");
    let mut s = String::new();
    for proj in AUDIT_PROJECTS {
        let f = dir.join(format!("{proj}-frontend-audit-2026-05-02.md"));
        match std::fs::read_to_string(&f) {
            Ok(t) => s.push_str(&format!("- {proj}: **ready** ({} lines) — `{}`\n", t.lines().count(), f.display())),
            Err(_) => s.push_str(&format!("- {proj}: not yet written — agent may still be running\n")),
        }
    }
    if s.is_empty() {
        "- (no audit files configured)".into()
    } else {
        s
    }
}

// ── CI ─────────────────────────────────────────────────────────────────

/// Render one `gh run list --limit 1 --json …` result as the script's jq did.
/// `None` when the JSON is unparseable (the script's line was then empty and skipped).
pub fn ci_line(repo: &str, json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let arr = v.as_array()?;
    let Some(run) = arr.first() else {
        return Some(format!("- {repo}: no workflow runs"));
    };
    let s = |k: &str| run.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let conclusion = match run.get("conclusion") {
        Some(serde_json::Value::String(c)) => c.clone(),
        _ => "in progress".into(),
    };
    Some(format!("- {repo}: {conclusion} — {} ({}, {})", s("name"), s("headBranch"), s("createdAt")))
}

fn gather_ci(exec: &dyn Exec) -> String {
    let mut lines = vec![];
    for repo in CI_REPOS {
        let r = exec.run("gh", &["run", "list", "--repo", repo, "--limit", "1", "--json", "conclusion,name,createdAt,headBranch"]);
        if r.exit_code == 127 {
            return "- gh not on PATH".into();
        }
        if !r.ok() {
            continue;
        }
        if let Some(l) = ci_line(repo, &r.stdout) {
            lines.push(l);
        }
    }
    if lines.is_empty() {
        "- gh CLI returned nothing (auth or no recent runs)".into()
    } else {
        lines.join("\n")
    }
}

// ── mailcurator ────────────────────────────────────────────────────────

fn gather_mailcurator(home: &Path) -> String {
    let dir = home.join(".local/share/mailcurator");
    if !dir.is_dir() {
        return "- mailcurator dir missing".into();
    }
    let del = std::fs::read_to_string(dir.join("deletions.jsonl")).unwrap_or_default();
    let deliv = std::fs::read_to_string(dir.join("deliveries.jsonl")).unwrap_or_default();
    let recent = indent4(&tail_lines(&del, 3));
    format!(
        "- deletions.jsonl lines: **{}**, deliveries.jsonl lines: **{}**\n- Last 3 deletions:\n\n```\n{recent}\n```",
        del.lines().count(),
        deliv.lines().count()
    )
}

// ── compose ────────────────────────────────────────────────────────────

pub struct Sections {
    pub gmail: String,
    pub drift: String,
    pub audits: String,
    pub ci: String,
    pub mailcurator: String,
}

/// The digest body, exactly as the script's heredoc laid it out (trailing
/// newlines stripped, as `$(cat <<EOF …)` did).
pub fn compose(now: &str, s: &Sections) -> String {
    let body = format!(
        "**Morning briefing — generated {now}**\n\n### Gmail pull\n{}\n\n### Package drift\n{}\n\n### Frontend audits (overnight)\n{}\n\n### CI\n{}\n\n### mailcurator\n{}",
        s.gmail, s.drift, s.audits, s.ci, s.mailcurator
    );
    body.trim_end_matches('\n').to_string()
}

/// The first marker block of the prior file — from the first `MARKER` line
/// (inclusive) up to, not including, the second — each line newline-terminated.
/// This is the awk `$0==m{c++; if(c==2) exit} c>=1{print}` of the script.
pub fn prior_first_block(prior: &str) -> String {
    let mut out = String::new();
    let mut seen = 0;
    for line in prior.lines() {
        if line == MARKER {
            seen += 1;
            if seen == 2 {
                break;
            }
        }
        if seen >= 1 {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The whole file: header, today's block, the prior first block carried forward.
pub fn render_file(now: &str, body: &str, prior: Option<&str>) -> String {
    let mut f = String::from("# Morning Briefing\n\n");
    f.push_str("_Overwritten daily by `~/dotfiles/rust-projects/morning-briefing` — rolling 2-day window. A derived dashboard, not a record: every figure is a live view of a durable source (gmpull.log, package-drift-check, shared/audits/, gh run list, mailcurator/*.jsonl)._\n\n");
    f.push_str(&format!("{MARKER}\n## {now}\n\n{body}\n"));
    if let Some(p) = prior {
        f.push_str(&prior_first_block(p));
    }
    f
}

fn briefing_path(home: &Path) -> PathBuf {
    home.join("Assistants/shared/MORNING-BRIEFING.md")
}

/// tmp + rename, as the script's `> file.tmp && mv -f`.
fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = PathBuf::from(format!("{}.tmp", path.display()));
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

/// Gather everything, compose, write. Returns the exit code.
fn run(exec: &dyn Exec, home: &Path, now: &str) -> i32 {
    let started = outcome::now_rfc3339();
    let sections = Sections {
        gmail: gather_gmail(home),
        drift: gather_drift(exec),
        audits: gather_audits(home),
        ci: gather_ci(exec),
        mailcurator: gather_mailcurator(home),
    };
    let body = compose(now, &sections);
    let path = briefing_path(home);
    let prior = std::fs::read_to_string(&path).ok();
    let content = render_file(now, &body, prior.as_deref());
    let result = write_atomic(&path, &content);
    let (code, last_error) = match &result {
        Ok(()) => (0, None),
        Err(e) => {
            let msg = format!("could not write {}: {e}", path.display());
            eprintln!("morning-briefing: {msg}");
            let _ = exec.run("notify-user", &["--tool", NAME, "morning-briefing failed", &msg]);
            (1, Some(msg))
        }
    };
    let o = outcome::Outcome {
        name: NAME,
        interval_secs: INTERVAL_SECS,
        started_at: started,
        last_action: if code == 0 { Some(format!("wrote {}", path.display())) } else { None },
        actions: if code == 0 { 1 } else { 0 },
        last_error,
    };
    if let Err(e) = outcome::record(&outcome::state_dir(home), &o) {
        eprintln!("morning-briefing: heartbeat write failed: {e}");
    }
    code
}

fn main() {
    let _ = Cli::parse();
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/Users/williamnapier"));
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M %Z").to_string();
    std::process::exit(run(&exec::Real, &home, &now));
}

#[cfg(test)]
mod tests {
    use super::*;
    use exec::{CmdResult, Fake};

    const LOG: &str = "2026-09-22 01:00 incremental pull complete fetched=3 errored=0\n[mailcurator] Activity check: skipping (idle 5h)\n2026-09-22 02:00 pull progress errored=2 of 9\nwarn: pizauth show gmail failed (token refresh in progress)\n2026-09-22 03:00 incremental pull complete fetched=0 errored=0\n";

    #[test]
    fn gmail_counters_match_the_four_greps() {
        assert_eq!(gmail_counters(LOG), (2, 1, 1, 1));
        assert_eq!(gmail_counters(""), (0, 0, 0, 0));
        assert!(!has_nonzero_errored("errored=0 errored="), "0 and a bare key are not errors");
        assert!(has_nonzero_errored("x errored=0 y errored=7"));
    }

    #[test]
    fn human_size_is_du_like() {
        assert_eq!(human_size(512), "512B");
        assert_eq!(human_size(3 * 1024 + 400), "3.4K");
        assert_eq!(human_size(120 * 1024 * 1024), "120M");
        assert_eq!(human_size(1_300_000_000), "1.2G");
    }

    #[test]
    fn ci_line_renders_like_the_jq_expression() {
        assert_eq!(ci_line("r/a", "[]").unwrap(), "- r/a: no workflow runs");
        let ok = r#"[{"conclusion":"success","name":"CI","createdAt":"2026-09-22T20:00:00Z","headBranch":"main"}]"#;
        assert_eq!(ci_line("r/a", ok).unwrap(), "- r/a: success — CI (main, 2026-09-22T20:00:00Z)");
        let running = r#"[{"conclusion":null,"name":"CI","createdAt":"t","headBranch":"b"}]"#;
        assert_eq!(ci_line("r/a", running).unwrap(), "- r/a: in progress — CI (b, t)");
        assert_eq!(ci_line("r/a", "not json"), None);
    }

    #[test]
    fn gather_ci_fallbacks() {
        let mut f = Fake::default();
        assert_eq!(gather_ci(&f), "- gh not on PATH", "unscripted gh is exit 127");
        f.respond("gh", &["run", "list", "--repo", CI_REPOS[0], "--limit", "1", "--json", "conclusion,name,createdAt,headBranch"], CmdResult::failure(1, "auth"));
        f.respond("gh", &["run", "list", "--repo", CI_REPOS[1], "--limit", "1", "--json", "conclusion,name,createdAt,headBranch"], CmdResult::failure(1, "auth"));
        assert_eq!(gather_ci(&f), "- gh CLI returned nothing (auth or no recent runs)");
        f.respond("gh", &["run", "list", "--repo", CI_REPOS[1], "--limit", "1", "--json", "conclusion,name,createdAt,headBranch"], CmdResult::success("[]"));
        assert_eq!(gather_ci(&f), "- willnapier/tm3-diary-capture: no workflow runs");
    }

    #[test]
    fn drift_fallbacks() {
        let mut f = Fake::default();
        assert_eq!(gather_drift(&f), "- package-drift-check not on PATH");
        f.respond("package-drift-check", &[], CmdResult::failure(1, ""));
        assert_eq!(gather_drift(&f), "- drift check exited non-zero");
        f.respond("package-drift-check", &[], CmdResult::success("- all in sync\n"));
        assert_eq!(gather_drift(&f), "- all in sync");
    }

    fn sections() -> Sections {
        Sections { gmail: "- g".into(), drift: "- d".into(), audits: "- a\n".into(), ci: "- c".into(), mailcurator: "- m".into() }
    }

    #[test]
    fn compose_lays_out_the_script_heredoc() {
        let b = compose("2026-09-23 07:30 +01:00", &sections());
        assert_eq!(
            b,
            "**Morning briefing — generated 2026-09-23 07:30 +01:00**\n\n### Gmail pull\n- g\n\n### Package drift\n- d\n\n### Frontend audits (overnight)\n- a\n\n\n### CI\n- c\n\n### mailcurator\n- m"
        );
    }

    #[test]
    fn prior_first_block_is_the_awk_window() {
        let prior = format!("# Morning Briefing\n\nintro\n\n{MARKER}\n## day2\n\nbody2\n{MARKER}\n## day1\n\nbody1\n");
        assert_eq!(prior_first_block(&prior), format!("{MARKER}\n## day2\n\nbody2\n"));
        assert_eq!(prior_first_block("no marker here\n"), "");
        assert_eq!(prior_first_block(&format!("{MARKER}\nonly\n")), format!("{MARKER}\nonly\n"));
    }

    #[test]
    fn render_file_keeps_exactly_two_entries_green_control() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        std::fs::create_dir_all(home.join("Assistants/shared")).unwrap();
        let f = Fake::default();
        assert_eq!(run(&f, home, "day1"), 0);
        assert_eq!(run(&f, home, "day2"), 0);
        assert_eq!(run(&f, home, "day3"), 0);
        let text = std::fs::read_to_string(briefing_path(home)).unwrap();
        assert_eq!(text.matches(MARKER).count(), 2, "{text}");
        assert!(text.contains("## day3\n") && text.contains("## day2\n") && !text.contains("## day1\n"), "{text}");
        assert!(text.starts_with("# Morning Briefing\n\n_Overwritten daily by `~/dotfiles/rust-projects/morning-briefing`"));
        assert!(text.contains("- gmail-rs dir missing") && text.contains("- mailcurator dir missing") && text.contains("- gh not on PATH"));
        assert!(text.contains("- mailforge: not yet written — agent may still be running\n- practiceforge: not yet written"));
        assert!(!briefing_path(home).with_extension("md.tmp").exists());
        let hb = std::fs::read_to_string(outcome::state_dir(home).join("morning-briefing.json")).unwrap();
        assert!(hb.contains("\"last_error\":null") && hb.contains("\"interval_secs\":86400"), "{hb}");
        assert!(f.calls.borrow().iter().all(|c| !c.starts_with("notify-user")), "no alert on a healthy run");
    }

    #[test]
    fn gmail_section_reads_synthetic_maildir_and_log() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        std::fs::create_dir_all(home.join("Mail/gmail-rs/INBOX/cur")).unwrap();
        std::fs::write(home.join("Mail/gmail-rs/INBOX/cur/a"), vec![b'x'; 2048]).unwrap();
        std::fs::write(home.join("Mail/gmail-rs/INBOX/cur/b"), vec![b'x'; 2048]).unwrap();
        std::fs::create_dir_all(home.join("Library/Logs")).unwrap();
        std::fs::write(home.join("Library/Logs/gmpull.log"), LOG).unwrap();
        let s = gather_gmail(home);
        assert!(s.starts_with("- Maildir: **2 messages** in 4.0K\n- Last tick: 20"), "{s}");
        assert!(s.contains("- Soak (since log start): **2** completed pulls, **1** idle-skipped, **1** errors, **1** transient auth-refresh blips\n- Tail:\n\n```\n    [mailcurator]"), "{s}");
        assert!(s.ends_with("errored=0\n```"), "{s}");
    }

    #[test]
    fn unwritable_output_is_a_failure_red_control() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        // Assistants/shared does not exist → the tmp write fails.
        let f = Fake::default();
        assert_eq!(run(&f, home, "day1"), 1);
        assert!(f.calls.borrow().iter().any(|c| c.starts_with("notify-user --tool morning-briefing")), "{:?}", f.calls.borrow());
        let hb = std::fs::read_to_string(outcome::state_dir(home).join("morning-briefing.json")).unwrap();
        assert!(hb.contains("\"last_error\":\"could not write"), "{hb}");
        assert!(hb.contains("\"actions\":0"), "{hb}");
    }
}
