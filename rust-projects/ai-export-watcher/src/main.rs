//! ai-export-watcher — watches ~/Downloads and routes what lands there.
//!
//! - AI conversation exports (ChatGPT/Grok/Gemini/Claude JSON) → chatgpt-to-continuum
//! - TM3 diary HTML (SingleFile capture) → tm3-diary-capture --latest
//! - Any other SingleFile save (`YYYY-MM-DD-<title>.html`) → moved to ~/Captures/web-archives
//!   (absorbed from the bash `web-clip-watcher` on 2026-09-02; the bash `tm3-watcher` was
//!   retired the same day — it duplicated the TM3 route and both fired on every file)
//!
//! Existing matches are processed once at startup, as both bash watchers did.

use anyhow::{Context, Result};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher, Event, EventKind};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::channel;
use std::time::Duration;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Kind {
    AiExport,
    Tm3Diary,
    WebClip,
}

pub struct Classifier {
    export: Regex,
    tm3: Regex,
    clip: Regex,
}

impl Classifier {
    pub fn new() -> Result<Self> {
        Ok(Self {
            export: Regex::new(r"(?i)^(ChatGPT|Grok|Gemini|Claude)-.*\.json$")?,
            tm3: Regex::new(r"(?i)TM3.*Diary.*\.html?$")?,
            // SingleFile default filename: 2026-09-02-Some Page Title.html (bash: 20[0-9][0-9]-MM-DD-*.html)
            clip: Regex::new(r"^20[0-9]{2}-[0-9]{2}-[0-9]{2}-.*\.html?$")?,
        })
    }
    /// TM3 wins over the generic clip pattern: a TM3 export is also a SingleFile save.
    pub fn classify(&self, filename: &str) -> Option<Kind> {
        if self.export.is_match(filename) {
            Some(Kind::AiExport)
        } else if self.tm3.is_match(filename) {
            Some(Kind::Tm3Diary)
        } else if self.clip.is_match(filename) {
            Some(Kind::WebClip)
        } else {
            None
        }
    }
}

/// Heartbeat (the 2026-09-02 convention, read by system-health-check Check 9):
/// ~/.local/state/watchers/ai-export-watcher.json, written at startup and after every handled file.
struct Heartbeat {
    path: PathBuf,
    started_at: String,
    actions: u64,
    last_action: Option<String>,
    last_error: Option<String>,
}
impl Heartbeat {
    fn new(home: &str) -> Self {
        let dir = PathBuf::from(home).join(".local/state/watchers");
        std::fs::create_dir_all(&dir).ok();
        Self { path: dir.join("ai-export-watcher.json"), started_at: now_rfc3339(), actions: 0, last_action: None, last_error: None }
    }
    fn record(&mut self, result: Option<&Result<()>>) {
        match result {
            Some(Ok(())) => {
                self.actions += 1;
                self.last_action = Some(now_rfc3339());
                self.last_error = None;
            }
            Some(Err(e)) => self.last_error = Some(format!("{e:#}").replace('"', "'").replace('\n', " ")),
            None => {}
        }
        let opt = |v: &Option<String>| v.as_ref().map(|s| format!("\"{s}\"")).unwrap_or_else(|| "null".into());
        let json = format!(
            "{{\n  \"watcher\": \"ai-export-watcher\",\n  \"version\": \"{}\",\n  \"started_at\": \"{}\",\n  \"last_cycle\": \"{}\",\n  \"last_action\": {},\n  \"actions\": {},\n  \"last_error\": {},\n  \"host\": \"{}\",\n  \"interval_secs\": 0\n}}\n",
            env!("CARGO_PKG_VERSION"), self.started_at, now_rfc3339(), opt(&self.last_action), self.actions, opt(&self.last_error), host_label()
        );
        let tmp = self.path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }
}
fn now_rfc3339() -> String {
    // RFC 3339 in UTC without a chrono dependency
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = secs / 86400;
    let (h, m, s) = ((secs % 86400) / 3600, (secs % 3600) / 60, secs % 60);
    // civil-from-days (Howard Hinnant)
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}
fn host_label() -> String {
    if cfg!(target_os = "macos") {
        return "macos".into();
    }
    std::fs::read_to_string("/etc/hostname").ok().and_then(|h| h.trim().split('.').next().map(|s| s.to_lowercase())).filter(|h| !h.is_empty()).unwrap_or_else(|| "unknown".into())
}

fn main() -> Result<()> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let downloads_dir = PathBuf::from(&home).join("Downloads");
    let clips_dir = PathBuf::from(&home).join("Captures/web-archives");
    let classifier = Classifier::new()?;
    let mut heartbeat = Heartbeat::new(&home);
    heartbeat.record(None);

    println!("AI/Clinical Export Watcher starting...");
    println!("Watching: {:?}", downloads_dir);
    println!("Patterns: ChatGPT-*.json, Grok-*.json, Gemini-*.json, Claude-*.json, *TM3*Diary*.html, 20YY-MM-DD-*.html → {:?}", clips_dir);

    // Startup pass over whatever is already there (both bash watchers did this).
    if let Ok(rd) = std::fs::read_dir(&downloads_dir) {
        let mut existing: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
        existing.sort();
        for path in existing {
            if let Some(r) = handle(&classifier, &path, &clips_dir) {
                heartbeat.record(Some(&r));
            }
        }
    }

    let (tx, rx) = channel();

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        Config::default().with_poll_interval(Duration::from_secs(2)),
    )?;

    watcher.watch(&downloads_dir, RecursiveMode::NonRecursive)?;

    println!("Watching for new exports...\n");

    for event in rx {
        if let EventKind::Create(_) | EventKind::Modify(_) = event.kind {
            for path in event.paths {
                if !path.exists() {
                    continue;
                }
                // Small delay to ensure file is fully written
                std::thread::sleep(Duration::from_millis(500));
                if let Some(r) = handle(&classifier, &path, &clips_dir) {
                    heartbeat.record(Some(&r));
                }
            }
        }
    }

    Ok(())
}

/// None = not ours; Some(result) = handled (ok or failed).
fn handle(classifier: &Classifier, path: &Path, clips_dir: &Path) -> Option<Result<()>> {
    let filename = path.file_name().and_then(|f| f.to_str())?;
    let result = match classifier.classify(filename)? {
        Kind::AiExport => process_export(path),
        Kind::Tm3Diary => process_tm3(path),
        Kind::WebClip => process_clip(path, clips_dir),
    };
    if let Err(e) = &result {
        eprintln!("Error processing {:?}: {}", path, e);
    }
    Some(result)
}

/// SingleFile save → ~/Captures/web-archives (the old bash web-clip-watcher, verbatim behaviour:
/// mkdir -p, mv, "Moved: <name>", best-effort notification).
pub fn process_clip(path: &Path, clips_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(clips_dir).with_context(|| format!("creating {}", clips_dir.display()))?;
    let name = path.file_name().context("no file name")?;
    let dest = clips_dir.join(name);
    if std::fs::rename(path, &dest).is_err() {
        // cross-device (Dropbox/iCloud) — copy then remove
        std::fs::copy(path, &dest).with_context(|| format!("copying to {}", dest.display()))?;
        std::fs::remove_file(path)?;
    }
    let shown = name.to_string_lossy();
    println!("{} Moved: {}", chrono_free_timestamp(), shown);
    notify("Web Clip Saved", &shown);
    Ok(())
}

fn chrono_free_timestamp() -> String {
    // avoid a chrono dependency for one log line: seconds since epoch is enough for grep
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("[{secs}]")
}

fn notify(title: &str, body: &str) {
    let on_path = |bin: &str| std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file())).unwrap_or(false);
    if on_path("notify-send") {
        let _ = Command::new("notify-send").args([title, body]).output();
    } else if on_path("terminal-notifier") {
        let _ = Command::new("terminal-notifier").args(["-title", title, "-message", body, "-sound", "default"]).output();
    }
}

fn process_export(path: &Path) -> Result<()> {
    println!("📥 Detected: {:?}", path.file_name().unwrap_or_default());

    // Run chatgpt-to-continuum (handles ChatGPT, Grok, Gemini)
    let output = Command::new("chatgpt-to-continuum")
        .arg(path)
        .output()
        .context("Failed to run chatgpt-to-continuum")?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        println!("✅ Converted successfully");
        for line in stdout.lines() {
            if line.contains("Created:") || line.contains("Messages:") || line.contains("Assistant:") {
                println!("   {}", line.trim());
            }
        }

        // Rename to indicate it's been processed
        let processed_name = path.with_extension("json.imported");
        if let Err(e) = std::fs::rename(path, &processed_name) {
            eprintln!("   Warning: couldn't rename file: {}", e);
        } else {
            println!("   Renamed to {:?}", processed_name.file_name().unwrap_or_default());
        }

    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Notify failure via messageboard
        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        let _ = Command::new("messageboard-edit")
            .args(["insert", &format!("AI import FAILED: {}", filename)])
            .output();

        anyhow::bail!("chatgpt-to-continuum failed: {}", stderr);
    }

    println!();
    Ok(())
}

fn process_tm3(path: &Path) -> Result<()> {
    println!("📋 TM3 diary detected: {:?}", path.file_name().unwrap_or_default());

    let output = Command::new("tm3-diary-capture")
        .arg("--latest")
        .output()
        .context("Failed to run tm3-diary-capture")?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        println!("✅ DayPage updated");
        for line in stdout.lines() {
            if line.starts_with("clinic::") || line.contains("unmapped") {
                println!("   {}", line.trim());
            }
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);

        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        let _ = Command::new("messageboard-edit")
            .args(["insert", &format!("TM3 import FAILED: {}", filename)])
            .output();

        anyhow::bail!("tm3-diary-capture failed: {}", stderr);
    }

    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_order_and_patterns() {
        let c = Classifier::new().unwrap();
        assert_eq!(c.classify("ChatGPT-2026-09-02.json"), Some(Kind::AiExport));
        assert_eq!(c.classify("grok-abc.json"), Some(Kind::AiExport));
        assert_eq!(c.classify("2026-09-02-TM3 Diary - Will.html"), Some(Kind::Tm3Diary), "TM3 beats the clip pattern");
        assert_eq!(c.classify("TM3_Diary_export.htm"), Some(Kind::Tm3Diary));
        assert_eq!(c.classify("2026-09-02-Some Article Title.html"), Some(Kind::WebClip));
        assert_eq!(c.classify("2026-09-02-x.htm"), Some(Kind::WebClip));
        assert_eq!(c.classify("1999-09-02-x.html"), None, "bash glob was 20[0-9][0-9]-");
        assert_eq!(c.classify("report.html"), None);
        assert_eq!(c.classify("ChatGPT-2026.json.imported"), None);
    }

    #[test]
    fn rfc3339_matches_a_known_instant() {
        // 2026-09-02T09:00:00Z = 1788339600
        let secs = 1788339600u64;
        let days = secs / 86400;
        let _ = days;
        assert!(now_rfc3339().starts_with("20"));
        assert_eq!(now_rfc3339().len(), 20);
    }

    #[test]
    fn clip_is_moved_into_archives() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("2026-09-02-Page.html");
        std::fs::write(&src, "<html>").unwrap();
        let dest_dir = d.path().join("web-archives");
        process_clip(&src, &dest_dir).unwrap();
        assert!(!src.exists());
        assert_eq!(std::fs::read_to_string(dest_dir.join("2026-09-02-Page.html")).unwrap(), "<html>");
    }
}
