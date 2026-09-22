//! sync-service-monitor — health of the dotfiles sync watchers on this host
//! and on its peer. Rust port (2026-09-22) of the Nushell script of the same
//! name, which only ever ran on nimbini and was written from the Mac's point
//! of view (local launchd, peer over ssh). This binary is host-aware: the
//! local side is launchd on macOS / systemd --user on Linux, the peer is the
//! shared ssh_config alias (`nimbini` from the Mac, `mac` from nimbini — the
//! same rule cross-machine-sync-check uses).
//!
//! What the port corrects (audit D2-20 and neighbours):
//! - "Recent sync activity" matched any `[HH:MM:SS]` in the last ten log
//!   lines, whatever the date. Entries now count only when their
//!   `[YYYY-MM-DD HH:MM:SS]` prefix falls inside `--window` minutes.
//! - Liveness comes from the watcher heartbeats in
//!   `~/.local/state/watchers/` (WATCHERS.md rules), not from lock-file age:
//!   the watchers write their lock once at startup and never again, so
//!   "lock older than 10 min" was every healthy watcher, and `clean`
//!   deleted the locks of running watchers. A lock is stale only when the
//!   PID it holds is not alive.
//! - `health` exits 1 on a failed check; the script always exited 0.
//! - The lock/log names are the ones the Rust watchers actually use.
//!
//! Actions: status (default) | restart | logs | health | clean.
//! `restart` and `clean` mutate and are never run by `status` or `health`.

mod exec;

use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use clap::{Parser, ValueEnum};
use exec::Exec;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "sync-service-monitor", about = "Health of the dotfiles sync watchers on this host and its peer")]
struct Cli {
    #[arg(value_enum, default_value_t = Action::Status)]
    action: Action,
    /// Minutes within which a timestamped log entry counts as recent activity
    #[arg(long, default_value_t = 30)]
    window: i64,
    /// Skip the peer (no ssh)
    #[arg(long)]
    local_only: bool,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
enum Action {
    /// Show status of the sync watchers here and on the peer
    Status,
    /// Restart the pull and push watchers here and on the peer (cleans dead-PID locks first)
    Restart,
    /// Show recent activity from the sync logs here and on the peer
    Logs,
    /// Seven-point health check; exit 1 on any FAIL
    Health,
    /// Remove lock files whose PID is no longer alive, here and on the peer
    Clean,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform {
    Mac,
    Linux,
}

impl Platform {
    fn local() -> Platform {
        if cfg!(target_os = "macos") { Platform::Mac } else { Platform::Linux }
    }
    fn name(self) -> &'static str {
        match self {
            Platform::Mac => "macOS",
            Platform::Linux => "Linux",
        }
    }
    fn dotter_package(self) -> &'static str {
        match self {
            Platform::Mac => "macos",
            Platform::Linux => "linux",
        }
    }
    /// ssh_config alias of the other machine and its platform.
    fn peer(self) -> (&'static str, Platform) {
        match self {
            Platform::Mac => ("nimbini", Platform::Linux),
            Platform::Linux => ("mac", Platform::Mac),
        }
    }
}

/// One sync watcher: how it is addressed on each platform.
pub struct Watcher {
    pub name: &'static str,
    pub label: &'static str,
    pub mac_agent: &'static str,
    pub linux_unit: &'static str,
    pub heartbeat: &'static str,
    pub lock: &'static str,
    pub mac_log: &'static str,
    pub linux_log: &'static str,
    /// Restarted by `restart`.
    pub restartable: bool,
}

pub const WATCHERS: [Watcher; 3] = [
    Watcher {
        name: "git-auto-pull-watcher",
        label: "📥 Auto-pull",
        mac_agent: "com.williamnapier.git-auto-pull-watcher",
        linux_unit: "git-auto-pull-watcher.service",
        heartbeat: "git-auto-pull-watcher.json",
        lock: "/tmp/git-auto-pull-watcher.lock",
        mac_log: ".local/share/git-auto-pull-watcher.log",
        linux_log: ".local/share/git-auto-pull-watcher.log",
        restartable: true,
    },
    Watcher {
        name: "git-auto-push-watcher",
        label: "📤 Auto-push",
        mac_agent: "com.williamnapier.git-auto-push-watcher",
        linux_unit: "git-auto-push-watcher.service",
        heartbeat: "git-auto-push-watcher-dotfiles.json",
        lock: "/tmp/git-auto-push-watcher-dotfiles.lock",
        mac_log: ".local/share/git-auto-push-watcher-macos.log",
        linux_log: ".local/share/git-auto-push-watcher.log",
        restartable: true,
    },
    Watcher {
        name: "dotter-realtime-watcher",
        label: "🔄 Dotter realtime",
        mac_agent: "com.user.dotter-realtime-watcher",
        linux_unit: "dotter-realtime-watcher.service",
        heartbeat: "dotter-realtime-watcher.json",
        lock: "/tmp/dotter-realtime-watcher.lock",
        mac_log: ".local/share/dotter-realtime-watcher.log",
        linux_log: ".local/share/dotter-realtime-watcher.log",
        restartable: false,
    },
];

impl Watcher {
    fn log(&self, platform: Platform) -> &'static str {
        match platform {
            Platform::Mac => self.mac_log,
            Platform::Linux => self.linux_log,
        }
    }
}

// ── unit state ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitState {
    /// Running now (PID on macOS; `active` on Linux).
    Active,
    /// Known to the service manager but not running (`inactive`, `failed`, or a launchd row with no PID).
    Loaded(String),
    NotLoaded,
    Unknown(String),
}

impl UnitState {
    fn ok(&self) -> bool {
        matches!(self, UnitState::Active)
    }
    fn show(&self) -> String {
        match self {
            UnitState::Active => "✅ running".into(),
            UnitState::Loaded(s) => format!("❌ not running ({s})"),
            UnitState::NotLoaded => "❌ not loaded".into(),
            UnitState::Unknown(s) => format!("❓ {s}"),
        }
    }
}

/// `launchctl list` rows: PID \t Status \t Label.
pub fn launchctl_state(list: &str, label: &str) -> UnitState {
    for line in list.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 && parts[2].trim() == label {
            return if parts[0].trim() == "-" { UnitState::Loaded(format!("last exit {}", parts[1].trim())) } else { UnitState::Active };
        }
    }
    UnitState::NotLoaded
}

pub fn systemd_state(is_active_out: &str) -> UnitState {
    match is_active_out.trim() {
        "active" | "activating" => UnitState::Active,
        "inactive" | "failed" | "deactivating" => UnitState::Loaded(is_active_out.trim().to_string()),
        "" => UnitState::Unknown("no answer from systemctl".into()),
        other => UnitState::Unknown(other.to_string()),
    }
}

fn local_unit_state(x: &dyn Exec, platform: Platform, w: &Watcher) -> UnitState {
    match platform {
        Platform::Mac => launchctl_state(&x.run("launchctl", &["list"]).stdout, w.mac_agent),
        Platform::Linux => systemd_state(&x.run("systemctl", &["--user", "is-active", w.linux_unit]).stdout),
    }
}

// ── heartbeats (WATCHERS.md rules, as system-health-check Check 9) ──

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Beat {
    Alive { age_min: i64, interval_secs: u64 },
    EventDriven { age_min: Option<i64> },
    Stale { age_min: i64, interval_secs: u64 },
    Error(String),
    Missing,
    Unreadable(&'static str),
}

impl Beat {
    fn ok(&self) -> bool {
        matches!(self, Beat::Alive { .. } | Beat::EventDriven { .. })
    }
    fn show(&self) -> String {
        match self {
            Beat::Alive { age_min, interval_secs } => format!("💓 heartbeat {age_min}m ago (every {interval_secs}s)"),
            Beat::EventDriven { age_min: Some(m) } => format!("💓 event-driven, last event {m}m ago"),
            Beat::EventDriven { age_min: None } => "💓 event-driven, no event yet".into(),
            Beat::Stale { age_min, interval_secs } => format!("💔 heartbeat STALE: {age_min}m ago (every {interval_secs}s) — dead or hung"),
            Beat::Error(e) => format!("💔 last error: {e}"),
            Beat::Missing => "💔 no heartbeat — never checked in here".into(),
            Beat::Unreadable(w) => format!("💔 heartbeat {w}"),
        }
    }
}

pub fn classify_heartbeat(now: DateTime<Local>, json: Option<&str>) -> Beat {
    let Some(json) = json else { return Beat::Missing };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return Beat::Unreadable("is not valid JSON") };
    if let Some(e) = v.get("last_error").and_then(|e| e.as_str()) {
        return Beat::Error(e.lines().next().unwrap_or(e).to_string());
    }
    let interval = v.get("interval_secs").and_then(|i| i.as_u64()).unwrap_or(0);
    let age = v
        .get("last_cycle")
        .and_then(|c| c.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| now.signed_duration_since(t.with_timezone(&Local)));
    if interval == 0 {
        return Beat::EventDriven { age_min: age.map(|a| a.num_minutes()) };
    }
    let Some(age) = age else { return Beat::Unreadable("has no parseable last_cycle") };
    let allowed = std::cmp::max(3 * interval as i64, 900);
    if age.num_seconds() > allowed {
        Beat::Stale { age_min: age.num_minutes(), interval_secs: interval }
    } else {
        Beat::Alive { age_min: age.num_minutes(), interval_secs: interval }
    }
}

fn local_heartbeat(home: &Path, w: &Watcher, now: DateTime<Local>) -> Beat {
    let path = home.join(".local/state/watchers").join(w.heartbeat);
    classify_heartbeat(now, std::fs::read_to_string(path).ok().as_deref())
}

// ── locks: stale means the PID is dead, not that the file is old ─────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockState {
    Absent,
    Held { pid: String, alive: bool },
    Unparseable,
}

impl LockState {
    fn stale(&self) -> bool {
        matches!(self, LockState::Held { alive: false, .. } | LockState::Unparseable)
    }
    fn show(&self) -> String {
        match self {
            LockState::Absent => "🆓 no lock".into(),
            LockState::Held { pid, alive: true } => format!("🔒 lock held by pid {pid} (alive)"),
            LockState::Held { pid, alive: false } => format!("🔴 lock held by pid {pid} which is DEAD — stale"),
            LockState::Unparseable => "🔴 lock file unreadable — stale".into(),
        }
    }
}

fn local_lock_state(x: &dyn Exec, path: &str) -> LockState {
    let Ok(text) = std::fs::read_to_string(path) else { return LockState::Absent };
    let pid = text.trim().to_string();
    if pid.is_empty() || !pid.chars().all(|c| c.is_ascii_digit()) {
        return LockState::Unparseable;
    }
    let alive = x.run("kill", &["-0", &pid]).ok();
    LockState::Held { pid, alive }
}

// ── log activity (D2-20): only timestamped entries inside the window ──

/// The `[YYYY-MM-DD HH:MM:SS]` prefix the sync watchers write, as local time.
pub fn entry_time(line: &str) -> Option<DateTime<Local>> {
    let rest = line.strip_prefix('[')?;
    let (stamp, _) = rest.split_once(']')?;
    let naive = NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M:%S").ok()?;
    Local.from_local_datetime(&naive).single()
}

#[derive(Debug, PartialEq, Eq)]
pub struct Activity {
    pub recent: usize,
    pub newest: Option<DateTime<Local>>,
}

pub fn activity(text: &str, now: DateTime<Local>, window_min: i64) -> Activity {
    let mut recent = 0;
    let mut newest: Option<DateTime<Local>> = None;
    for line in text.lines() {
        let Some(t) = entry_time(line) else { continue };
        if newest.map_or(true, |n| t > n) {
            newest = Some(t);
        }
        if now.signed_duration_since(t).num_minutes() <= window_min && t <= now {
            recent += 1;
        }
    }
    Activity { recent, newest }
}

fn tail(text: &str, n: usize) -> Vec<&str> {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].to_vec()
}

// ── the peer, in one ssh round trip ──────────────────────────────────

/// Shell for the peer that prints one tagged line per fact. Both shells are
/// POSIX sh; the peer's platform decides how units are asked.
fn peer_script(platform: Platform) -> String {
    let mut s = String::new();
    for w in &WATCHERS {
        match platform {
            Platform::Mac => s.push_str(&format!(
                "if r=$(launchctl list | grep -F '\t{a}'); then p=$(printf '%s' \"$r\" | cut -f1); if [ \"$p\" = \"-\" ]; then echo 'UNIT {n} loaded'; else echo 'UNIT {n} active'; fi; else echo 'UNIT {n} notloaded'; fi; ",
                a = w.mac_agent,
                n = w.name
            )),
            Platform::Linux => s.push_str(&format!("echo \"UNIT {n} $(systemctl --user is-active {u} 2>/dev/null)\"; ", n = w.name, u = w.linux_unit)),
        }
        s.push_str(&format!(
            "if [ -f ~/.local/state/watchers/{h} ]; then echo \"HB {n} $(tr -d '\\n' < ~/.local/state/watchers/{h})\"; else echo 'HB {n} MISSING'; fi; ",
            h = w.heartbeat,
            n = w.name
        ));
        s.push_str(&format!(
            "if [ -f {l} ]; then p=$(cat {l}); if kill -0 \"$p\" 2>/dev/null; then echo \"LOCK {n} $p alive\"; else echo \"LOCK {n} $p dead\"; fi; else echo 'LOCK {n} absent'; fi; ",
            l = w.lock,
            n = w.name
        ));
        s.push_str(&format!("echo 'LOG {n}'; tail -n 5 ~/{log} 2>/dev/null | sed 's/^/  /'; ", n = w.name, log = w.log(platform)));
    }
    s.push_str("echo \"PLATFORM $(cat ~/.dotter/local.toml 2>/dev/null | tr -d '\\n')\"; echo CONNECTED");
    s
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PeerSnapshot {
    pub connected: bool,
    pub units: Vec<(String, String)>,
    pub heartbeats: Vec<(String, Option<String>)>,
    pub locks: Vec<(String, LockState)>,
    pub logs: Vec<(String, Vec<String>)>,
    pub platform_line: String,
}

pub fn parse_peer_snapshot(text: &str) -> PeerSnapshot {
    let mut snap = PeerSnapshot::default();
    let mut current_log: Option<usize> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("  ") {
            if let Some(i) = current_log {
                snap.logs[i].1.push(rest.to_string());
            }
            continue;
        }
        current_log = None;
        if line == "CONNECTED" {
            snap.connected = true;
        } else if let Some(rest) = line.strip_prefix("UNIT ") {
            let (n, st) = rest.split_once(' ').unwrap_or((rest, ""));
            snap.units.push((n.to_string(), st.trim().to_string()));
        } else if let Some(rest) = line.strip_prefix("HB ") {
            let (n, j) = rest.split_once(' ').unwrap_or((rest, "MISSING"));
            snap.heartbeats.push((n.to_string(), if j == "MISSING" { None } else { Some(j.to_string()) }));
        } else if let Some(rest) = line.strip_prefix("LOCK ") {
            let parts: Vec<&str> = rest.split(' ').collect();
            let state = match parts.as_slice() {
                [_, "absent"] => LockState::Absent,
                [_, pid, "alive"] => LockState::Held { pid: pid.to_string(), alive: true },
                [_, pid, "dead"] => LockState::Held { pid: pid.to_string(), alive: false },
                _ => LockState::Unparseable,
            };
            snap.locks.push((parts[0].to_string(), state));
        } else if let Some(n) = line.strip_prefix("LOG ") {
            snap.logs.push((n.to_string(), vec![]));
            current_log = Some(snap.logs.len() - 1);
        } else if let Some(p) = line.strip_prefix("PLATFORM ") {
            snap.platform_line = p.to_string();
        }
    }
    snap
}

fn peer_unit_state(snap: &PeerSnapshot, peer: Platform, name: &str) -> UnitState {
    let st = snap.units.iter().find(|(n, _)| n == name).map(|(_, s)| s.as_str()).unwrap_or("");
    match peer {
        Platform::Mac => match st {
            "active" => UnitState::Active,
            "loaded" => UnitState::Loaded("not running".into()),
            "notloaded" => UnitState::NotLoaded,
            other => UnitState::Unknown(other.to_string()),
        },
        Platform::Linux => systemd_state(st),
    }
}

fn fetch_peer(x: &dyn Exec, alias: &str, platform: Platform) -> PeerSnapshot {
    let r = x.run("ssh", &["-o", "ConnectTimeout=8", "-o", "BatchMode=yes", alias, &peer_script(platform)]);
    parse_peer_snapshot(&r.stdout)
}

// ── actions ──────────────────────────────────────────────────────────

struct Ctx<'a> {
    x: &'a dyn Exec,
    home: PathBuf,
    platform: Platform,
    now: DateTime<Local>,
    window: i64,
    local_only: bool,
}

fn show_local(c: &Ctx) -> bool {
    println!("🔍 Sync watchers on this host ({})\n", c.platform.name());
    let mut all_ok = true;
    for w in &WATCHERS {
        let unit = local_unit_state(c.x, c.platform, w);
        let beat = local_heartbeat(&c.home, w, c.now);
        let lock = local_lock_state(c.x, w.lock);
        println!("{} {}", w.label, w.name);
        println!("  {}", unit.show());
        println!("  {}", beat.show());
        println!("  {}", lock.show());
        if let Ok(text) = std::fs::read_to_string(c.home.join(w.log(c.platform))) {
            let a = activity(&text, c.now, c.window);
            match a.newest {
                Some(t) => println!("  📝 {} entr{} in the last {}m; newest {}", a.recent, if a.recent == 1 { "y" } else { "ies" }, c.window, t.format("%Y-%m-%d %H:%M")),
                None => println!("  📝 no timestamped entries in the log"),
            }
        }
        all_ok &= unit.ok() && beat.ok() && !lock.stale();
        println!();
    }
    all_ok
}

fn show_peer(c: &Ctx) -> Option<PeerSnapshot> {
    if c.local_only {
        return None;
    }
    let (alias, peer) = c.platform.peer();
    println!("🌐 Peer `{alias}` ({})\n", peer.name());
    let snap = fetch_peer(c.x, alias, peer);
    if !snap.connected {
        println!("  ❌ cannot reach {alias} over ssh\n");
        return Some(snap);
    }
    for w in &WATCHERS {
        println!("{} {}", w.label, w.name);
        println!("  {}", peer_unit_state(&snap, peer, w.name).show());
        let hb = snap.heartbeats.iter().find(|(n, _)| n == w.name).and_then(|(_, j)| j.as_deref());
        println!("  {}", classify_heartbeat(c.now, hb).show());
        if let Some((_, l)) = snap.locks.iter().find(|(n, _)| n == w.name) {
            println!("  {}", l.show());
        }
        println!();
    }
    Some(snap)
}

fn status(c: &Ctx) -> bool {
    show_local(c);
    show_peer(c);
    true
}

fn logs(c: &Ctx) -> bool {
    println!("📝 Recent sync activity\n");
    for w in &WATCHERS[..2] {
        println!("{} {} (last 5 entries here):", w.label, w.name);
        match std::fs::read_to_string(c.home.join(w.log(c.platform))) {
            Ok(text) => {
                for l in tail(&text, 5) {
                    println!("  {l}");
                }
            }
            Err(_) => println!("  (no log)"),
        }
        println!();
    }
    if !c.local_only {
        let (alias, peer) = c.platform.peer();
        let snap = fetch_peer(c.x, alias, peer);
        if !snap.connected {
            println!("❌ cannot reach {alias} over ssh");
            return true;
        }
        for w in &WATCHERS[..2] {
            println!("{} {} on {alias} (last 5 entries):", w.label, w.name);
            match snap.logs.iter().find(|(n, _)| n == w.name) {
                Some((_, lines)) if !lines.is_empty() => lines.iter().for_each(|l| println!("  {l}")),
                _ => println!("  (no log)"),
            }
            println!();
        }
    }
    true
}

/// Dead-PID locks only. Returns the paths removed.
fn clean_local(c: &Ctx) -> Vec<String> {
    let mut removed = vec![];
    for w in &WATCHERS {
        if local_lock_state(c.x, w.lock).stale() {
            match std::fs::remove_file(w.lock) {
                Ok(()) => {
                    println!("  🧹 removed {} (holder not alive)", w.lock);
                    removed.push(w.lock.to_string());
                }
                Err(e) => println!("  ❌ could not remove {}: {e}", w.lock),
            }
        }
    }
    removed
}

fn clean(c: &Ctx) -> bool {
    println!("🧹 Lock files held by a dead process\n");
    let removed = clean_local(c);
    if removed.is_empty() {
        println!("  ✅ no stale locks here");
    }
    if !c.local_only {
        let (alias, _) = c.platform.peer();
        let script = WATCHERS
            .iter()
            .map(|w| format!("if [ -f {l} ] && ! kill -0 \"$(cat {l})\" 2>/dev/null; then rm -f {l} && echo \"removed {l}\"; fi; ", l = w.lock))
            .collect::<String>()
            + "echo done";
        let r = c.x.run("ssh", &["-o", "ConnectTimeout=8", "-o", "BatchMode=yes", alias, &script]);
        if r.ok() {
            let lines: Vec<&str> = r.stdout.lines().filter(|l| l.starts_with("removed")).collect();
            if lines.is_empty() {
                println!("  ✅ no stale locks on {alias}");
            } else {
                lines.iter().for_each(|l| println!("  🧹 {alias}: {l}"));
            }
        } else {
            println!("  ❌ cannot reach {alias} over ssh");
        }
    }
    true
}

fn restart(c: &Ctx) -> bool {
    println!("🔄 Restarting the pull and push watchers\n");
    clean_local(c);
    let mut ok = true;
    for w in WATCHERS.iter().filter(|w| w.restartable) {
        let r = match c.platform {
            Platform::Mac => {
                let uid = c.x.run("id", &["-u"]).out();
                let plist = c.home.join("Library/LaunchAgents").join(format!("{}.plist", w.mac_agent));
                let _ = c.x.run("launchctl", &["bootout", &format!("gui/{uid}/{}", w.mac_agent)]);
                c.x.run("launchctl", &["bootstrap", &format!("gui/{uid}"), &plist.to_string_lossy()])
            }
            Platform::Linux => c.x.run("systemctl", &["--user", "restart", w.linux_unit]),
        };
        if r.ok() {
            println!("  ✅ {} restarted here", w.name);
        } else {
            println!("  ❌ {} restart failed here: {}", w.name, r.stderr.trim());
            ok = false;
        }
    }
    if !c.local_only {
        let (alias, peer) = c.platform.peer();
        let script = match peer {
            Platform::Linux => "systemctl --user restart git-auto-pull-watcher.service git-auto-push-watcher.service && echo done".to_string(),
            Platform::Mac => "u=$(id -u); for a in com.williamnapier.git-auto-pull-watcher com.williamnapier.git-auto-push-watcher; do launchctl bootout gui/$u/$a 2>/dev/null; launchctl bootstrap gui/$u ~/Library/LaunchAgents/$a.plist || exit 1; done; echo done".to_string(),
        };
        let r = c.x.run("ssh", &["-o", "ConnectTimeout=8", "-o", "BatchMode=yes", alias, &script]);
        if r.ok() && r.stdout.contains("done") {
            println!("  ✅ pull and push watchers restarted on {alias}");
        } else {
            println!("  ❌ restart on {alias} failed: {}", r.stderr.trim());
            ok = false;
        }
    }
    ok
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct HealthScore {
    pub pass: usize,
    pub fail: usize,
    pub warn: usize,
}

fn verdict(score: &HealthScore, total: usize) -> &'static str {
    let pct = score.pass * 100 / total.max(1);
    if pct >= 90 {
        "🟢 EXCELLENT"
    } else if pct >= 70 {
        "🟡 GOOD"
    } else if pct >= 50 {
        "🟠 FAIR"
    } else {
        "🔴 POOR"
    }
}

fn health(c: &Ctx) -> bool {
    println!("🏥 Sync health check\n");
    let mut s = HealthScore::default();
    let mut check = |n: u8, title: &str, outcome: Result<String, String>| match outcome {
        Ok(m) => {
            println!("🔍 Check {n}: {title}\n  ✅ PASS: {m}\n");
            s.pass += 1;
        }
        Err(m) if m.starts_with("WARN") => {
            println!("🔍 Check {n}: {title}\n  ⚠️  {m}\n");
            s.warn += 1;
        }
        Err(m) => {
            println!("🔍 Check {n}: {title}\n  ❌ FAIL: {m}\n");
            s.fail += 1;
        }
    };

    // 1 + 2: the local pull and push watchers, by unit AND heartbeat.
    for (n, w) in WATCHERS.iter().filter(|w| w.restartable).enumerate() {
        let unit = local_unit_state(c.x, c.platform, w);
        let beat = local_heartbeat(&c.home, w, c.now);
        let outcome = if unit.ok() && beat.ok() {
            Ok(format!("{} — {}", unit.show().trim_start_matches("✅ "), beat.show().trim_start_matches("💓 ")))
        } else {
            Err(format!("{} — {}", unit.show(), beat.show()))
        };
        check(n as u8 + 1, &format!("{} here", w.name), outcome);
    }

    // 3: activity inside the window (D2-20). The sync logs are event-only —
    // a quiet repository writes nothing for hours — so a quiet window is a
    // WARN; liveness is checks 1, 2 and 6. A log that cannot be read is a FAIL.
    let mut recent = 0usize;
    let mut newest: Option<DateTime<Local>> = None;
    let mut unreadable = vec![];
    for w in &WATCHERS[..2] {
        match std::fs::read_to_string(c.home.join(w.log(c.platform))) {
            Ok(text) => {
                let a = activity(&text, c.now, c.window);
                recent += a.recent;
                if let Some(t) = a.newest {
                    if newest.map_or(true, |n| t > n) {
                        newest = Some(t);
                    }
                }
            }
            Err(_) => unreadable.push(w.name),
        }
    }
    let outcome = if !unreadable.is_empty() {
        Err(format!("no readable log for {}", unreadable.join(", ")))
    } else if recent > 0 {
        Ok(format!("{recent} sync log entries in the last {}m", c.window))
    } else {
        Err(format!("WARN: no sync log entry in the last {}m (newest {})", c.window, newest.map(|t| t.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_else(|| "none".into())))
    };
    check(3, "Recent sync activity", outcome);

    // 4: the repository answers.
    let dotfiles = c.home.join("dotfiles");
    let r = c.x.run("git", &["-C", &dotfiles.to_string_lossy(), "status", "--porcelain"]);
    check(4, "Git repository status", if r.ok() { Ok("~/dotfiles answers git status".into()) } else { Err(format!("git status failed: {}", r.stderr.trim())) });

    // 5: locks held by dead processes.
    let stale: Vec<&str> = WATCHERS.iter().filter(|w| local_lock_state(c.x, w.lock).stale()).map(|w| w.lock).collect();
    check(5, "Lock file health", if stale.is_empty() { Ok("no lock is held by a dead process".into()) } else { Err(format!("stale locks (holder dead): {}", stale.join(", "))) });

    // 6: the peer, and its pull/push watchers.
    let snap = if c.local_only { None } else { Some(fetch_peer(c.x, c.platform.peer().0, c.platform.peer().1)) };
    let (alias, peer) = c.platform.peer();
    let outcome = match &snap {
        None => Err("WARN: peer skipped (--local-only)".into()),
        Some(s) if !s.connected => Err(format!("cannot reach {alias} over ssh")),
        Some(s) => {
            let mut bad = vec![];
            for w in WATCHERS.iter().filter(|w| w.restartable) {
                let u = peer_unit_state(s, peer, w.name);
                let hb = s.heartbeats.iter().find(|(n, _)| n == w.name).and_then(|(_, j)| j.as_deref());
                let b = classify_heartbeat(c.now, hb);
                if !(u.ok() && b.ok()) {
                    bad.push(format!("{}: {} / {}", w.name, u.show(), b.show()));
                }
            }
            if bad.is_empty() { Ok(format!("{alias} reachable; pull and push watchers alive there")) } else { Err(format!("{alias}: {}", bad.join("; "))) }
        }
    };
    check(6, "Peer sync watchers", outcome);

    // 7: Dotter platform package here (and on the peer, as a bonus line).
    let local_toml = std::fs::read_to_string(c.home.join(".dotter/local.toml")).unwrap_or_default();
    let expected = c.platform.dotter_package();
    let outcome = if local_toml.is_empty() {
        Err("missing ~/.dotter/local.toml — platform-specific configs will not deploy".into())
    } else if local_toml.contains(expected) {
        Ok(format!("platform package '{expected}'"))
    } else {
        Err(format!("~/.dotter/local.toml does not select '{expected}'"))
    };
    check(7, "Dotter platform configuration", outcome);
    if let Some(s) = &snap {
        if s.connected {
            let want = peer.dotter_package();
            if s.platform_line.contains(want) {
                println!("  ✅ BONUS: {alias} selects '{want}'\n");
            } else {
                println!("  ⚠️  {alias}: ~/.dotter/local.toml does not select '{want}' ({})\n", if s.platform_line.is_empty() { "missing" } else { "wrong" });
            }
        }
    }

    println!("🏥 Overall: {}/7 passed, {} failed, {} warnings — {}", s.pass, s.fail, s.warn, verdict(&s, 7));
    s.fail == 0
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        eprintln!("HOME is not set");
        return ExitCode::from(2);
    };
    let real = exec::Real;
    let c = Ctx { x: &real, home, platform: Platform::local(), now: Local::now(), window: cli.window, local_only: cli.local_only };
    let ok = match cli.action {
        Action::Status => status(&c),
        Action::Restart => restart(&c),
        Action::Logs => logs(&c),
        Action::Health => health(&c),
        Action::Clean => clean(&c),
    };
    if ok { ExitCode::SUCCESS } else { ExitCode::from(1) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exec::{CmdResult, Fake};

    fn at(s: &str) -> DateTime<Local> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Local)
    }

    #[test]
    fn activity_counts_only_timestamped_entries_inside_the_window() {
        let now = at("2026-09-22T23:30:00+01:00");
        let log = "\
[2026-09-21 23:28:00] ✅ pulled yesterday at this time
[2026-09-22 08:04:50] ✅ this morning
Please make sure you have the correct access rights at 12:34:56 (no prefix, must not count)
[2026-09-22 23:27:00] 📥 three minutes ago
";
        let a = activity(log, now, 30);
        assert_eq!(a.recent, 1, "only the 23:27 entry is inside 30 minutes");
        assert_eq!(a.newest, Some(at("2026-09-22T23:27:00+01:00")));
        // Known-red: a log whose newest entry is yesterday is not recent, however many lines it has.
        let stale = "[2026-09-21 23:28:00] a\n[2026-09-21 23:29:00] b\n[2026-09-21 23:29:30] c\n";
        assert_eq!(activity(stale, now, 30).recent, 0);
        // The old check: any HH:MM:SS anywhere in the last lines.
        assert!(entry_time("some text 12:34:56 inside").is_none());
        assert!(entry_time("[2026-09-22 23:27:00] x").is_some());
        assert!(entry_time("[not a date] x").is_none());
    }

    #[test]
    fn heartbeat_rules_match_check_nine() {
        let now = at("2026-09-22T23:30:00+01:00");
        let fresh = r#"{"last_cycle":"2026-09-22T23:29:00+01:00","interval_secs":120}"#;
        assert!(matches!(classify_heartbeat(now, Some(fresh)), Beat::Alive { interval_secs: 120, .. }));
        let stale = r#"{"last_cycle":"2026-09-22T21:14:00+01:00","interval_secs":120}"#;
        assert!(matches!(classify_heartbeat(now, Some(stale)), Beat::Stale { .. }));
        let err = r#"{"last_cycle":"2026-09-22T23:29:00+01:00","interval_secs":120,"last_error":"ssh: connect to host github.com port 22\nfatal"}"#;
        assert_eq!(classify_heartbeat(now, Some(err)), Beat::Error("ssh: connect to host github.com port 22".into()));
        let event = r#"{"last_cycle":"2026-09-18T11:02:00+01:00","interval_secs":0}"#;
        assert!(matches!(classify_heartbeat(now, Some(event)), Beat::EventDriven { .. }));
        assert_eq!(classify_heartbeat(now, None), Beat::Missing);
        assert!(!classify_heartbeat(now, Some("nope")).ok());
    }

    #[test]
    fn unit_states_from_launchctl_and_systemctl() {
        let list = "PID\tStatus\tLabel\n123\t0\tcom.williamnapier.git-auto-pull-watcher\n-\t-15\tcom.williamnapier.git-auto-push-watcher\n";
        assert_eq!(launchctl_state(list, "com.williamnapier.git-auto-pull-watcher"), UnitState::Active);
        assert_eq!(launchctl_state(list, "com.williamnapier.git-auto-push-watcher"), UnitState::Loaded("last exit -15".into()));
        assert_eq!(launchctl_state(list, "com.user.dotter-realtime-watcher"), UnitState::NotLoaded);
        assert_eq!(systemd_state("active\n"), UnitState::Active);
        assert_eq!(systemd_state("failed\n"), UnitState::Loaded("failed".into()));
        assert!(matches!(systemd_state(""), UnitState::Unknown(_)));
    }

    #[test]
    fn lock_is_stale_only_when_its_pid_is_dead() {
        let dir = tempfile::tempdir().unwrap();
        let alive = dir.path().join("alive.lock");
        let dead = dir.path().join("dead.lock");
        let junk = dir.path().join("junk.lock");
        std::fs::write(&alive, "4242\n").unwrap();
        std::fs::write(&dead, "9999\n").unwrap();
        std::fs::write(&junk, "not a pid\n").unwrap();
        let mut fake = Fake::default();
        fake.respond("kill", &["-0", "4242"], CmdResult::success(""));
        fake.respond("kill", &["-0", "9999"], CmdResult::failure(1, "No such process"));
        assert_eq!(local_lock_state(&fake, alive.to_str().unwrap()), LockState::Held { pid: "4242".into(), alive: true });
        assert!(!local_lock_state(&fake, alive.to_str().unwrap()).stale(), "a four-day-old lock of a live watcher is not stale");
        assert!(local_lock_state(&fake, dead.to_str().unwrap()).stale());
        assert!(local_lock_state(&fake, junk.to_str().unwrap()).stale());
        assert_eq!(local_lock_state(&fake, dir.path().join("none.lock").to_str().unwrap()), LockState::Absent);
    }

    #[test]
    fn peer_snapshot_round_trip() {
        let text = "UNIT git-auto-pull-watcher active\nHB git-auto-pull-watcher {\"last_cycle\":\"2026-09-22T23:29:00+01:00\",\"interval_secs\":120}\nLOCK git-auto-pull-watcher 1597 dead\nLOG git-auto-pull-watcher\n  [2026-09-22 18:34:37] committed\n  [2026-09-22 18:34:39] ✅ pushed\nUNIT git-auto-push-watcher inactive\nHB git-auto-push-watcher MISSING\nLOCK git-auto-push-watcher absent\nLOG git-auto-push-watcher\nPLATFORM packages = [\"linux\"]\nCONNECTED\n";
        let s = parse_peer_snapshot(text);
        assert!(s.connected);
        assert_eq!(peer_unit_state(&s, Platform::Linux, "git-auto-pull-watcher"), UnitState::Active);
        assert_eq!(peer_unit_state(&s, Platform::Linux, "git-auto-push-watcher"), UnitState::Loaded("inactive".into()));
        assert_eq!(s.locks[0].1, LockState::Held { pid: "1597".into(), alive: false });
        assert_eq!(s.logs[0].1.len(), 2);
        assert!(s.logs[1].1.is_empty());
        assert_eq!(s.heartbeats[1].1, None);
        assert!(s.platform_line.contains("linux"));
        assert!(!parse_peer_snapshot("ssh: connect to host mac port 22: timed out\n").connected);
        // A Mac peer's unit lines.
        let mac = parse_peer_snapshot("UNIT git-auto-pull-watcher loaded\nCONNECTED\n");
        assert!(matches!(peer_unit_state(&mac, Platform::Mac, "git-auto-pull-watcher"), UnitState::Loaded(_)));
    }

    #[test]
    fn verdict_bands_and_peer_alias_rule() {
        assert_eq!(verdict(&HealthScore { pass: 7, fail: 0, warn: 0 }, 7), "🟢 EXCELLENT");
        assert_eq!(verdict(&HealthScore { pass: 5, fail: 2, warn: 0 }, 7), "🟡 GOOD");
        assert_eq!(verdict(&HealthScore { pass: 4, fail: 3, warn: 0 }, 7), "🟠 FAIR");
        assert_eq!(verdict(&HealthScore { pass: 2, fail: 5, warn: 0 }, 7), "🔴 POOR");
        assert_eq!(Platform::Mac.peer(), ("nimbini", Platform::Linux));
        assert_eq!(Platform::Linux.peer(), ("mac", Platform::Mac));
        assert!(peer_script(Platform::Linux).contains("systemctl --user is-active git-auto-pull-watcher.service"));
        assert!(peer_script(Platform::Mac).contains("launchctl list"));
    }
}
