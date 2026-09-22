//! hx-sync-check — Rust port (2026-09-22) of the bash script the `hx` wrapper
//! runs before opening a file: ask the local Syncthing REST API whether another
//! device modified the file within the last `HX_SYNC_CHECK_THRESHOLD` minutes
//! (default 30) and say so, naming the device.
//!
//! Audit finding D2-22: the script's header promised `2 = error`, but it
//! exited 0 — "safe to edit" — on almost every failure (no config, daemon
//! down, JSON it could not read), and its GNU-only `date -d` meant the
//! timestamp step always failed on macOS, so the guard was permanently open
//! there. Here the exit codes mean what they say: 0 safe, 1 modified remotely
//! within the threshold, 2 the check could not run (one line says why).
//! Timestamps are parsed with chrono. Folders come from config.xml instead of
//! a hardcoded list of four, so every shared folder is covered. Warning text,
//! env var and default are unchanged.

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "hx-sync-check", version, about = "Warn if a file was recently modified on another Syncthing device")]
struct Cli {
    /// File about to be opened
    file: PathBuf,
    /// Warn if modified remotely within this many minutes
    #[arg(long, env = "HX_SYNC_CHECK_THRESHOLD", default_value_t = 30)]
    threshold: u64,
    /// Syncthing REST base (default from config.xml's <gui><address>, else localhost:8384)
    #[arg(long)]
    api: Option<String>,
}

// ── config.xml ──────────────────────────────────────────────────────

/// Standard locations, checked in order (Syncthing v2 XDG state dir, Mac, legacy).
pub fn find_config(home: &Path) -> Option<PathBuf> {
    [
        ".local/state/syncthing/config.xml",
        "Library/Application Support/Syncthing/config.xml",
        ".config/syncthing/config.xml",
    ]
    .iter()
    .map(|p| home.join(p))
    .find(|p| p.exists())
}

fn element_text(block: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let s = block.find(&open)? + open.len();
    let e = block[s..].find(&close)? + s;
    Some(block[s..e].trim().to_string())
}

/// `<gui><apikey>` and `<gui><address>`; the file is machine-written and
/// these are simple text elements, so no XML crate.
pub fn parse_gui(xml: &str) -> Option<(String, String)> {
    let start = xml.find("<gui")?;
    let end = xml[start..].find("</gui>").map(|e| start + e).unwrap_or(xml.len());
    let block = &xml[start..end];
    let apikey = element_text(block, "apikey")?;
    Some((apikey, element_text(block, "address").unwrap_or_default()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub id: String,
    pub path: PathBuf,
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let key = format!(" {name}=\"");
    let s = tag.find(&key)? + key.len();
    let e = tag[s..].find('"')? + s;
    Some(tag[s..e].to_string())
}

/// Every `<folder id="…" path="…">` in config.xml. A leading `~` in the path is
/// expanded against `home`.
pub fn parse_folders(xml: &str, home: &Path) -> Vec<Folder> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<folder ") {
        let tag_end = rest[i..].find('>').map(|e| i + e).unwrap_or(rest.len());
        let tag = &rest[i..tag_end];
        if let (Some(id), Some(path)) = (attr(tag, "id"), attr(tag, "path")) {
            let path = if let Some(stripped) = path.strip_prefix("~/") { home.join(stripped) } else { PathBuf::from(path) };
            out.push(Folder { id, path });
        }
        rest = &rest[tag_end..];
    }
    out
}

/// Which folder holds `file`, and the path Syncthing knows it by (relative,
/// forward slashes). The longest matching folder path wins.
pub fn resolve(file: &Path, folders: &[Folder]) -> Option<(String, String)> {
    let mut best: Option<(&Folder, PathBuf)> = None;
    for f in folders {
        let root = f.path.to_string_lossy().trim_end_matches('/').to_string();
        if let Ok(rel) = file.strip_prefix(&root) {
            let longer = best.as_ref().map(|(b, _)| root.len() > b.path.to_string_lossy().trim_end_matches('/').len()).unwrap_or(true);
            if longer {
                best = Some((f, rel.to_path_buf()));
            }
        }
    }
    best.map(|(f, rel)| (f.id.clone(), rel.to_string_lossy().replace('\\', "/")))
}

pub fn api_base(address: &str) -> String {
    const DEFAULT: &str = "http://localhost:8384";
    let addr = address.trim();
    if addr.is_empty() || addr.starts_with('/') || addr.starts_with("unix://") {
        return DEFAULT.into();
    }
    let Some((host, port)) = addr.rsplit_once(':') else { return DEFAULT.into() };
    if port.parse::<u16>().is_err() {
        return DEFAULT.into();
    }
    let host = match host {
        "" | "0.0.0.0" | "::" | "[::]" => "127.0.0.1",
        h => h,
    };
    format!("http://{host}:{port}")
}

// ── REST, behind a trait so the verdict path is testable ────────────

pub trait Api {
    /// GET `<base><path>`: `Ok(Some(json))`, `Ok(None)` when Syncthing answers
    /// 404 (it does not know the file), `Err` for anything else.
    fn get_json(&self, path: &str) -> Result<Option<Value>>;
}

struct Rest {
    base: String,
    key: String,
}

impl Api for Rest {
    fn get_json(&self, path: &str) -> Result<Option<Value>> {
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(5)).build();
        match agent.get(&format!("{}{}", self.base, path)).set("X-API-Key", &self.key).call() {
            Ok(resp) => {
                let body = resp.into_string().with_context(|| format!("reading {path}"))?;
                Ok(Some(serde_json::from_str(&body).with_context(|| format!("parsing {path}"))?))
            }
            Err(ureq::Error::Status(404, _)) => Ok(None),
            Err(ureq::Error::Status(code, _)) => Err(anyhow!("GET {path}: HTTP {code}")),
            Err(e) => Err(anyhow!("GET {path}: {e}")),
        }
    }
}

// ── verdict ─────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing to warn about; the reason is for diagnostics only.
    Safe(&'static str),
    /// Modified on another device within the threshold.
    Remote { device: String, age_minutes: i64 },
    /// The check could not run; open the file anyway but say so.
    CannotCheck(String),
}

impl Verdict {
    pub fn exit_code(&self) -> u8 {
        match self {
            Verdict::Safe(_) => 0,
            Verdict::Remote { .. } => 1,
            Verdict::CannotCheck(_) => 2,
        }
    }
}

/// Pure decision over the three API answers. `my_id` is the full local device
/// ID; Syncthing reports `global.modifiedBy` as the 7-character short form.
pub fn verdict(file_info: Option<&Value>, my_id: &str, devices: Option<&Value>, now: DateTime<Utc>, threshold_minutes: u64) -> Verdict {
    let Some(info) = file_info else { return Verdict::Safe("not tracked by Syncthing") };
    let modified_by = info.pointer("/global/modifiedBy").and_then(|v| v.as_str()).unwrap_or("");
    let modified = info.pointer("/global/modified").and_then(|v| v.as_str()).unwrap_or("");
    if modified_by.is_empty() || modified.is_empty() {
        return Verdict::CannotCheck("Syncthing returned no modifiedBy/modified for the file".into());
    }
    if my_id.is_empty() {
        return Verdict::CannotCheck("Syncthing returned no local device ID".into());
    }
    if my_id.starts_with(modified_by) {
        return Verdict::Safe("last modified locally");
    }
    let Ok(t) = DateTime::parse_from_rfc3339(modified) else {
        return Verdict::CannotCheck(format!("could not parse modification time {modified:?}"));
    };
    // Whole seconds, as the script's epoch arithmetic did: 23:25:00.32 → 23:30:00 is 5 minutes.
    let age_minutes = (now.timestamp() - t.timestamp()) / 60;
    if age_minutes >= threshold_minutes as i64 {
        return Verdict::Safe("remote modification older than the threshold");
    }
    let device = devices
        .and_then(|d| d.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|d| d.get("deviceID").and_then(|i| i.as_str()).map(|i| i.starts_with(modified_by)).unwrap_or(false))
                .and_then(|d| d.get("name").and_then(|n| n.as_str()))
                .map(String::from)
        })
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "another device".into());
    Verdict::Remote { device, age_minutes }
}

/// The whole check for one already-resolved (folder, relative) pair.
pub fn check(api: &dyn Api, folder: &str, relative: &str, now: DateTime<Utc>, threshold_minutes: u64) -> Verdict {
    let file_info = match api.get_json(&format!("/rest/db/file?folder={folder}&file={}", url_encode(relative))) {
        Ok(v) => v,
        Err(e) => return Verdict::CannotCheck(format!("{e:#}")),
    };
    if file_info.is_none() {
        return Verdict::Safe("not tracked by Syncthing");
    }
    let my_id = match api.get_json("/rest/system/status") {
        Ok(Some(v)) => v.get("myID").and_then(|i| i.as_str()).unwrap_or("").to_string(),
        Ok(None) => String::new(),
        Err(e) => return Verdict::CannotCheck(format!("{e:#}")),
    };
    // A missing device list only costs the friendly name.
    let devices = api.get_json("/rest/config/devices").ok().flatten();
    verdict(file_info.as_ref(), &my_id, devices.as_ref(), now, threshold_minutes)
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let home = match std::env::var_os("HOME").map(PathBuf::from) {
        Some(h) => h,
        None => {
            eprintln!("hx-sync-check: could not check: HOME is not set");
            return ExitCode::from(2);
        }
    };
    let file = cli.file.canonicalize().unwrap_or(cli.file.clone());

    let Some(config) = find_config(&home) else {
        eprintln!("hx-sync-check: could not check {}: no Syncthing config.xml found", file.display());
        return ExitCode::from(2);
    };
    let xml = match std::fs::read_to_string(&config) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("hx-sync-check: could not check {}: reading {}: {e}", file.display(), config.display());
            return ExitCode::from(2);
        }
    };
    let Some((apikey, address)) = parse_gui(&xml) else {
        eprintln!("hx-sync-check: could not check {}: no <gui><apikey> in {}", file.display(), config.display());
        return ExitCode::from(2);
    };
    let folders = parse_folders(&xml, &home);
    let Some((folder, relative)) = resolve(&file, &folders) else {
        return ExitCode::SUCCESS; // not in any shared folder: nothing another device could have changed
    };

    let api = Rest { base: cli.api.unwrap_or_else(|| api_base(&address)), key: apikey };
    match check(&api, &folder, &relative, Utc::now(), cli.threshold) {
        Verdict::Safe(_) => ExitCode::SUCCESS,
        Verdict::Remote { device, age_minutes } => {
            println!("⚠️  Warning: This file was modified on {device} {age_minutes} minutes ago");
            println!("   File: {relative}");
            println!("   Consider closing it there before editing here.");
            println!();
            ExitCode::from(1)
        }
        Verdict::CannotCheck(why) => {
            eprintln!("hx-sync-check: could not check {relative}: {why}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    const CONFIG: &str = r#"<configuration version="37">
    <folder id="Forge" label="Forge" path="/Users/w/Forge" type="sendreceive" rescanIntervalS="300">
        <device id="AAAAAAA-1"></device>
    </folder>
    <folder id="Notes" label="Notes" path="~/Notes" type="sendreceive">
    </folder>
    <folder id="Forge-sub" label="Sub" path="/Users/w/Forge/scrolls" type="sendreceive">
    </folder>
    <gui enabled="true" tls="false">
        <address>127.0.0.1:8384</address>
        <apikey>secretkey123</apikey>
    </gui>
</configuration>"#;

    #[test]
    fn folders_and_gui_come_from_config_xml() {
        let home = Path::new("/Users/w");
        let f = parse_folders(CONFIG, home);
        assert_eq!(f.len(), 3);
        assert_eq!(f[0], Folder { id: "Forge".into(), path: "/Users/w/Forge".into() });
        assert_eq!(f[1].path, PathBuf::from("/Users/w/Notes"), "~ expands");
        assert_eq!(parse_gui(CONFIG), Some(("secretkey123".into(), "127.0.0.1:8384".into())));
        assert_eq!(api_base("127.0.0.1:8384"), "http://127.0.0.1:8384");
        assert_eq!(api_base("0.0.0.0:8384"), "http://127.0.0.1:8384");
        assert_eq!(api_base(""), "http://localhost:8384");
    }

    #[test]
    fn resolve_picks_the_deepest_folder_and_ignores_outsiders() {
        let home = Path::new("/Users/w");
        let f = parse_folders(CONFIG, home);
        assert_eq!(resolve(Path::new("/Users/w/Forge/notes/a b.md"), &f), Some(("Forge".into(), "notes/a b.md".into())));
        assert_eq!(resolve(Path::new("/Users/w/Forge/scrolls/x.md"), &f), Some(("Forge-sub".into(), "x.md".into())));
        assert_eq!(resolve(Path::new("/Users/w/Notes/c.md"), &f), Some(("Notes".into(), "c.md".into())));
        assert_eq!(resolve(Path::new("/Users/w/Code/x.rs"), &f), None, "outside every folder → exit 0, not tracked");
        assert_eq!(url_encode("notes/a b.md"), "notes/a%20b.md");
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-22T23:30:00Z").unwrap().with_timezone(&Utc)
    }
    fn info(by: &str, modified: &str) -> Value {
        serde_json::json!({"global": {"modifiedBy": by, "modified": modified, "name": "x.md"}})
    }
    const MY_ID: &str = "AAAAAAA-BBBBBBB-CCCCCCC-DDDDDDD-EEEEEEE-FFFFFFF-GGGGGGG-HHHHHHH";
    fn devices() -> Value {
        serde_json::json!([{"deviceID": MY_ID, "name": "mac"}, {"deviceID": "ZZZZZZZ-1111111-2222222", "name": "nimbini"}])
    }

    #[test]
    fn verdict_known_red_known_green_and_cannot_check() {
        // Known-red: nimbini modified it 5 minutes ago.
        let v = verdict(Some(&info("ZZZZZZZ", "2026-09-22T23:25:00.322816454Z")), MY_ID, Some(&devices()), now(), 30);
        assert_eq!(v, Verdict::Remote { device: "nimbini".into(), age_minutes: 5 });
        assert_eq!(v.exit_code(), 1);
        // Known-green: modified locally, and a remote modification 45 minutes ago.
        assert_eq!(verdict(Some(&info("AAAAAAA", "2026-09-22T23:25:00Z")), MY_ID, None, now(), 30).exit_code(), 0);
        assert_eq!(verdict(Some(&info("ZZZZZZZ", "2026-09-22T22:45:00Z")), MY_ID, None, now(), 30).exit_code(), 0);
        // Untracked file: safe.
        assert_eq!(verdict(None, MY_ID, None, now(), 30).exit_code(), 0);
        // Could not check: no modification data, unparseable time, no local ID.
        assert_eq!(verdict(Some(&info("", "")), MY_ID, None, now(), 30).exit_code(), 2);
        assert_eq!(verdict(Some(&info("ZZZZZZZ", "yesterday")), MY_ID, None, now(), 30).exit_code(), 2);
        assert_eq!(verdict(Some(&info("ZZZZZZZ", "2026-09-22T23:25:00Z")), "", None, now(), 30).exit_code(), 2);
        // Unknown device short ID still warns, generically.
        let v = verdict(Some(&info("QQQQQQQ", "2026-09-22T23:20:00Z")), MY_ID, Some(&devices()), now(), 30);
        assert_eq!(v, Verdict::Remote { device: "another device".into(), age_minutes: 10 });
    }

    /// Canned API: paths → Ok(Some), Ok(None) for 404, Err for failure.
    struct Fake {
        responses: HashMap<String, Result<Option<Value>, String>>,
        calls: RefCell<Vec<String>>,
    }
    impl Api for Fake {
        fn get_json(&self, path: &str) -> Result<Option<Value>> {
            self.calls.borrow_mut().push(path.to_string());
            let key = path.split('?').next().unwrap_or(path);
            match self.responses.get(key) {
                Some(Ok(v)) => Ok(v.clone()),
                Some(Err(e)) => Err(anyhow!("{e}")),
                None => Err(anyhow!("unscripted {path}")),
            }
        }
    }
    fn fake(file: Result<Option<Value>, String>, status: Result<Option<Value>, String>) -> Fake {
        let mut responses = HashMap::new();
        responses.insert("/rest/db/file".to_string(), file);
        responses.insert("/rest/system/status".to_string(), status);
        responses.insert("/rest/config/devices".to_string(), Ok(Some(devices())));
        Fake { responses, calls: RefCell::new(vec![]) }
    }

    #[test]
    fn check_maps_every_failure_class_to_exit_2_and_404_to_safe() {
        let status = Ok(Some(serde_json::json!({"myID": MY_ID})));
        let red = fake(Ok(Some(info("ZZZZZZZ", "2026-09-22T23:25:00Z"))), status.clone());
        assert_eq!(check(&red, "Forge", "a b.md", now(), 30).exit_code(), 1);
        assert!(red.calls.borrow()[0].ends_with("file=a%20b.md"));
        // Syncthing does not know the file (404): safe.
        assert_eq!(check(&fake(Ok(None), status.clone()), "Forge", "new.md", now(), 30).exit_code(), 0);
        // Daemon down / broken JSON on the file query: could not check.
        let v = check(&fake(Err("GET /rest/db/file: connection refused".into()), status.clone()), "Forge", "x.md", now(), 30);
        assert!(matches!(v, Verdict::CannotCheck(ref m) if m.contains("connection refused")));
        assert_eq!(v.exit_code(), 2);
        // Status endpoint failing: could not check, not "safe".
        let v = check(&fake(Ok(Some(info("ZZZZZZZ", "2026-09-22T23:25:00Z"))), Err("parsing /rest/system/status".into())), "Forge", "x.md", now(), 30);
        assert_eq!(v.exit_code(), 2);
        // Device list failing only loses the friendly name.
        let mut f = fake(Ok(Some(info("ZZZZZZZ", "2026-09-22T23:25:00Z"))), status);
        f.responses.insert("/rest/config/devices".into(), Err("boom".into()));
        assert_eq!(check(&f, "Forge", "x.md", now(), 30), Verdict::Remote { device: "another device".into(), age_minutes: 5 });
    }
}
