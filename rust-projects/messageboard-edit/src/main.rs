//! messageboard-edit — atomic Messageboard editing, one writer per file.
//!
//! Rust port (2026-09-23) of the Nushell script of the same name, with one
//! structural change. The board lives in a Syncthing folder, and Syncthing
//! is only safe when each file has a single writer: two hosts editing
//! `MESSAGEBOARD.md` within one sync window produced `.sync-conflict` copies
//! and stalled the Assistants follower on 2026-09-22. So:
//!
//! - Every host writes ONLY its own `MESSAGEBOARD.<host>.md`,
//!   `MESSAGEBOARD-ARCHIVE.<host>.md` and `MESSAGEBOARD-TOMBSTONES.<host>.tsv`,
//!   where `<host>` is the short lower-case hostname (`nimbini`,
//!   `williams-macbook-air`).
//! - The old `MESSAGEBOARD.md` / `MESSAGEBOARD-ARCHIVE.md` are LEGACY and
//!   read-only from now on. Migration is zero-step: readers merge legacy +
//!   every per-host file.
//! - A section is identified by a stable id: the first 12 hex of the SHA-256
//!   of its text (`### date — device` header + body). Removing or archiving
//!   a section you own edits your own file. Removing or archiving a section
//!   in the legacy file or another host's file appends a TOMBSTONE
//!   (`<id>\t<iso-ts>\t<host>\t<reason>`) to your tombstones file — append
//!   only, one writer — and, for archive-*, appends the section text to YOUR
//!   archive file. Readers hide any section whose id appears in any host's
//!   tombstones. `archive-containing` therefore works from either host with
//!   no cross-host write.
//! - `render` prints the merged live view with no decoration (`--json` for
//!   the structured form); `ai-brief` and `forum` read the board through it.
//!
//! The CLI, the section format (`### YYYY-MM-DD — device`, blank line,
//! content, `---` separators) and the "✅ …" confirmation lines are those of
//! the script, so every caller keeps working unchanged.

use anyhow::{Context, Result};
use chrono::{Local, NaiveDate};
use clap::{Parser, ValueEnum};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const LIVE_HEADER: &str = "# Messageboard

> **Usage**: This header persists. Ordinary inbox messages below get
> cleared by the responsible receiver once addressed (action taken, information
> noted, or question resolved). **Design-forum pointers are attention gateways,
> not per-assistant inbox tasks:** follow them to `design-forum/INDEX.md`; do not
> clear them merely because one assistant has participated. Retire or supersede
> them only when they no longer point to relevant open forum work.
>
> **Window:** live inbox only — not a ship chronicle. Short pointers for
> cross-machine work still belong here (one line + filename). Long shipped-work
> narratives go to `ASSISTANT-HANDOFF.md` + named docs. `forum acknowledge`
> archives matching FORUM COMPLETE notices; unread state lives in `forum inbox`.
> **Clearing:** `messageboard-edit unclaimed 7` reports what nobody has cleared
> (report-only — unclaimed is NOT the same as stale; most stranded items are
> real work awaiting a machine that has not run a session). Verify, then
> `archive-containing <unique text>`.
> **Work claims (kernel rule, 2026-09-23):** before the first mutation of any
> batch, `messageboard-edit claims` (open claims across hosts), then one insert
> `CLAIMED <id> — host, paths, services cycled, not touching …`; finish with
> one `DONE <id> — …` insert. Overlap with an open claim is raised, not worked
> around. Reading and diagnosis need no claim.
> **Automatic sweep:** `forum sweep --older-than 14` runs daily on nimbini
> (`messageboard-sweep.timer`, 07:20). It archives what forum state proves
> finished — acknowledged FORUM COMPLETE notices, FORUM OPEN pointers to
> threads no longer open, work orders whose thread has an
> `### Implementation receipt` — plus ordinary items older than 14 days.
> FORUM OPEN pointers and unreceipted work orders are exempt from the age
> ceiling. A live item must therefore be re-posted or moved to
> `ASSISTANT-HANDOFF.md` before it turns 14 days old. `cross-machine-sync-check` flags anything past that same ceiling as drift.
> Older entries: [`MESSAGEBOARD-ARCHIVE.md`](MESSAGEBOARD-ARCHIVE.md).
> Last archive: 2026-09-13. Newest at top. Format: `### YYYY-MM-DD — device`
>
> **One writer per file (2026-09-23).** Each host writes only its own
> `MESSAGEBOARD.<host>.md`; `MESSAGEBOARD.md` is the legacy, read-only file.
> `messageboard-edit show` / `render` give the merged view; sections owned by
> another host are retired by tombstone, not by editing its file.
> **Sync is automatic.** `messageboard-edit` triggers a Syncthing rescan.
> Raw Edit/Write is blocked; never hand-edit these files.
>
> **Automated daily briefings do NOT belong here.** They bloated the
> board to 414 KB once already (cleared 2026-06-03). Status snapshots go
> to their own file or the generator self-prunes — never the board.

---

## Messages";

const ARCHIVE_HEADER: &str = "# Messageboard archive

Historical inbox items, newest-first. The live file is the current inbox only — see [`MESSAGEBOARD.md`](MESSAGEBOARD.md).

> Last archive: 2026-09-13 (manual sweep of shipped narratives, decided-forum pointers, receipted work orders, and resolved ordinary items from 2026-08-21 through 2026-09-11; `forum sweep` now runs daily on nimbini). Search this file; do not grow the live inbox.
> Since 2026-09-23 each host archives into its own `MESSAGEBOARD-ARCHIVE.<host>.md`; this file is the legacy archive.

## Messages";

/// Read-only since 2026-09-23; its archive `MESSAGEBOARD-ARCHIVE.md` likewise.
const LEGACY_LIVE: &str = "MESSAGEBOARD.md";
const BLOAT_ITEMS: usize = 25;
const BLOAT_BYTES: usize = 40_000;

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq, Debug)]
#[value(rename_all = "kebab-case")]
enum Action {
    Insert,
    RemoveSection,
    RemoveContaining,
    ArchiveContaining,
    ArchiveUnless,
    ArchiveOlderThan,
    Unclaimed,
    Claims,
    SweepStubs,
    RefreshHeader,
    Show,
    Render,
}

#[derive(Parser)]
#[command(
    name = "messageboard-edit",
    version,
    about = "Atomic Messageboard editing — one writer per file, merged view for readers",
    long_about = "Each host writes only its own MESSAGEBOARD.<host>.md; MESSAGEBOARD.md is legacy and read-only. \
Sections you own are edited in place; sections owned by the legacy file or another host are retired by a tombstone \
in MESSAGEBOARD-TOMBSTONES.<host>.tsv (and copied into your archive for archive-*). show/render merge every file \
and hide tombstoned sections.\n\n\
Actions:\n  insert \"message\"\n  remove-section \"### 2026-01-11 — device\"\n  remove-containing \"unique text\"\n  \
archive-containing \"unique text\"\n  archive-unless \"keep needle\" \"keep needle\" ...\n  archive-older-than 30\n  \
unclaimed 7           # report-only: what nobody has cleared\n  claims                # open CLAIMED <id> items with no DONE <id> (any host)\n  sweep-stubs           # remove header-only residue\n  \
refresh-header\n  show                  # merged live view\n  render [--json]       # merged live view, undecorated"
)]
struct Cli {
    #[arg(value_enum)]
    action: Action,
    content: Option<String>,
    rest: Vec<String>,
    /// Structured output for `render`
    #[arg(long)]
    json: bool,
    /// Board directory (default ~/Assistants/shared)
    #[arg(long, env = "MESSAGEBOARD_ROOT")]
    root: Option<PathBuf>,
}

// ── model ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    Legacy,
    Host(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Section {
    /// `### YYYY-MM-DD — device` line
    header: String,
    /// everything after the header, trimmed, without the trailing `---`
    body: String,
    source: Source,
}

impl Section {
    /// `CLAIMED <id> — …` / `DONE <id> — …` on the first body line: the
    /// work-claim protocol (kernel, 2026-09-23). `COMPLETE <id>` (the
    /// 2026-09-23 work-order precedent) counts as DONE. Returns (verb, id)
    /// with verb normalised to "CLAIMED" or "DONE".
    fn claim(&self) -> Option<(&'static str, String)> {
        let line = self.first_body_line();
        for (written, verb) in [("CLAIMED", "CLAIMED"), ("DONE", "DONE"), ("COMPLETE", "DONE")] {
            if let Some(rest) = line.strip_prefix(written).and_then(|r| r.strip_prefix(' ')) {
                let id: String = rest
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_end_matches(|c: char| !c.is_alphanumeric())
                    .to_string();
                if !id.is_empty() {
                    return Some((verb, id));
                }
            }
        }
        None
    }
    /// The section as written to disk: header, blank line, body.
    fn text(&self) -> String {
        if self.body.is_empty() {
            self.header.clone()
        } else {
            format!("{}\n\n{}", self.header, self.body)
        }
    }
    fn id(&self) -> String {
        section_id(&self.text())
    }
    fn is_stub(&self) -> bool {
        self.body.trim().is_empty()
    }
    fn date(&self) -> Option<NaiveDate> {
        let stamp: String = self.header.trim_start_matches("### ").chars().take(10).collect();
        NaiveDate::parse_from_str(&stamp, "%Y-%m-%d").ok()
    }
    fn contains(&self, needle: &str) -> bool {
        self.text().contains(needle)
    }
    /// Forum-owned items are retired by forum state, not by age.
    fn is_forum_owned(&self) -> bool {
        let t = self.text();
        t.contains("FORUM OPEN —") || t.contains("<!-- forum-work-order:")
    }
    fn first_body_line(&self) -> String {
        self.body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string()
    }
}

fn section_id(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// Sections of one board file, in file order; stubs (header-only residue)
/// are dropped at the parser so every rewrite sweeps them. Returns the
/// sections and the stub count.
fn parse_board(text: &str, source: &Source) -> Result<(Vec<Section>, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|l| *l == "## Messages")
        .with_context(|| "board has no ## Messages heading")?;
    let mut sections = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut started = false;
    let flush = |current: &Vec<&str>, sections: &mut Vec<Section>| {
        if current.is_empty() {
            return;
        }
        let header = current[0].to_string();
        let raw_body = current[1..].join("\n");
        let body = trim_body(&raw_body);
        sections.push(Section { header, body, source: source.clone() });
    };
    for line in &lines[start + 1..] {
        if line.starts_with("### ") {
            if started {
                flush(&current, &mut sections);
            }
            current = vec![line];
            started = true;
        } else if started {
            current.push(line);
        }
    }
    if started {
        flush(&current, &mut sections);
    }
    let total = sections.len();
    let real: Vec<Section> = sections.into_iter().filter(|s| !s.is_stub()).collect();
    let stubs = total - real.len();
    Ok((real, stubs))
}

/// The script's `trim-section`: trim, drop a trailing `---` rule, trim.
fn trim_body(raw: &str) -> String {
    let t = raw.trim();
    let mut lines: Vec<&str> = t.lines().collect();
    while let Some(last) = lines.last() {
        if last.trim().is_empty() || last.trim() == "---" {
            lines.pop();
        } else {
            break;
        }
    }
    lines.join("\n").trim().to_string()
}

fn render_board(header: &str, sections: &[Section]) -> String {
    if sections.is_empty() {
        return format!("{header}\n");
    }
    let body: Vec<String> = sections.iter().map(|s| format!("{}\n\n---", s.text())).collect();
    format!("{header}\n\n{}\n", body.join("\n\n"))
}

// ── files ────────────────────────────────────────────────────────────

struct Board {
    root: PathBuf,
    host: String,
    /// The device name written into new section headers (the script used
    /// `sys host | get hostname`, the full name).
    device: String,
}

impl Board {
    fn live_path(&self, source: &Source) -> PathBuf {
        match source {
            Source::Legacy => self.root.join(LEGACY_LIVE),
            Source::Host(h) => self.root.join(format!("MESSAGEBOARD.{h}.md")),
        }
    }
    fn own_live(&self) -> PathBuf {
        self.live_path(&Source::Host(self.host.clone()))
    }
    fn own_archive(&self) -> PathBuf {
        self.root.join(format!("MESSAGEBOARD-ARCHIVE.{}.md", self.host))
    }
    fn own_tombstones(&self) -> PathBuf {
        self.root.join(format!("MESSAGEBOARD-TOMBSTONES.{}.tsv", self.host))
    }

    /// Host names that have a live file, own host first, then sorted.
    fn hosts(&self) -> Result<Vec<String>> {
        let mut found = BTreeSet::new();
        if let Ok(rd) = fs::read_dir(&self.root) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if let Some(h) = host_of_live_file(&name) {
                    found.insert(h);
                }
            }
        }
        let mut hosts: Vec<String> = Vec::new();
        if found.remove(&self.host) {
            hosts.push(self.host.clone());
        }
        hosts.extend(found);
        Ok(hosts)
    }

    fn read_live(&self, source: &Source) -> Result<(Vec<Section>, usize)> {
        let path = self.live_path(source);
        if !path.is_file() {
            return Ok((Vec::new(), 0));
        }
        let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        parse_board(&text, source).with_context(|| path.display().to_string())
    }

    /// Every tombstoned id across every host.
    fn tombstoned(&self) -> Result<BTreeSet<String>> {
        let mut ids = BTreeSet::new();
        if let Ok(rd) = fs::read_dir(&self.root) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with("MESSAGEBOARD-TOMBSTONES.") && name.ends_with(".tsv") {
                    for line in fs::read_to_string(e.path()).unwrap_or_default().lines() {
                        if let Some(id) = line.split('\t').next() {
                            if !id.is_empty() {
                                ids.insert(id.to_string());
                            }
                        }
                    }
                }
            }
        }
        Ok(ids)
    }

    /// The merged live view: own host, other hosts, legacy; tombstoned
    /// sections hidden; newest first by header date (stable within a date).
    fn merged(&self) -> Result<Vec<Section>> {
        let dead = self.tombstoned()?;
        let mut all: Vec<Section> = Vec::new();
        for h in self.hosts()? {
            let (s, _) = self.read_live(&Source::Host(h))?;
            all.extend(s);
        }
        let (legacy, _) = self.read_live(&Source::Legacy)?;
        all.extend(legacy);
        let mut live: Vec<Section> = all.into_iter().filter(|s| !dead.contains(&s.id())).collect();
        live.sort_by(|a, b| b.date().cmp(&a.date()));
        Ok(live)
    }

    fn write_own_live(&self, sections: &[Section]) -> Result<String> {
        let text = render_board(LIVE_HEADER, sections);
        atomic_write(&self.own_live(), &text)?;
        Ok(text)
    }

    fn prepend_to_own_archive(&self, sections: &[Section]) -> Result<()> {
        if sections.is_empty() {
            return Ok(());
        }
        let path = self.own_archive();
        let existing = if path.is_file() {
            parse_board(&fs::read_to_string(&path)?, &Source::Host(self.host.clone()))?.0
        } else {
            Vec::new()
        };
        let mut combined: Vec<Section> = sections.to_vec();
        combined.extend(existing);
        atomic_write(&path, &render_board(ARCHIVE_HEADER, &combined))
    }

    fn tombstone(&self, section: &Section, reason: &str) -> Result<()> {
        let line = format!("{}\t{}\t{}\t{}\n", section.id(), Local::now().to_rfc3339(), self.host, reason);
        let mut existing = fs::read_to_string(self.own_tombstones()).unwrap_or_default();
        existing.push_str(&line);
        atomic_write(&self.own_tombstones(), &existing)
    }

    /// Retire `sections` from the live view: own ones by rewriting the own
    /// file, foreign ones by tombstone. `archive` also copies each into the
    /// own archive. Returns how many were retired.
    fn retire(&self, sections: &[Section], archive: bool, reason: &str) -> Result<usize> {
        let ids: BTreeSet<String> = sections.iter().map(Section::id).collect();
        let own_source = Source::Host(self.host.clone());
        let (own_all, _) = self.read_live(&own_source)?;
        let own_keep: Vec<Section> = own_all.iter().filter(|s| !ids.contains(&s.id())).cloned().collect();
        let own_changed = own_keep.len() != own_all.len();
        for s in sections {
            if s.source != own_source {
                self.tombstone(s, reason)?;
            }
        }
        if archive {
            self.prepend_to_own_archive(sections)?;
        }
        if own_changed || self.own_live().is_file() {
            let text = self.write_own_live(&own_keep)?;
            let _ = text;
        }
        Ok(sections.len())
    }

    fn after_write(&self) -> Result<()> {
        trigger_sync();
        let merged = self.merged()?;
        let text = render_board(LIVE_HEADER, &merged);
        if merged.len() > BLOAT_ITEMS || text.len() > BLOAT_BYTES {
            println!("⚠️ The Messageboard is now {} items / {} bytes. Live inbox, not a chronicle.", merged.len(), text.len());
            println!("   Find what is stranded:  messageboard-edit unclaimed 7");
            println!("   Then verify and clear:  messageboard-edit archive-containing \"unique text\"");
        }
        Ok(())
    }
}

/// `MESSAGEBOARD.<host>.md` → host; never the legacy file, an archive, or a
/// Syncthing conflict copy.
fn host_of_live_file(name: &str) -> Option<String> {
    let rest = name.strip_prefix("MESSAGEBOARD.")?.strip_suffix(".md")?;
    if rest.is_empty() || rest.contains('.') || rest.starts_with("sync-conflict") || rest.starts_with("ARCHIVE") {
        return None;
    }
    Some(rest.to_string())
}

fn atomic_write(path: &Path, text: &str) -> Result<()> {
    let dir = path.parent().with_context(|| format!("{} has no parent", path.display()))?;
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("board")));
    fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

/// Trigger an immediate Syncthing rescan so the entry reaches the other
/// machine without waiting for the periodic scan. Best effort; a sync failure
/// must never break the edit.
fn trigger_sync() {
    let key = Command::new("syncthing")
        .args(["cli", "config", "gui", "apikey", "get"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if key.is_empty() {
        return;
    }
    let _ = Command::new("curl")
        .args(["-s", "-X", "POST", "http://127.0.0.1:8384/rest/db/scan?folder=Assistants", "-H", &format!("X-API-Key: {key}")])
        .output();
}

/// Short lower-case hostname, first label: /etc/hostname, then `hostname`,
/// then $HOSTNAME — the same order system-health-check uses.
fn short_hostname() -> String {
    let candidates: [Option<String>; 3] = [
        fs::read_to_string("/etc/hostname").ok(),
        Command::new("hostname").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()),
        std::env::var("HOSTNAME").ok(),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(|h| h.trim().to_lowercase())
        .filter_map(|h| h.split('.').next().map(String::from))
        .find(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn device_name() -> String {
    Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(short_hostname)
}

// ── actions ──────────────────────────────────────────────────────────

fn run(cli: Cli) -> Result<()> {
    let root = match cli.root {
        Some(r) => r,
        None => std::env::var_os("HOME").map(PathBuf::from).context("HOME is not set")?.join("Assistants/shared"),
    };
    let board = Board { root, host: short_hostname(), device: device_name() };
    let content = cli.content.clone().unwrap_or_default();
    let today = Local::now().date_naive();

    match cli.action {
        Action::Insert => {
            if content.trim().is_empty() {
                println!("Error: message content required");
                return Ok(());
            }
            let own_source = Source::Host(board.host.clone());
            let (own, _) = board.read_live(&own_source)?;
            let header = format!("### {} — {}", today.format("%Y-%m-%d"), board.device);
            let new = Section { header, body: content.trim().to_string(), source: own_source };
            let mut sections = vec![new];
            sections.extend(own);
            board.write_own_live(&sections)?;
            board.after_write()?;
            println!("✅ Message inserted into MESSAGEBOARD.{}.md at {} — {}", board.host, today.format("%Y-%m-%d"), board.device);
        }
        Action::RemoveSection => {
            if content.is_empty() {
                println!("Error: section header required (e.g., '### 2026-01-11 — mac')");
                return Ok(());
            }
            let matching: Vec<Section> = board.merged()?.into_iter().filter(|s| s.text().starts_with(&content)).collect();
            if matching.is_empty() {
                println!("Section not found: {content}");
                return Ok(());
            }
            board.retire(&matching, false, "remove-section")?;
            board.after_write()?;
            println!("✅ Removed section: {content}");
        }
        Action::RemoveContaining => {
            if content.is_empty() {
                println!("Error: search string required");
                return Ok(());
            }
            // The script removed the FIRST matching section only.
            let first = board.merged()?.into_iter().find(|s| s.contains(&content));
            let Some(first) = first else {
                println!("No section containing: {content}");
                return Ok(());
            };
            board.retire(&[first], false, "remove-containing")?;
            board.after_write()?;
            println!("✅ Removed section containing: {content}");
        }
        Action::ArchiveContaining => {
            if content.is_empty() {
                println!("Error: search string required");
                return Ok(());
            }
            let matching: Vec<Section> = board.merged()?.into_iter().filter(|s| s.contains(&content)).collect();
            if matching.is_empty() {
                println!("No section containing: {content}");
                return Ok(());
            }
            let n = board.retire(&matching, true, "archive-containing")?;
            board.after_write()?;
            println!("✅ Archived {n} matching sections containing: {content}");
        }
        Action::ArchiveUnless => {
            let mut keeps: Vec<String> = Vec::new();
            if !content.is_empty() {
                keeps.push(content.clone());
            }
            keeps.extend(cli.rest.iter().filter(|k| !k.is_empty()).cloned());
            if keeps.is_empty() {
                println!("Error: at least one keep-needle is required");
                return Ok(());
            }
            let merged = board.merged()?;
            let (keep, archive): (Vec<Section>, Vec<Section>) =
                merged.into_iter().partition(|s| keeps.iter().any(|k| s.contains(k)));
            if archive.is_empty() {
                let own: Vec<Section> = keep.into_iter().filter(|s| s.source == Source::Host(board.host.clone())).collect();
                board.write_own_live(&own)?;
                println!("No sections to archive; live header refreshed");
                return Ok(());
            }
            let kept = keep.len();
            let n = board.retire(&archive, true, "archive-unless")?;
            board.after_write()?;
            println!("✅ Archived {n} sections; kept {kept}");
        }
        Action::ArchiveOlderThan => {
            let days = match content.parse::<i64>() {
                Ok(d) if d >= 1 => d,
                Ok(_) => {
                    println!("Error: day count must be >= 1");
                    return Ok(());
                }
                Err(_) => {
                    if content.is_empty() {
                        println!("Error: day count required (e.g. 30)");
                    } else {
                        println!("Error: day count must be an integer, got: {content}");
                    }
                    return Ok(());
                }
            };
            let cutoff = today - chrono::Duration::days(days);
            let merged = board.merged()?;
            let (archive, keep): (Vec<Section>, Vec<Section>) = merged.into_iter().partition(|s| {
                !s.is_forum_owned() && s.date().map(|d| d < cutoff).unwrap_or(true)
            });
            if archive.is_empty() {
                println!("No ordinary items older than {days} days");
                return Ok(());
            }
            let kept = keep.len();
            let n = board.retire(&archive, true, "archive-older-than")?;
            board.after_write()?;
            println!("✅ Archived {n} ordinary items older than {days} days; kept {kept}; FORUM OPEN pointers and unreceipted work orders retained");
        }
        Action::Unclaimed => {
            let days = if content.is_empty() { 7 } else { content.parse::<i64>().unwrap_or(-1) };
            if days < 1 {
                println!("Error: day count must be a positive integer, got: {content}");
                return Ok(());
            }
            let cutoff = today - chrono::Duration::days(days);
            let stale: Vec<Section> = board
                .merged()?
                .into_iter()
                .filter(|s| !s.is_forum_owned() && s.date().map(|d| d < cutoff).unwrap_or(true))
                .collect();
            if stale.is_empty() {
                println!("✅ Nothing unclaimed for more than {days} days");
                return Ok(());
            }
            println!("⚠️ {} items unclaimed for more than {days} days.", stale.len());
            println!("   These are NOT automatically stale — verify each, then archive-containing.");
            for s in &stale {
                let header = s.header.trim_start_matches("### ");
                let age = s.date().map(|d| format!("{}d", (today - d).num_days())).unwrap_or_else(|| "?".into());
                let body: String = s.first_body_line().chars().take(88).collect();
                println!("  [{age}] {header} — {body}");
            }
        }
        Action::Claims => {
            let merged = board.merged()?;
            let done: std::collections::HashSet<String> = merged
                .iter()
                .filter_map(|s| s.claim())
                .filter(|(verb, _)| *verb == "DONE")
                .map(|(_, id)| id)
                .collect();
            let open: Vec<&Section> = merged
                .iter()
                .filter(|s| matches!(s.claim(), Some(("CLAIMED", id)) if !done.contains(&id)))
                .collect();
            if open.is_empty() {
                println!("✅ No open claims — nothing on the board is CLAIMED without a matching DONE");
                return Ok(());
            }
            println!("⚠️ {} open claim(s). Do not start work that overlaps their paths, services or hosts; raise it instead.", open.len());
            for s in &open {
                let header = s.header.trim_start_matches("### ");
                let age = s.date().map(|d| (today - d).num_days()).unwrap_or(-1);
                let flag = if age > 1 { " STALE" } else { "" };
                let age_s = if age < 0 { "?".to_string() } else { format!("{age}d") };
                println!("  [{age_s}{flag}] {header} — {}", s.first_body_line());
            }
        }
        Action::SweepStubs => {
            let own_source = Source::Host(board.host.clone());
            let (own, own_stubs) = board.read_live(&own_source)?;
            let mut foreign_stubs = 0;
            for h in board.hosts()? {
                if h != board.host {
                    foreign_stubs += board.read_live(&Source::Host(h))?.1;
                }
            }
            foreign_stubs += board.read_live(&Source::Legacy)?.1;
            if own_stubs == 0 {
                println!("✅ No header-only stub sections");
            } else {
                board.write_own_live(&own)?;
                board.after_write()?;
                println!("✅ Swept {own_stubs} header-only stub sections");
            }
            if foreign_stubs > 0 {
                println!("   ({foreign_stubs} stub(s) in files this host does not own are hidden from the merged view but stay on disk)");
            }
        }
        Action::RefreshHeader => {
            let own_source = Source::Host(board.host.clone());
            let (own, _) = board.read_live(&own_source)?;
            board.write_own_live(&own)?;
            board.after_write()?;
            println!("✅ Refreshed MESSAGEBOARD.{}.md usage header", board.host);
        }
        Action::Show => {
            print!("{}", render_board(LIVE_HEADER, &board.merged()?));
        }
        Action::Render => {
            let merged = board.merged()?;
            if cli.json {
                let rows: Vec<serde_json::Value> = merged
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "id": s.id(),
                            "source": match &s.source { Source::Legacy => "legacy".to_string(), Source::Host(h) => h.clone() },
                            "date": s.date().map(|d| d.to_string()),
                            "header": s.header,
                            "body": s.body,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                print!("{}", render_board(LIVE_HEADER, &merged));
            }
        }
    }
    Ok(())
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("messageboard-edit: {e:#}");
        std::process::exit(1);
    }
}

// ── tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const NU_FORMAT: &str = "# Messageboard\n\n> usage header\n\n---\n\n## Messages\n\n### 2026-09-20 — nimbini\n\nOlder body line\n\n---\n\n### 2026-09-22 — Williams-MacBook-Air.local\n\nNewer body\nsecond line\n\n---\n\n### 2026-09-21 — nimbini\n\n---\n";

    fn board_in(dir: &Path, host: &str) -> Board {
        Board { root: dir.to_path_buf(), host: host.into(), device: format!("{host}.local") }
    }

    fn file_with(dir: &Path, name: &str, sections: &[(&str, &str)]) {
        let body: Vec<String> = sections.iter().map(|(h, b)| format!("### {h}\n\n{b}\n\n---")).collect();
        fs::write(dir.join(name), format!("# Messageboard\n\n## Messages\n\n{}\n", body.join("\n\n"))).unwrap();
    }

    #[test]
    fn parses_the_nu_format_drops_stubs_and_trims_rules() {
        let (s, stubs) = parse_board(NU_FORMAT, &Source::Legacy).unwrap();
        assert_eq!(stubs, 1, "the header-only 2026-09-21 section is a stub");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].header, "### 2026-09-20 — nimbini");
        assert_eq!(s[0].body, "Older body line");
        assert_eq!(s[1].body, "Newer body\nsecond line");
        assert_eq!(s[1].date(), NaiveDate::from_ymd_opt(2026, 9, 22));
        // Round trip keeps the script's byte shape.
        let rendered = render_board(LIVE_HEADER, &s);
        let (again, _) = parse_board(&rendered, &Source::Legacy).unwrap();
        assert_eq!(again, s);
        assert!(parse_board("no messages heading", &Source::Legacy).is_err());
    }

    #[test]
    fn ids_are_stable_and_content_bound() {
        let a = Section { header: "### 2026-09-22 — x".into(), body: "hello".into(), source: Source::Legacy };
        let b = Section { header: "### 2026-09-22 — x".into(), body: "hello".into(), source: Source::Host("nimbini".into()) };
        let c = Section { header: "### 2026-09-22 — x".into(), body: "hello!".into(), source: Source::Legacy };
        assert_eq!(a.id(), b.id(), "source does not enter the id");
        assert_ne!(a.id(), c.id());
        assert_eq!(a.id().len(), 12);
        assert_eq!(section_id("### 2026-09-22 — x\n\nhello"), a.id());
    }

    #[test]
    fn claims_pairs_claimed_with_done_across_hosts() {
        let d = tempfile::tempdir().unwrap();
        file_with(
            d.path(),
            "MESSAGEBOARD.williams-macbook-air.md",
            &[
                ("2026-09-23 — mac", "CLAIMED BATCH-A — Fable on the Mac. Touching: x"),
                ("2026-09-23 — mac", "DONE BATCH-B — finished"),
                ("2026-09-20 — mac", "CLAIMED WO-OLD — never closed"),
                ("2026-09-23 — mac", "MORNING BRIEF — not a claim; CLAIMED appears later in the body"),
            ],
        );
        file_with(d.path(), "MESSAGEBOARD.nimbini.md", &[("2026-09-23 — nimbini", "CLAIMED BATCH-B — Astra. Touching: y")]);
        let b = board_in(d.path(), "williams-macbook-air");
        let merged = b.merged().unwrap();
        let claims: Vec<(&str, String)> = merged.iter().filter_map(|s| s.claim()).collect();
        assert_eq!(claims.len(), 4, "three CLAIMED + one DONE; the brief is not a claim");
        let done: std::collections::HashSet<String> =
            claims.iter().filter(|(v, _)| *v == "DONE").map(|(_, id)| id.clone()).collect();
        let open: Vec<String> = merged
            .iter()
            .filter_map(|s| s.claim())
            .filter(|(v, id)| *v == "CLAIMED" && !done.contains(id))
            .map(|(_, id)| id)
            .collect();
        // Green control: BATCH-B was claimed on nimbini and closed from the Mac — not open.
        // Red control: BATCH-A (today) and WO-OLD (three days) are open.
        assert_eq!(open, vec!["BATCH-A".to_string(), "WO-OLD".to_string()]);
        // A trailing punctuation mark after the id does not enter it.
        let s = Section { header: "### 2026-09-23 — x".into(), body: "DONE BATCH-C. all good".into(), source: Source::Legacy };
        assert_eq!(s.claim(), Some(("DONE", "BATCH-C".to_string())));
        let n = Section { header: "### 2026-09-23 — x".into(), body: "CLAIMEDX".into(), source: Source::Legacy };
        assert_eq!(n.claim(), None);
        let c = Section { header: "### 2026-09-23 — x".into(), body: "COMPLETE WO-1 — six ports".into(), source: Source::Legacy };
        assert_eq!(c.claim(), Some(("DONE", "WO-1".to_string())), "COMPLETE closes a claim");
    }

    #[test]
    fn merged_view_orders_newest_first_across_hosts_and_legacy() {
        let d = tempfile::tempdir().unwrap();
        file_with(d.path(), "MESSAGEBOARD.md", &[("2026-09-18 — old-mac", "legacy item")]);
        file_with(d.path(), "MESSAGEBOARD.nimbini.md", &[("2026-09-23 — nimbini", "newest"), ("2026-09-19 — nimbini", "mid")]);
        file_with(d.path(), "MESSAGEBOARD.williams-macbook-air.md", &[("2026-09-22 — mac", "mac item")]);
        fs::write(d.path().join("MESSAGEBOARD.sync-conflict-20260922-1930-ABC.md"), "# Messageboard\n\n## Messages\n\n### 2026-09-30 — ghost\n\nconflict copy must be ignored\n").unwrap();
        fs::write(d.path().join("MESSAGEBOARD-ARCHIVE.nimbini.md"), "# archive\n\n## Messages\n\n### 2026-09-29 — archived\n\nmust be ignored\n").unwrap();
        let b = board_in(d.path(), "williams-macbook-air");
        let hosts = b.hosts().unwrap();
        assert_eq!(hosts, vec!["williams-macbook-air".to_string(), "nimbini".to_string()], "own host first");
        let m = b.merged().unwrap();
        let bodies: Vec<&str> = m.iter().map(|s| s.body.as_str()).collect();
        assert_eq!(bodies, vec!["newest", "mac item", "mid", "legacy item"]);
        assert_eq!(m[3].source, Source::Legacy);
        assert_eq!(host_of_live_file("MESSAGEBOARD.sync-conflict-x.md"), None);
        assert_eq!(host_of_live_file("MESSAGEBOARD.md"), None);
        assert_eq!(host_of_live_file("MESSAGEBOARD.nimbini.md"), Some("nimbini".into()));
    }

    #[test]
    fn own_sections_are_edited_foreign_ones_are_tombstoned_and_hidden() {
        let d = tempfile::tempdir().unwrap();
        file_with(d.path(), "MESSAGEBOARD.md", &[("2026-09-18 — old-mac", "legacy item LEGACY-KEY")]);
        file_with(d.path(), "MESSAGEBOARD.nimbini.md", &[("2026-09-23 — nimbini", "nimbini item NIM-KEY")]);
        file_with(d.path(), "MESSAGEBOARD.williams-macbook-air.md", &[("2026-09-22 — mac", "mac item MAC-KEY")]);
        let nimbini_before = fs::read_to_string(d.path().join("MESSAGEBOARD.nimbini.md")).unwrap();
        let legacy_before = fs::read_to_string(d.path().join("MESSAGEBOARD.md")).unwrap();
        let mac = board_in(d.path(), "williams-macbook-air");

        // Known-red control: both hosts' sections render before anything is retired.
        assert_eq!(mac.merged().unwrap().len(), 3);

        // Own section: edited in place, no tombstone.
        let own: Vec<Section> = mac.merged().unwrap().into_iter().filter(|s| s.contains("MAC-KEY")).collect();
        mac.retire(&own, true, "archive-containing").unwrap();
        assert!(!fs::read_to_string(d.path().join("MESSAGEBOARD.williams-macbook-air.md")).unwrap().contains("MAC-KEY"));
        assert!(fs::read_to_string(mac.own_archive()).unwrap().contains("MAC-KEY"));
        assert!(!mac.own_tombstones().exists());

        // Foreign sections (another host's and legacy): their files are untouched; tombstones + own archive.
        let foreign: Vec<Section> = mac.merged().unwrap().into_iter().filter(|s| s.contains("NIM-KEY") || s.contains("LEGACY-KEY")).collect();
        assert_eq!(foreign.len(), 2);
        mac.retire(&foreign, true, "archive-containing").unwrap();
        assert_eq!(fs::read_to_string(d.path().join("MESSAGEBOARD.nimbini.md")).unwrap(), nimbini_before, "foreign file byte-identical");
        assert_eq!(fs::read_to_string(d.path().join("MESSAGEBOARD.md")).unwrap(), legacy_before, "legacy file byte-identical");
        let tomb = fs::read_to_string(mac.own_tombstones()).unwrap();
        assert_eq!(tomb.lines().count(), 2);
        assert!(tomb.lines().all(|l| l.split('\t').count() == 4 && l.contains("\twilliams-macbook-air\tarchive-containing")));
        let archive = fs::read_to_string(mac.own_archive()).unwrap();
        assert!(archive.contains("NIM-KEY") && archive.contains("LEGACY-KEY") && archive.contains("MAC-KEY"));

        // Hidden from the merged view on EITHER host — nimbini reads the Mac's tombstones too.
        assert!(mac.merged().unwrap().is_empty());
        let nimbini = board_in(d.path(), "nimbini");
        assert!(nimbini.merged().unwrap().is_empty());
        // And a remove (not archive) of an own section leaves no archive copy.
        file_with(d.path(), "MESSAGEBOARD.williams-macbook-air.md", &[("2026-09-24 — mac", "transient TR-KEY")]);
        let tr: Vec<Section> = mac.merged().unwrap().into_iter().filter(|s| s.contains("TR-KEY")).collect();
        mac.retire(&tr, false, "remove-containing").unwrap();
        assert!(!fs::read_to_string(mac.own_archive()).unwrap().contains("TR-KEY"));
        assert!(mac.merged().unwrap().is_empty());
    }

    #[test]
    fn archive_older_than_window_exempts_forum_owned_items() {
        let d = tempfile::tempdir().unwrap();
        file_with(
            d.path(),
            "MESSAGEBOARD.nimbini.md",
            &[
                ("2026-09-23 — nimbini", "fresh"),
                ("2026-08-01 — nimbini", "FORUM OPEN — `x` pointer"),
                ("2026-08-02 — nimbini", "old work order <!-- forum-work-order:y -->"),
                ("2026-08-03 — nimbini", "old ordinary"),
                ("undated — nimbini", "no date at all"),
            ],
        );
        let b = board_in(d.path(), "nimbini");
        let today = NaiveDate::from_ymd_opt(2026, 9, 23).unwrap();
        let cutoff = today - chrono::Duration::days(14);
        let merged = b.merged().unwrap();
        let (archive, keep): (Vec<Section>, Vec<Section>) =
            merged.into_iter().partition(|s| !s.is_forum_owned() && s.date().map(|d| d < cutoff).unwrap_or(true));
        let a: Vec<&str> = archive.iter().map(|s| s.body.as_str()).collect();
        assert_eq!(a, vec!["old ordinary", "no date at all"]);
        assert_eq!(keep.len(), 3);
        b.retire(&archive, true, "archive-older-than").unwrap();
        let live = fs::read_to_string(b.own_live()).unwrap();
        assert!(live.contains("fresh") && live.contains("FORUM OPEN") && !live.contains("old ordinary"));
    }

    #[test]
    fn insert_writes_only_the_own_file_and_render_merges_it() {
        let d = tempfile::tempdir().unwrap();
        file_with(d.path(), "MESSAGEBOARD.md", &[("2026-09-18 — old-mac", "legacy item")]);
        let b = board_in(d.path(), "nimbini");
        let own_source = Source::Host("nimbini".into());
        let new = Section { header: "### 2026-09-23 — nimbini.local".into(), body: "posted".into(), source: own_source };
        b.write_own_live(&[new]).unwrap();
        assert!(fs::read_to_string(b.own_live()).unwrap().starts_with("# Messageboard"));
        assert!(!fs::read_to_string(d.path().join("MESSAGEBOARD.md")).unwrap().contains("posted"), "legacy never written");
        let text = render_board(LIVE_HEADER, &b.merged().unwrap());
        let posted = text.find("posted").unwrap();
        let legacy = text.find("legacy item").unwrap();
        assert!(posted < legacy, "newest first");
        assert!(text.contains("### 2026-09-23 — nimbini.local\n\nposted\n\n---"));
    }
}
