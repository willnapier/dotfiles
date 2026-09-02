//! The checks. Each returns a list of problem strings; an empty list means
//! "checked and clean". "Could not check" is ALWAYS a problem string, never an
//! empty list (review D2-10) — a checker that cannot distinguish "fine" from
//! "did not look" is not a checker.
//!
//! Problem strings and verbose lines are kept byte-for-byte compatible with the
//! Nushell version this replaced (2026-09-01), so the log file, the desktop
//! notification and anything grepping the log are unaffected.

use crate::exec::Exec;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub struct Ctx<'a> {
    pub exec: &'a dyn Exec,
    pub verbose: bool,
    pub fix: bool,
    pub home: PathBuf,
    pub log: &'a dyn Fn(&str, &str),
}

impl Ctx<'_> {
    fn say(&self, line: &str) {
        if self.verbose {
            println!("{line}");
        }
    }
    fn section(&self, title: &str) {
        self.say(&format!("── {title} ──"));
    }
    fn end_section(&self) {
        self.say("");
    }
    fn log_fix(&self, msg: &str) {
        (self.log)("FIX", msg);
    }
}

const SELF_TIMER: &str = "system-health-check.timer";
const SELF_UNIT: &str = "system-health-check.service";
const SELF_AGENT: &str = "com.williamnapier.system-health-check";

/// Intentional-divergence register: agents whose plist is deployed on this
/// machine but which must NOT be loaded here. "Deployed but not enabled" is how
/// a single-writer or single-worker decision is expressed (see
/// DOTTER-CROSS-PLATFORM-MASTER-GUIDE.md), so their absence from launchctl is
/// the intended state, not a problem.
///
/// - forum-worker: nimbini is the elected forum worker; the Mac must not run a
///   competing one (machine layer `context/machines/macos.md`).
#[cfg(target_os = "macos")]
const INTENTIONALLY_UNLOADED_AGENTS: &[&str] = &["com.williamnapier.forum-worker"];
#[cfg(not(target_os = "macos"))]
const INTENTIONALLY_UNLOADED_AGENTS: &[&str] = &[];

pub fn intentionally_unloaded(label: &str) -> bool {
    INTENTIONALLY_UNLOADED_AGENTS.contains(&label)
}

// ── Check 1a: Timer health (Linux) ───────────────────────────────────
// Any enabled timer that's not active is dead. An active timer with no next
// elapse is armed-for-never (the OnBootSec+OnUnitActiveSec class).
pub fn check_timers(c: &Ctx) -> Vec<String> {
    c.section("Timers");
    let mut problems = vec![];

    let r = c.exec.run("systemctl", &["--user", "list-unit-files", "--type=timer", "--state=enabled", "--no-legend"]);
    if !r.ok() {
        c.say("  ❌ Could not query timers");
        c.end_section();
        return vec!["Timers: could not query systemd (check skipped)".into()];
    }

    for timer in parse_first_column(&r.stdout) {
        if timer == SELF_TIMER {
            continue;
        }
        let name = timer.trim_end_matches(".timer").to_string();
        let status = c.exec.run("systemctl", &["--user", "is-active", &timer]).out();

        if status == "active" {
            let next = c
                .exec
                .run(
                    "systemctl",
                    &["--user", "show", &timer, "-p", "NextElapseUSecRealtime", "-p", "NextElapseUSecMonotonic", "--value"],
                )
                .out();
            if next.is_empty() {
                problems.push(format!("Timer never fires: {name} [active, no next elapse]"));
                c.say(&format!("  ❌ {name}: active but no next elapse"));
            } else {
                c.say(&format!("  ✅ {name}"));
            }
            continue;
        }

        if c.fix {
            if c.exec.run("systemctl", &["--user", "start", &timer]).ok() {
                c.log_fix(&format!("Started timer {timer}"));
                c.say(&format!("  🔧 {name}: was {status}, restarted"));
            } else {
                problems.push(format!("Timer dead: {name} [{status}]"));
                c.say(&format!("  ❌ {name}: {status} — fix failed"));
            }
        } else {
            problems.push(format!("Timer dead: {name} [{status}]"));
            c.say(&format!("  ❌ {name}: {status}"));
        }
    }

    c.end_section();
    problems
}

/// First whitespace-separated token of each non-empty line.
pub fn parse_first_column(s: &str) -> Vec<String> {
    s.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| l.split_whitespace().next().map(String::from))
        .collect()
}

// ── Check 1b: LaunchAgent health (macOS) ─────────────────────────────
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchdEntry {
    pub label: String,
    pub pid: Option<u32>,
    pub last_exit: Option<i32>,
}

/// Parse `launchctl list` (PID \t Status \t Label), keeping only our labels.
pub fn parse_launchctl_list(s: &str) -> Vec<LaunchdEntry> {
    s.lines()
        .filter(|l| l.contains("com.williamnapier.") || l.contains("com.user."))
        .filter_map(|l| {
            let parts: Vec<&str> = l.split('\t').collect();
            if parts.len() < 3 {
                return None;
            }
            let pid = if parts[0] == "-" { None } else { parts[0].trim().parse().ok() };
            let last_exit = parts[1].trim().parse().ok();
            Some(LaunchdEntry { label: parts[2].trim().to_string(), pid, last_exit })
        })
        .collect()
}

fn our_plists(agents_dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(agents_dir) else { return vec![] };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            n.ends_with(".plist") && (n.starts_with("com.williamnapier.") || n.starts_with("com.user."))
        })
        .collect();
    v.sort();
    v
}

pub fn check_launchagents(c: &Ctx) -> Vec<String> {
    c.section("LaunchAgents");
    let mut problems = vec![];

    let agents_dir = c.home.join("Library/LaunchAgents");
    if !agents_dir.exists() {
        c.say("  ❌ No LaunchAgents directory");
        c.end_section();
        return vec!["LaunchAgents: directory missing (check skipped)".into()];
    }
    let plists = our_plists(&agents_dir);
    if plists.is_empty() {
        c.say("  ❌ No managed agents found");
        c.end_section();
        return vec!["LaunchAgents: no com.williamnapier/com.user plists found (check skipped)".into()];
    }

    let loaded = parse_launchctl_list(&c.exec.run("launchctl", &["list"]).stdout);
    if loaded.is_empty() {
        c.say("  ❌ launchctl list returned nothing");
        c.end_section();
        return vec!["LaunchAgents: could not read launchctl list (check skipped)".into()];
    }

    for plist in &plists {
        let label = plist.file_name().and_then(|n| n.to_str()).unwrap_or("").trim_end_matches(".plist").to_string();
        if label == SELF_AGENT {
            continue;
        }
        let plist_s = plist.to_string_lossy();
        let info = loaded.iter().find(|e| e.label == label);

        match info {
            None if intentionally_unloaded(&label) => {
                c.say(&format!("  ⚪ {label}: deliberately not loaded on this machine"));
            }
            None => {
                if c.fix {
                    if c.exec.run("launchctl", &["load", &plist_s]).ok() {
                        c.log_fix(&format!("Loaded agent {label}"));
                        c.say(&format!("  🔧 {label}: was not loaded, loaded"));
                    } else {
                        problems.push(format!("Agent not loaded: {label}"));
                        c.say(&format!("  ❌ {label}: not loaded — fix failed"));
                    }
                } else {
                    problems.push(format!("Agent not loaded: {label}"));
                    c.say(&format!("  ❌ {label}: not loaded"));
                }
            }
            // A live PID means the agent is running now; launchd's "last exit"
            // then describes the PREVIOUS instance (e.g. -15 after a
            // `kickstart -k`), not this one. Check running before errored.
            Some(e) if e.pid.is_some() => {
                c.say(&format!("  ✅ {label}: running (pid {})", e.pid.unwrap_or(0)));
            }
            Some(e) if e.last_exit.is_some_and(|x| x != 0) => {
                let code = e.last_exit.unwrap_or(0);
                if c.fix {
                    let _ = c.exec.run("launchctl", &["unload", &plist_s]);
                    if c.exec.run("launchctl", &["load", &plist_s]).ok() {
                        c.log_fix(&format!("Reloaded agent {label}"));
                        c.say(&format!("  🔧 {label}: exit {code}, reloaded"));
                    } else {
                        problems.push(format!("Agent errored: {label} [exit {code}]"));
                        c.say(&format!("  ❌ {label}: exit {code} — fix failed"));
                    }
                } else {
                    problems.push(format!("Agent errored: {label} [exit {code}]"));
                    c.say(&format!("  ❌ {label}: exit {code}"));
                }
            }
            Some(e) => {
                let status = match e.pid {
                    Some(p) => format!("running (pid {p})"),
                    None => "loaded, idle".to_string(),
                };
                c.say(&format!("  ✅ {label}: {status}"));
            }
        }
    }

    c.end_section();
    problems
}

// ── Check 2a: Service health (Linux) ─────────────────────────────────
/// Unit names from `systemctl --state=failed --plain --no-legend`. With
/// `--plain` the first field is the unit; without it systemd prefixes a "●"
/// glyph, so drop a leading token that is not a unit name (belt and braces).
pub fn parse_failed_units(s: &str) -> Vec<String> {
    s.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            match f.first() {
                Some(first) if first.contains('.') => Some(first.to_string()),
                _ => f.get(1).map(|s| s.to_string()),
            }
        })
        .collect()
}

pub fn check_services(c: &Ctx) -> Vec<String> {
    c.section("Services");
    let mut problems = vec![];
    let mut reported: Vec<String> = vec![];

    // Part A: any unit in failed state
    let failed = c.exec.run("systemctl", &["--user", "--state=failed", "--no-legend", "--no-pager", "--plain"]);
    if !failed.ok() {
        problems.push("Services: could not query failed units (check skipped)".into());
        c.say("  ❌ Could not query failed units");
    } else {
        for unit in parse_failed_units(&failed.stdout) {
            // Self-exclusion: this tool exits 1 on any problem, which puts its
            // own unit into failed — it must not then report itself forever.
            if unit == SELF_UNIT {
                continue;
            }
            let name = unit.trim_end_matches(".service").trim_end_matches(".timer").to_string();
            if c.fix {
                if c.exec.run("systemctl", &["--user", "restart", &unit]).ok() {
                    c.log_fix(&format!("Restarted failed {unit}"));
                    c.say(&format!("  🔧 {name}: was failed, restarted"));
                    continue;
                }
                problems.push(format!("Service failed: {name}"));
                c.say(&format!("  ❌ {name}: failed — fix failed"));
            } else {
                problems.push(format!("Service failed: {name}"));
                c.say(&format!("  ❌ {name}: failed"));
            }
            reported.push(name);
        }
    }

    // Part B: key long-running services that should always be active
    for svc in ["link-service"] {
        if reported.iter().any(|n| n == svc) {
            continue;
        }
        let unit = format!("{svc}.service");
        if c.exec.run("systemctl", &["--user", "is-enabled", &unit]).out() != "enabled" {
            continue;
        }
        let status = c.exec.run("systemctl", &["--user", "is-active", &unit]).out();
        if status == "active" {
            c.say(&format!("  ✅ {svc}"));
            continue;
        }
        if c.fix {
            if c.exec.run("systemctl", &["--user", "start", &unit]).ok() {
                c.log_fix(&format!("Started service {svc}"));
                c.say(&format!("  🔧 {svc}: was {status}, started"));
            } else {
                problems.push(format!("Service dead: {svc} [{status}]"));
                c.say(&format!("  ❌ {svc}: {status} — fix failed"));
            }
        } else {
            problems.push(format!("Service dead: {svc} [{status}]"));
            c.say(&format!("  ❌ {svc}: {status}"));
        }
    }

    c.end_section();
    problems
}

// ── Check 2b: Key macOS services ─────────────────────────────────────
// Long-running agents that should have a PID. link-service spawns child
// watchers then exits, so it is deliberately not in this list.
pub fn check_mac_services(c: &Ctx) -> Vec<String> {
    c.section("Key Services");
    let mut problems = vec![];
    let agents_dir = c.home.join("Library/LaunchAgents");
    let loaded = parse_launchctl_list(&c.exec.run("launchctl", &["list"]).stdout);

    for svc in ["com.williamnapier.syncthing-monitor"] {
        let plist = agents_dir.join(format!("{svc}.plist"));
        if !plist.exists() {
            c.say(&format!("  ⚪ {svc}: no plist installed"));
            continue;
        }
        let plist_s = plist.to_string_lossy();
        let info = loaded.iter().find(|e| e.label == svc);
        match info.and_then(|e| e.pid) {
            Some(pid) => c.say(&format!("  ✅ {svc}: pid {pid}")),
            None => {
                if c.fix {
                    if info.is_some() {
                        let _ = c.exec.run("launchctl", &["unload", &plist_s]);
                    }
                    let _ = c.exec.run("launchctl", &["load", &plist_s]);
                    c.log_fix(&format!("Reloaded service {svc}"));
                    c.say(&format!("  🔧 {svc}: reloaded"));
                } else {
                    let status = if info.is_none() { "not loaded" } else { "loaded but no PID" };
                    problems.push(format!("Service not running: {svc}"));
                    c.say(&format!("  ❌ {svc}: {status}"));
                }
            }
        }
    }

    c.end_section();
    problems
}

// ── Check 3: Uncommitted dotfiles ────────────────────────────────────
/// Paths from `git status --porcelain` (columns 4.. of each line).
pub fn parse_porcelain(s: &str) -> Vec<String> {
    s.lines().filter(|l| l.len() > 3).map(|l| l[3..].to_string()).collect()
}

pub fn check_dotfiles(c: &Ctx) -> Vec<String> {
    c.section("Dotfiles");
    let dotfiles = c.home.join("dotfiles");
    if !dotfiles.exists() {
        c.say("  ~/dotfiles not found");
        c.end_section();
        return vec![];
    }
    let r = c.exec.run("git", &["-C", &dotfiles.to_string_lossy(), "status", "--porcelain"]);
    if !r.ok() || r.out().is_empty() {
        c.say("  ✅ Clean");
        c.end_section();
        return vec![];
    }
    let changed = parse_porcelain(&r.stdout);
    let now = SystemTime::now();
    let stale: Vec<&String> = changed
        .iter()
        .filter(|f| {
            fs::metadata(dotfiles.join(f))
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| now.duration_since(m).ok())
                .is_some_and(|age| age > Duration::from_secs(24 * 3600))
        })
        .collect();

    if stale.is_empty() {
        c.say(&format!("  ✅ {} changed, all recent", changed.len()));
        c.end_section();
        return vec![];
    }
    for f in &stale {
        c.say(&format!("  ⚠ {f}"));
    }
    c.end_section();
    let n = stale.len();
    vec![format!("Dotfiles: {n} uncommitted {} >24h old", if n == 1 { "file" } else { "files" })]
}

// ── Check 4: Rust tool deployment ────────────────────────────────────
// Source in rust-projects/ but binary missing from ~/.local/bin/.
// Actively-used tools only.
pub const RUST_TOOLS: &[(&str, &str)] = &[
    ("ai-brief", "ai-brief"),
    ("ai-export-watcher", "ai-export-watcher"),
    ("concert-capture", "concert-capture"),
    ("continuum-activity", "continuum-activity"),
    ("dotter-drift-monitor", "dotter-drift-monitor"),
    ("forge-metadata-backup", "forge-metadata-backup"),
    ("readwise-sync", "readwise-sync"),
    ("state-capture", "state-capture"),
    ("system-health-check", "system-health-check"),
    ("wiki-resolve-batch", "wiki-resolve-batch"),
    ("yt-transcript", "yt-transcript"),
];

pub fn check_rust_tools(c: &Ctx) -> Vec<String> {
    c.section("Rust Tools");
    let rust_dir = c.home.join("dotfiles/rust-projects");
    let bin_dir = c.home.join(".local/bin");
    let mut problems = vec![];
    for (project, binary) in RUST_TOOLS {
        if !rust_dir.join(project).exists() {
            continue;
        }
        if bin_dir.join(binary).exists() {
            c.say(&format!("  ✅ {binary}"));
        } else {
            problems.push(format!("Rust tool missing: {binary}"));
            c.say(&format!("  ❌ {binary}: source exists, no binary"));
        }
    }
    c.end_section();
    problems
}

// ── Check 5: DNA drift ───────────────────────────────────────────────
#[derive(Debug, serde::Deserialize)]
struct DnaReport {
    captures: Vec<DnaCapture>,
}
#[derive(Debug, serde::Deserialize)]
struct DnaCapture {
    name: String,
    status: String,
    #[serde(default)]
    added: Vec<String>,
    #[serde(default)]
    removed: Vec<String>,
}

pub fn check_dna_drift(c: &Ctx, is_macos: bool) -> Vec<String> {
    c.section("DNA Drift");
    let mut problems = vec![];

    // 5a: state-capture drift (packages, services, crates, groups, …)
    if !c.exec.which("state-capture") {
        problems.push("DNA: state-capture not installed".into());
        c.say("  ❌ state-capture: not in PATH");
    } else {
        let sc = c.exec.run("state-capture", &["check", "--json", "--quiet"]);
        let out = sc.out();
        if sc.ok() && !out.is_empty() {
            c.say("  ✅ state-capture (all clean)");
        } else if !out.is_empty() {
            match serde_json::from_str::<DnaReport>(&out) {
                Ok(report) => {
                    for cap in report.captures {
                        match cap.status.as_str() {
                            "drift" => {
                                let summary = format!("+{}/-{}", cap.added.len(), cap.removed.len());
                                problems.push(format!("DNA: {} drift {summary}", cap.name));
                                c.say(&format!("  ❌ {}: {summary}", cap.name));
                                for a in &cap.added {
                                    c.say(&format!("      + {a}"));
                                }
                                for r in &cap.removed {
                                    c.say(&format!("      - {r}"));
                                }
                            }
                            "nobaseline" => c.say(&format!("  ⚠ {}: no baseline", cap.name)),
                            "error" => {
                                problems.push(format!("DNA: {} check error", cap.name));
                                c.say(&format!("  ❌ {}: error", cap.name));
                            }
                            _ => c.say(&format!("  ✅ {}", cap.name)),
                        }
                    }
                }
                Err(e) => {
                    problems.push("DNA: state-capture check failed".into());
                    c.say(&format!("  ❌ state-capture: unparseable report ({e})"));
                }
            }
        } else {
            problems.push("DNA: state-capture check failed".into());
            c.say("  ❌ state-capture: command failed");
        }
    }

    // 5b: Snapper health (Linux only — structural, not a list-based capture)
    if !is_macos {
        let mut snapper_ok = true;
        let cfgs = c.exec.run("snapper", &["list-configs"]);
        if !cfgs.ok() {
            problems.push("DNA: snapper not accessible".into());
            c.say("  ❌ snapper: cannot list configs");
            snapper_ok = false;
        } else {
            for cfg in ["root", "home"] {
                if !cfgs.stdout.contains(cfg) {
                    problems.push(format!("DNA: snapper config '{cfg}' missing"));
                    c.say(&format!("  ❌ snapper: {cfg} config missing"));
                    snapper_ok = false;
                }
            }
            for timer in ["snapper-timeline.timer", "snapper-cleanup.timer"] {
                let status = c.exec.run("systemctl", &["is-enabled", timer]).out();
                if status != "enabled" {
                    problems.push(format!("DNA: {timer} not enabled"));
                    c.say(&format!("  ❌ snapper: {timer} [{status}]"));
                    snapper_ok = false;
                }
            }
        }
        if snapper_ok {
            c.say("  ✅ snapper");
        }
    }

    c.end_section();
    problems
}

// ── Check 6: Dotter drift ────────────────────────────────────────────
// Runs the stateless intent-vs-disk checker. Its self-test runs FIRST so a
// checker that can no longer fail is itself reported, not trusted.
pub fn parse_drift_lines(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|l| l.starts_with("❌"))
        .map(|l| l.trim_start_matches("❌").trim().to_string())
        .collect()
}

pub fn check_dotter_drift(c: &Ctx) -> Vec<String> {
    c.section("Dotter Drift");
    let mut problems = vec![];

    if !c.exec.which("dotter-drift-monitor") {
        c.say("  ❌ dotter-drift-monitor not in PATH");
        c.end_section();
        return vec!["Dotter: dotter-drift-monitor not in PATH (check skipped)".into()];
    }

    if !c.exec.run("dotter-drift-monitor", &["--self-test"]).ok() {
        problems.push("Dotter: drift checker FAILED its self-test — its results cannot be trusted".into());
        c.say("  ❌ self-test failed");
    } else {
        c.say("  ✅ self-test (checker can fail)");
    }

    let r = c.exec.run("dotter-drift-monitor", &["--quiet"]);
    match r.exit_code {
        0 => c.say("  ✅ deployed state matches global.toml"),
        1 => {
            let lines = parse_drift_lines(&r.stdout);
            for l in &lines {
                problems.push(format!("Dotter drift: {l}"));
                c.say(&format!("  ❌ {l}"));
            }
            if lines.is_empty() {
                problems.push("Dotter drift: reported but no detail lines parsed".into());
            }
        }
        2 => {
            problems.push("Dotter: could not check (config unreadable or zero mappings)".into());
            c.say("  ❌ could not check");
        }
        n => {
            problems.push(format!("Dotter: checker crashed [exit {n}]"));
            c.say(&format!("  ❌ checker crashed (exit {n})"));
        }
    }

    c.end_section();
    problems
}


// ── Check 7: Nushell `watch` debounce flag ───────────────────────────
// nu 0.107 deprecated `watch --debounce-ms <int>` in favour of `--debounce
// <duration>`, and 0.109 removes it. But 0.106 has no `--debounce`, and a
// script is parsed whole, so the scripts cannot switch until EVERY host is
// ≥ 0.107 — a condition no single machine can see. Each host publishes its
// nu version in its health status file (~/Assistants/health/<host>.json,
// Syncthing-carried); this check reads the peers' files and fires the
// go-ahead the day the last host crosses the floor, and an alarm if this
// host reaches the removal version first. Added 2026-09-02 at Will's request.

pub const NU_WATCH_FLAG_FLOOR: (u32, u32) = (0, 107);
pub const NU_WATCH_FLAG_REMOVED: (u32, u32) = (0, 109);
const OLD_WATCH_FLAG: &str = "--debounce-ms";

/// "0.107.0" → (0, 107). Anything unparseable → None.
pub fn nu_version_pair(v: &str) -> Option<(u32, u32)> {
    let mut it = v.trim().split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    Some((major, minor))
}

/// (host, nu_version) for every ~/Assistants/health/*.json except our own.
/// A peer file without `nu_version` (older tool) is (host, None): unknown,
/// which the verdict treats as "not ready".
pub fn peer_nu_versions(health_dir: &Path, own_host: &str) -> Vec<(String, Option<String>)> {
    let mut out = vec![];
    let Ok(rd) = fs::read_dir(health_dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&p) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        let host = v.get("host").and_then(|h| h.as_str()).unwrap_or("").to_string();
        if host.is_empty() || host == own_host {
            continue;
        }
        let nu = v.get("nu_version").and_then(|h| h.as_str()).map(|s| s.trim().to_string());
        out.push((host, nu));
    }
    out.sort();
    out
}

/// Script files (one level, regular files) under `dir` still passing the old
/// flag. None when the directory cannot be read — that is a problem, not a
/// clean result.
pub fn scripts_using_old_flag(dir: &Path) -> Option<Vec<String>> {
    let rd = fs::read_dir(dir).ok()?;
    let mut out = vec![];
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&p) {
            if text.contains(OLD_WATCH_FLAG) {
                if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
                    out.push(n.to_string());
                }
            }
        }
    }
    out.sort();
    Some(out)
}

/// The decision, pure so it can be tested.
pub fn watch_flag_verdict(local: Option<&str>, peers: &[(String, Option<String>)], offending: &[String]) -> Vec<String> {
    if offending.is_empty() {
        return vec![];
    }
    let n = offending.len();
    let list = offending.join(", ");
    let Some(local_v) = local.and_then(nu_version_pair) else {
        return vec![format!("Nushell: could not read the local nu version (watch-flag check skipped; {n} scripts use `{OLD_WATCH_FLAG}`)")];
    };
    let local_s = local.unwrap().trim().to_string();
    if local_v >= NU_WATCH_FLAG_REMOVED {
        return vec![format!("Nushell {local_s}: `watch {OLD_WATCH_FLAG}` was removed in 0.109 — {n} scripts will fail: {list}")];
    }
    // One host cannot speak for the fleet: no peer file means not ready.
    let mut ready = local_v >= NU_WATCH_FLAG_FLOOR && !peers.is_empty();
    let mut hosts = vec![format!("this host {local_s}")];
    for (host, v) in peers {
        match v.as_deref().and_then(nu_version_pair) {
            Some(pv) if pv >= NU_WATCH_FLAG_FLOOR => hosts.push(format!("{host} {}", v.as_deref().unwrap())),
            _ => ready = false,
        }
    }
    if ready {
        vec![format!(
            "Nushell ≥ 0.107 on every host ({}) — switch `watch {OLD_WATCH_FLAG}` to `--debounce` in {n} scripts: {list}",
            hosts.join(", ")
        )]
    } else {
        vec![]
    }
}

pub fn check_nu_watch_flag(c: &Ctx, local_nu: Option<&str>, own_host: &str) -> Vec<String> {
    c.section("Nushell watch flag");
    let scripts_dir = c.home.join("dotfiles/scripts");
    let Some(offending) = scripts_using_old_flag(&scripts_dir) else {
        c.say(&format!("  ❌ could not read {}", scripts_dir.display()));
        c.end_section();
        return vec![format!("Nushell: could not read {} (watch-flag check skipped)", scripts_dir.display())];
    };
    let peers = peer_nu_versions(&c.home.join("Assistants/health"), own_host);
    c.say(&format!("  local nu: {}", local_nu.map(str::trim).unwrap_or("unknown")));
    for (h, v) in &peers {
        c.say(&format!("  peer {h}: {}", v.as_deref().unwrap_or("unknown")));
    }
    c.say(&format!("  scripts using `{OLD_WATCH_FLAG}`: {}", offending.len()));
    let problems = watch_flag_verdict(local_nu, &peers, &offending);
    if problems.is_empty() {
        c.say("  ✅ nothing to do yet");
    } else {
        for p in &problems {
            c.say(&format!("  ❌ {p}"));
        }
    }
    c.end_section();
    problems
}

// ---------------------------------------------------------------------------
// ── Check 8: derived documents ───────────────────────────────────────
/// Documents rendered from a Markdown source for a human audience (e.g. the
/// household photo guide). The Markdown is canonical and doc-gated; the render
/// is mechanical. A rendered file older than its source is a stale page that
/// someone may be reading, so it is a problem, not a note. A missing render is
/// also a problem: the page has never been produced on this machine's copy.
const DERIVED_DOCS: &[(&str, &str)] = &[(
    "Assistants/shared/PHOTO-SYSTEM-GUIDE.md",
    "Assistants/shared/PHOTO-SYSTEM-GUIDE.html",
)];

pub fn check_derived_docs(c: &Ctx) -> Vec<String> {
    c.section("Derived Documents");
    let problems = derived_doc_problems(&c.home, DERIVED_DOCS);
    for p in &problems {
        c.say(&format!("  ❌ {p}"));
    }
    if problems.is_empty() {
        for (src, _) in DERIVED_DOCS {
            c.say(&format!("  ✅ {src} render is current"));
        }
    }
    c.end_section();
    problems
}

pub fn derived_doc_problems(home: &Path, pairs: &[(&str, &str)]) -> Vec<String> {
    let mut problems = vec![];
    for (src, out) in pairs {
        let (sp, op) = (home.join(src), home.join(out));
        let name = Path::new(src).file_name().unwrap_or_default().to_string_lossy().into_owned();
        let out_name = Path::new(out).file_name().unwrap_or_default().to_string_lossy().into_owned();
        let Ok(sm) = fs::metadata(&sp).and_then(|m| m.modified()) else {
            problems.push(format!("Derived doc source missing or unreadable: {name}"));
            continue;
        };
        match fs::metadata(&op).and_then(|m| m.modified()) {
            Ok(om) if om >= sm => {}
            Ok(_) => problems.push(format!("Derived doc stale: {out_name} is older than {name} — re-render and republish")),
            Err(_) => problems.push(format!("Derived doc missing: {out_name} has never been rendered from {name}")),
        }
    }
    problems
}


// ── Check 9: watcher heartbeats ──────────────────────────────────────
// Since 2026-09-02 every long-running watcher (the Rust ports) writes
// ~/.local/state/watchers/<name>.json at startup and after every
// cycle/event: {watcher, version, started_at, last_cycle, last_action,
// actions, last_error, host, interval_secs}. This is the answer to the audit
// question "what artifact proves it is alive?" — a watcher that is running but
// has silently stopped doing its job (the Domain 7 class) shows up here as a
// stale cycle or a recorded error, without anyone reading its logs.
//
// Rules: a heartbeat with last_error → problem. interval_secs > 0 and
// last_cycle older than max(3×interval, 15 min) → dead or hung. An expected
// heartbeat that does not exist → never checked in (deployed but not running,
// or never deployed). Event-driven watchers (interval_secs 0) are exempt from
// the staleness rule: a quiet Forge is not a dead watcher.

#[cfg(target_os = "macos")]
pub const EXPECTED_WATCHERS: &[&str] = &[
    "git-auto-push-watcher-dotfiles",
    "git-auto-push-watcher-Assistants",
    "git-auto-pull-watcher",
    "syncthing-connection-monitor",
    "dotter-realtime-watcher",
    "forge-md-revs",
    "collect-projects-watcher",
    "zotero-watcher-pdf",
    "wiki-link-service-backlinks",
    "wiki-link-service-resolve-mark",
];
#[cfg(not(target_os = "macos"))]
pub const EXPECTED_WATCHERS: &[&str] = &[
    "git-auto-push-watcher-dotfiles",
    "git-auto-push-watcher-Assistants",
    "git-auto-pull-watcher",
    "dotter-realtime-watcher",
    "forge-md-revs",
    "wiki-link-service-backlinks",
    "wiki-link-service-resolve-mark",
];

pub struct HeartbeatFile {
    pub name: String,
    pub value: serde_json::Value,
}

pub fn read_heartbeats(dir: &Path) -> Vec<HeartbeatFile> {
    let mut out = vec![];
    let Ok(rd) = fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = p.file_stem().and_then(|n| n.to_str()).map(String::from) else { continue };
        match fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
            Some(value) => out.push(HeartbeatFile { name, value }),
            None => out.push(HeartbeatFile { name, value: serde_json::Value::Null }),
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Pure verdict. `last_cycle` is parsed as RFC 3339.
pub fn heartbeat_verdict(now: chrono::DateTime<chrono::Local>, files: &[HeartbeatFile], expected: &[&str]) -> Vec<String> {
    let mut problems = vec![];
    for f in files {
        if f.value.is_null() {
            problems.push(format!("Watcher {}: heartbeat file is not valid JSON", f.name));
            continue;
        }
        if let Some(err) = f.value.get("last_error").and_then(|v| v.as_str()) {
            problems.push(format!("Watcher {}: last error: {err}", f.name));
        }
        let interval = f.value.get("interval_secs").and_then(|v| v.as_u64()).unwrap_or(0);
        if interval > 0 {
            let last = f.value.get("last_cycle").and_then(|v| v.as_str()).and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
            match last {
                Some(t) => {
                    let age = now.signed_duration_since(t.with_timezone(&chrono::Local));
                    let allowed = std::cmp::max(3 * interval as i64, 900);
                    if age.num_seconds() > allowed {
                        problems.push(format!("Watcher {}: last cycle {} min ago (interval {interval}s) — dead or hung", f.name, age.num_minutes()));
                    }
                }
                None => problems.push(format!("Watcher {}: heartbeat has no parseable last_cycle", f.name)),
            }
        }
    }
    for name in expected {
        if !files.iter().any(|f| &f.name == name) {
            problems.push(format!("Watcher {name}: no heartbeat — never checked in on this machine"));
        }
    }
    problems
}

pub fn check_watcher_heartbeats(c: &Ctx) -> Vec<String> {
    c.section("Watcher Heartbeats");
    let dir = c.home.join(".local/state/watchers");
    let files = read_heartbeats(&dir);
    let problems = heartbeat_verdict(chrono::Local::now(), &files, EXPECTED_WATCHERS);
    c.say(&format!("  {} heartbeat files in {}", files.len(), dir.display()));
    for f in &files {
        let cycle = f.value.get("last_cycle").and_then(|v| v.as_str()).unwrap_or("?");
        let actions = f.value.get("actions").and_then(|v| v.as_u64()).unwrap_or(0);
        c.say(&format!("  {} — last cycle {cycle}, {actions} actions", f.name));
    }
    if problems.is_empty() {
        c.say("  ✅ every expected watcher has checked in, none stale, no errors");
    } else {
        for p in &problems {
            c.say(&format!("  ❌ {p}"));
        }
    }
    c.end_section();
    problems
}

#[cfg(test)]
mod tests {

    #[test]
    fn derived_doc_stale_missing_and_current_are_told_apart() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        fs::create_dir_all(home.join("d")).unwrap();
        let pairs: &[(&str, &str)] = &[("d/a.md", "d/a.html")];
        // missing render
        fs::write(home.join("d/a.md"), "x").unwrap();
        let p = derived_doc_problems(home, pairs);
        assert_eq!(p.len(), 1);
        assert!(p[0].contains("never been rendered"), "{p:?}");
        // stale render: html older than md
        fs::write(home.join("d/a.html"), "y").unwrap();
        let old = SystemTime::now() - Duration::from_secs(600);
        let f = fs::File::options().write(true).open(home.join("d/a.html")).unwrap();
        f.set_modified(old).unwrap();
        let p = derived_doc_problems(home, pairs);
        assert_eq!(p.len(), 1);
        assert!(p[0].contains("stale"), "{p:?}");
        // current render
        f.set_modified(SystemTime::now() + Duration::from_secs(1)).unwrap();
        assert!(derived_doc_problems(home, pairs).is_empty());
        // missing source
        assert!(derived_doc_problems(home, &[("d/none.md", "d/none.html")])[0].contains("source missing"));
    }
    use super::*;
    use crate::exec::{CmdResult, Fake};

    fn ctx<'a>(exec: &'a Fake, fix: bool, log: &'a dyn Fn(&str, &str)) -> Ctx<'a> {
        Ctx { exec, verbose: false, fix, home: PathBuf::from("/nonexistent-home"), log }
    }
    fn nolog(_: &str, _: &str) {}

    #[test]
    fn launchctl_list_parses_pid_exit_and_label() {
        let out = "PID\tStatus\tLabel\n1726\t0\tcom.user.x\n-\t78\tcom.williamnapier.state-capture\n123\t0\tcom.apple.foo\n";
        let e = parse_launchctl_list(out);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0], LaunchdEntry { label: "com.user.x".into(), pid: Some(1726), last_exit: Some(0) });
        assert_eq!(e[1], LaunchdEntry { label: "com.williamnapier.state-capture".into(), pid: None, last_exit: Some(78) });
    }

    #[test]
    fn failed_units_survive_the_bullet_glyph() {
        assert_eq!(parse_failed_units("● foo.service loaded failed failed X\nbar.timer loaded failed failed Y\n"), vec!["foo.service", "bar.timer"]);
    }

    #[test]
    fn timers_query_failure_is_a_problem_not_silence() {
        let mut f = Fake::default();
        f.respond("systemctl", &["--user", "list-unit-files", "--type=timer", "--state=enabled", "--no-legend"], CmdResult::failure(1, "no bus"));
        let p = check_timers(&ctx(&f, false, &nolog));
        assert_eq!(p, vec!["Timers: could not query systemd (check skipped)"]);
    }

    #[test]
    fn active_timer_with_no_next_elapse_is_reported() {
        let mut f = Fake::default();
        f.respond("systemctl", &["--user", "list-unit-files", "--type=timer", "--state=enabled", "--no-legend"], CmdResult::success("dead.timer enabled enabled\nfine.timer enabled enabled\nsystem-health-check.timer enabled enabled\n"));
        f.respond("systemctl", &["--user", "is-active", "dead.timer"], CmdResult::success("active\n"));
        f.respond("systemctl", &["--user", "is-active", "fine.timer"], CmdResult::success("active\n"));
        f.respond("systemctl", &["--user", "show", "dead.timer", "-p", "NextElapseUSecRealtime", "-p", "NextElapseUSecMonotonic", "--value"], CmdResult::success("\n\n"));
        f.respond("systemctl", &["--user", "show", "fine.timer", "-p", "NextElapseUSecRealtime", "-p", "NextElapseUSecMonotonic", "--value"], CmdResult::success("Tue 2026-09-02 06:00:00 BST\n\n"));
        let p = check_timers(&ctx(&f, false, &nolog));
        assert_eq!(p, vec!["Timer never fires: dead [active, no next elapse]"]);
    }

    #[test]
    fn inactive_timer_is_started_with_fix_and_logged() {
        let mut f = Fake::default();
        f.respond("systemctl", &["--user", "list-unit-files", "--type=timer", "--state=enabled", "--no-legend"], CmdResult::success("x.timer enabled enabled\n"));
        f.respond("systemctl", &["--user", "is-active", "x.timer"], CmdResult::success("inactive\n"));
        f.respond("systemctl", &["--user", "start", "x.timer"], CmdResult::success(""));
        let logged = std::cell::RefCell::new(vec![]);
        let log = |lvl: &str, msg: &str| logged.borrow_mut().push(format!("{lvl} {msg}"));
        let p = check_timers(&ctx(&f, true, &log));
        assert!(p.is_empty());
        assert_eq!(logged.borrow().as_slice(), ["FIX Started timer x.timer"]);
    }

    #[test]
    fn failed_service_excludes_self_and_reports_others() {
        let mut f = Fake::default();
        f.respond("systemctl", &["--user", "--state=failed", "--no-legend", "--no-pager", "--plain"], CmdResult::success("system-health-check.service loaded failed failed X\ncross-machine-sync-check.service loaded failed failed Y\n"));
        f.respond("systemctl", &["--user", "is-enabled", "link-service.service"], CmdResult::success("enabled\n"));
        f.respond("systemctl", &["--user", "is-active", "link-service.service"], CmdResult::success("active\n"));
        let p = check_services(&ctx(&f, false, &nolog));
        assert_eq!(p, vec!["Service failed: cross-machine-sync-check"]);
    }

    #[test]
    fn dna_missing_binary_and_drift_report() {
        let f = Fake::default();
        assert_eq!(check_dna_drift(&ctx(&f, false, &nolog), true), vec!["DNA: state-capture not installed"]);

        let mut f = Fake::default();
        f.on_path.push("state-capture".into());
        f.respond(
            "state-capture",
            &["check", "--json", "--quiet"],
            CmdResult { exit_code: 1, stdout: r#"{"captures":[{"name":"brew","status":"drift","added":["a","b"],"removed":[]},{"name":"groups","status":"clean"},{"name":"x","status":"error"}]}"#.into(), stderr: String::new() },
        );
        assert_eq!(check_dna_drift(&ctx(&f, false, &nolog), true), vec!["DNA: brew drift +2/-0", "DNA: x check error"]);
    }

    #[test]
    fn dotter_drift_lines_become_problems_and_selftest_failure_is_reported() {
        let mut f = Fake::default();
        f.on_path.push("dotter-drift-monitor".into());
        f.respond("dotter-drift-monitor", &["--self-test"], CmdResult::failure(1, ""));
        f.respond("dotter-drift-monitor", &["--quiet"], CmdResult { exit_code: 1, stdout: "❌ NOT-SYMLINK   ~/.codex/config.toml  (codex/config.toml) [file]\n🚨 1 drifted — checked 357 deployed files, 356 OK\n".into(), stderr: String::new() });
        let p = check_dotter_drift(&ctx(&f, false, &nolog));
        assert_eq!(
            p,
            vec![
                "Dotter: drift checker FAILED its self-test — its results cannot be trusted",
                "Dotter drift: NOT-SYMLINK   ~/.codex/config.toml  (codex/config.toml) [file]"
            ]
        );

        let mut f = Fake::default();
        f.on_path.push("dotter-drift-monitor".into());
        f.respond("dotter-drift-monitor", &["--self-test"], CmdResult::success(""));
        f.respond("dotter-drift-monitor", &["--quiet"], CmdResult::failure(2, ""));
        assert_eq!(check_dotter_drift(&ctx(&f, false, &nolog)), vec!["Dotter: could not check (config unreadable or zero mappings)"]);
    }

    #[test]
    fn intentional_divergence_register_is_platform_scoped() {
        if cfg!(target_os = "macos") {
            assert!(intentionally_unloaded("com.williamnapier.forum-worker"));
        } else {
            assert!(!intentionally_unloaded("com.williamnapier.forum-worker"));
        }
        assert!(!intentionally_unloaded("com.williamnapier.gmpull"));
    }

    #[test]
    fn running_agent_with_signal_last_exit_is_not_errored() {
        // launchctl shows the previous instance's exit (-15 from kickstart -k)
        // next to the live PID; the live PID wins.
        let e = parse_launchctl_list("18682\t-15\tcom.williamnapier.forge-md-revs\n");
        assert_eq!(e[0].pid, Some(18682));
        assert_eq!(e[0].last_exit, Some(-15));
        // (the arm order in check_launchagents is what enforces this; the
        // parse test pins the inputs it relies on)
    }

    #[test]
    fn porcelain_paths() {
        assert_eq!(parse_porcelain(" M a/b.txt\n?? new dir/\n"), vec!["a/b.txt", "new dir/"]);
    }

    fn peers(v: &[(&str, Option<&str>)]) -> Vec<(String, Option<String>)> {
        v.iter().map(|(h, n)| (h.to_string(), n.map(String::from))).collect()
    }
    fn offending() -> Vec<String> {
        vec!["a-watcher".into(), "b-watcher".into()]
    }

    #[test]
    fn nu_version_pair_parses_and_rejects() {
        assert_eq!(nu_version_pair("0.107.0\n"), Some((0, 107)));
        assert_eq!(nu_version_pair("1.2"), Some((1, 2)));
        assert_eq!(nu_version_pair("nu 0.1"), None);
        assert_eq!(nu_version_pair(""), None);
    }

    #[test]
    fn watch_flag_silent_while_any_host_is_below_the_floor() {
        assert!(watch_flag_verdict(Some("0.106.1"), &peers(&[("nimbini", Some("0.107.0"))]), &offending()).is_empty());
        assert!(watch_flag_verdict(Some("0.107.1"), &peers(&[("nimbini", Some("0.106.1"))]), &offending()).is_empty());
    }

    #[test]
    fn watch_flag_fires_when_every_host_is_at_the_floor() {
        let p = watch_flag_verdict(Some("0.107.1\n"), &peers(&[("nimbini", Some("0.107.0"))]), &offending());
        assert_eq!(p.len(), 1);
        assert!(p[0].contains("this host 0.107.1") && p[0].contains("nimbini 0.107.0"), "{}", p[0]);
        assert!(p[0].contains("2 scripts: a-watcher, b-watcher"), "{}", p[0]);
    }

    #[test]
    fn watch_flag_unknown_or_missing_peer_is_not_ready() {
        assert!(watch_flag_verdict(Some("0.107.0"), &peers(&[("nimbini", None)]), &offending()).is_empty());
        assert!(watch_flag_verdict(Some("0.107.0"), &[], &offending()).is_empty());
    }

    #[test]
    fn watch_flag_removal_alarm_does_not_wait_for_peers() {
        let p = watch_flag_verdict(Some("0.109.0"), &peers(&[("nimbini", Some("0.106.1"))]), &offending());
        assert_eq!(p.len(), 1);
        assert!(p[0].contains("removed in 0.109") && p[0].contains("2 scripts will fail"), "{}", p[0]);
    }

    #[test]
    fn watch_flag_nothing_offending_is_clean_and_unknown_local_is_a_problem() {
        assert!(watch_flag_verdict(Some("0.109.0"), &peers(&[("nimbini", Some("0.109.0"))]), &[]).is_empty());
        let p = watch_flag_verdict(None, &[], &offending());
        assert_eq!(p.len(), 1);
        assert!(p[0].contains("could not read the local nu version"), "{}", p[0]);
    }

    fn hb(name: &str, json: &str) -> HeartbeatFile {
        HeartbeatFile { name: name.into(), value: serde_json::from_str(json).unwrap() }
    }

    #[test]
    fn heartbeat_error_stale_and_missing_are_problems_event_driven_is_exempt() {
        let now = chrono::Local::now();
        let fresh = now.to_rfc3339();
        let old = (now - chrono::Duration::minutes(40)).to_rfc3339();
        let files = vec![
            hb("git-auto-pull-watcher", &format!(r#"{{"last_cycle":"{fresh}","interval_secs":120,"last_error":null}}"#)),
            hb("git-auto-push-watcher-dotfiles", &format!(r#"{{"last_cycle":"{old}","interval_secs":120,"last_error":null}}"#)),
            hb("forge-md-revs", &format!(r#"{{"last_cycle":"{old}","interval_secs":0,"last_error":null}}"#)),
            hb("zotero-watcher-pdf", &format!(r#"{{"last_cycle":"{fresh}","interval_secs":0,"last_error":"import failed: x"}}"#)),
            HeartbeatFile { name: "broken".into(), value: serde_json::Value::Null },
        ];
        let p = heartbeat_verdict(now, &files, &["git-auto-pull-watcher", "forge-md-revs", "wiki-link-service-backlinks"]);
        assert!(p.iter().any(|m| m.starts_with("Watcher git-auto-push-watcher-dotfiles: last cycle 40 min ago")), "{p:?}");
        assert!(p.iter().any(|m| m == "Watcher zotero-watcher-pdf: last error: import failed: x"), "{p:?}");
        assert!(p.iter().any(|m| m == "Watcher broken: heartbeat file is not valid JSON"), "{p:?}");
        assert!(p.iter().any(|m| m == "Watcher wiki-link-service-backlinks: no heartbeat — never checked in on this machine"), "{p:?}");
        assert!(!p.iter().any(|m| m.contains("forge-md-revs")), "event-driven must not be stale: {p:?}");
        assert!(!p.iter().any(|m| m.contains("git-auto-pull-watcher:")), "{p:?}");
        assert_eq!(p.len(), 4, "{p:?}");
    }

    #[test]
    fn heartbeat_staleness_floor_is_fifteen_minutes() {
        let now = chrono::Local::now();
        let ten_min = (now - chrono::Duration::minutes(10)).to_rfc3339();
        let files = vec![hb("git-auto-pull-watcher", &format!(r#"{{"last_cycle":"{ten_min}","interval_secs":120}}"#))];
        assert!(heartbeat_verdict(now, &files, &[]).is_empty());
    }
}
