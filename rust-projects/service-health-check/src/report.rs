//! The five actions. Output shape follows the Nushell original so the eye
//! (and any grep) finds the same groups; exit status is the verdict.

use crate::services::{discover, Ctx, Service};
use chrono::{DateTime, Local};
use std::path::Path;

// ── Watcher heartbeats ───────────────────────────────────────────────
// Every long-running watcher writes ~/.local/state/watchers/<name>.json at
// startup and after every cycle/event: {watcher, version, started_at,
// last_cycle, last_action, actions, last_error, host, interval_secs}. Same
// rules as system-health-check Check 9: last_error → problem; interval_secs > 0
// and last_cycle older than max(3×interval, 15 min) → dead or hung;
// interval_secs 0 is event-driven and exempt from staleness (a quiet Forge is
// not a dead watcher). The expected-watcher register stays with
// system-health-check; this report describes what is present.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Beat {
    /// Cycling on time.
    Alive { age_min: i64, interval_secs: u64 },
    /// Event-driven; age is informational only.
    EventDriven { age_min: Option<i64> },
    /// interval_secs > 0 and last_cycle beyond the allowance.
    Stale { age_min: i64, interval_secs: u64 },
    /// last_error recorded by the watcher itself.
    Error(String),
    /// Not JSON, or no parseable last_cycle where one is required.
    Unreadable(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heartbeat {
    pub name: String,
    pub beat: Beat,
}

impl Heartbeat {
    pub fn problem(&self) -> bool {
        !matches!(self.beat, Beat::Alive { .. } | Beat::EventDriven { .. })
    }
}

/// Pure classification of one heartbeat document at `now`.
pub fn classify_heartbeat(now: DateTime<Local>, value: Option<&serde_json::Value>) -> Beat {
    let Some(v) = value else { return Beat::Unreadable("not valid JSON") };
    if let Some(err) = v.get("last_error").and_then(|e| e.as_str()) {
        return Beat::Error(err.to_string());
    }
    let interval = v.get("interval_secs").and_then(|i| i.as_u64()).unwrap_or(0);
    let last = v
        .get("last_cycle")
        .and_then(|c| c.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| now.signed_duration_since(t.with_timezone(&Local)));
    if interval == 0 {
        return Beat::EventDriven { age_min: last.map(|a| a.num_minutes()) };
    }
    let Some(age) = last else { return Beat::Unreadable("no parseable last_cycle") };
    let allowed = std::cmp::max(3 * interval as i64, 900);
    if age.num_seconds() > allowed {
        Beat::Stale { age_min: age.num_minutes(), interval_secs: interval }
    } else {
        Beat::Alive { age_min: age.num_minutes(), interval_secs: interval }
    }
}

pub fn read_heartbeats(dir: &Path, now: DateTime<Local>) -> Vec<Heartbeat> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut out: Vec<Heartbeat> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .filter_map(|p| {
            let name = p.file_stem()?.to_str()?.to_string();
            let value = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
            Some(Heartbeat { name, beat: classify_heartbeat(now, value.as_ref()) })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn heartbeat_line(h: &Heartbeat) -> String {
    match &h.beat {
        Beat::Alive { age_min, interval_secs } => format!("  🟢 {} — last cycle {age_min}m ago (every {interval_secs}s)", h.name),
        Beat::EventDriven { age_min: Some(m) } => format!("  🟢 {} — event-driven, last event {m}m ago", h.name),
        Beat::EventDriven { age_min: None } => format!("  🟢 {} — event-driven, no event yet", h.name),
        Beat::Stale { age_min, interval_secs } => format!("  🔴 {} — STALE, last cycle {age_min}m ago (every {interval_secs}s) — dead or hung", h.name),
        Beat::Error(e) => format!("  🔴 {} — last error: {e}", h.name),
        Beat::Unreadable(why) => format!("  🔴 {} — {why}", h.name),
    }
}

// ── Verdict ──────────────────────────────────────────────────────────

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub healthy: usize,
    pub degraded: usize,
    pub broken: usize,
    pub missing_script: usize,
    pub reporting: usize,
    pub bad_heartbeats: usize,
}

impl Summary {
    /// 🔴 — the exit-1 condition. A dead watcher is a broken service.
    pub fn red(&self) -> bool {
        self.broken > 0 || self.missing_script > 0 || self.bad_heartbeats > 0
    }
    pub fn green(&self) -> bool {
        !self.red() && self.reporting == 0 && self.degraded == 0
    }
}

/// Same bookkeeping as the full report, without printing — so the verdict can
/// be tested against known-red and known-green inputs.
pub fn summarise(services: &[Service], beats: &[Heartbeat]) -> Summary {
    let mut s = Summary::default();
    for svc in services {
        if svc.running() || svc.loaded_ok() {
            if svc.script_exists {
                s.healthy += 1;
            } else {
                s.degraded += 1;
                s.missing_script += 1;
            }
        } else if svc.reports_issues() {
            s.reporting += 1;
            s.healthy += 1;
        } else if svc.errored() {
            s.broken += 1;
            if !svc.script_exists {
                s.missing_script += 1;
            }
        } else if !svc.loaded {
            // Deployed-but-not-enabled is how a single-writer decision is
            // expressed (WATCHERS.md); a unit deliberately left disabled on
            // this host naturally has no binary here either. Degraded, not
            // a missing-script red — that is reserved for units that would
            // run it.
            s.degraded += 1;
        }
    }
    s.bad_heartbeats = beats.iter().filter(|b| b.problem()).count();
    s
}

fn exit_desc(code: i32, long: bool) -> String {
    match (code, long) {
        (1, false) => "error".into(),
        (1, true) => "script returned error".into(),
        (78, false) => "script not found".into(),
        (78, true) => "script not found by launchd".into(),
        (127, false) => "command not found".into(),
        (127, true) => "command not found in PATH".into(),
        (c, false) => format!("exit {c}"),
        (c, true) => format!("exit code {c}"),
    }
}

fn missing_tag(svc: &Service) -> &'static str {
    if svc.script_exists { "" } else { " [SCRIPT MISSING]" }
}

fn when(svc: &Service) -> String {
    if svc.last_exit_at.is_empty() { String::new() } else { format!(" [{}]", svc.last_exit_at) }
}

fn heartbeat_dir(c: &Ctx) -> std::path::PathBuf {
    c.home.join(".local/state/watchers")
}

// ── Actions ──────────────────────────────────────────────────────────

pub fn full_check(c: &Ctx) -> bool {
    let platform = if cfg!(target_os = "macos") { "macOS" } else { "Linux" };
    println!("service-health-check — {platform}\n");

    let services = discover(c);
    if services.is_empty() {
        println!("No services discovered.");
        return false;
    }
    println!("Found {} services\n", services.len());

    let running: Vec<&Service> = services.iter().filter(|s| s.running()).collect();
    if !running.is_empty() {
        println!("── Running ({}) ──", running.len());
        for svc in &running {
            println!("  ✅ {}{}", svc.label(), missing_tag(svc));
        }
        println!();
    }

    let loaded_ok: Vec<&Service> = services.iter().filter(|s| s.loaded_ok()).collect();
    if !loaded_ok.is_empty() {
        println!("── Loaded, last exit OK ({}) ──", loaded_ok.len());
        for svc in &loaded_ok {
            println!("  ✅ {}{}{}", svc.label(), when(svc), missing_tag(svc));
        }
        println!();
    }

    let reporting: Vec<&Service> = services.iter().filter(|s| s.reports_issues()).collect();
    if !reporting.is_empty() {
        println!("── Checkers reporting issues ({}) ──", reporting.len());
        for svc in &reporting {
            println!("  🟡 {} — exit 1 = found something; run it for the detail{}", svc.label(), when(svc));
        }
        println!();
    }

    let errored: Vec<&Service> = services.iter().filter(|s| s.errored()).collect();
    if !errored.is_empty() {
        println!("── Errored ({}) ──", errored.len());
        for svc in &errored {
            println!("  ❌ {} — {}{}{}", svc.label(), exit_desc(svc.last_exit.unwrap_or(1), false), when(svc), missing_tag(svc));
        }
        println!();
    }

    let not_loaded: Vec<&Service> = services.iter().filter(|s| !s.loaded).collect();
    if !not_loaded.is_empty() {
        println!("── Not loaded ({}) ──", not_loaded.len());
        for svc in &not_loaded {
            let absent = if svc.script_exists { "" } else { " (binary absent here too)" };
            println!("  ⚪ {}{}", svc.label(), absent);
        }
        println!();
    }

    let beats = read_heartbeats(&heartbeat_dir(c), Local::now());
    println!("── Watcher heartbeats ({}) ──", beats.len());
    if beats.is_empty() {
        println!("  No heartbeat files in {}", heartbeat_dir(c).display());
    }
    for h in &beats {
        println!("{}", heartbeat_line(h));
    }

    let s = summarise(&services, &beats);
    println!("\n── Summary ──");
    println!("  Healthy:         {}", s.healthy);
    println!("  Degraded:        {}", s.degraded);
    println!("  Broken:          {}", s.broken);
    println!("  Missing scripts: {}", s.missing_script);
    println!("  Reporting:       {}", s.reporting);
    println!("  Bad heartbeats:  {}", s.bad_heartbeats);

    if s.green() {
        println!("\n🟢 All services healthy");
    } else if s.red() {
        println!("\n🔴 Issues require attention — run `service-health-check fix` for guidance");
    } else {
        println!("\n🟡 Minor issues detected");
    }
    !s.red()
}

pub fn quick_check(c: &Ctx) -> bool {
    let services = discover(c);
    let beats = read_heartbeats(&heartbeat_dir(c), Local::now());
    let s = summarise(&services, &beats);
    let running = services.iter().filter(|x| x.running()).count();
    let loaded_ok = services.iter().filter(|x| x.loaded_ok()).count();
    let not_loaded = services.iter().filter(|x| !x.loaded).count();

    println!(
        "Services: {} total | {running} running | {loaded_ok} ok | {} errored | {} reporting | {not_loaded} unloaded",
        services.len(),
        s.broken,
        s.reporting
    );
    println!("Scripts:  {} missing", s.missing_script);
    println!("Watchers: {} heartbeats, {} bad", beats.len(), s.bad_heartbeats);

    if s.red() {
        println!("🔴 Issues detected");
        false
    } else if s.green() {
        println!("🟢 Healthy");
        true
    } else {
        println!("🟡 Minor issues");
        true
    }
}

pub fn check_missing_scripts(c: &Ctx) -> bool {
    let services: Vec<Service> = discover(c).into_iter().filter(|s| !s.script_exists && s.loaded).collect();
    if services.is_empty() {
        println!("✅ All service scripts exist");
        return true;
    }
    println!("Found {} services with missing scripts:\n", services.len());
    for svc in &services {
        println!("  ❌ {}", svc.label());
        println!("     Expected: {}", svc.script);
        if !svc.script.is_empty() {
            let basename = Path::new(&svc.script).file_name().and_then(|n| n.to_str()).unwrap_or("");
            let mut candidates = Vec::new();
            for dir in [c.home.join("dotfiles/scripts"), c.home.join(".local/bin")] {
                if let Ok(rd) = std::fs::read_dir(&dir) {
                    for e in rd.flatten() {
                        if e.file_name().to_string_lossy().starts_with(basename) {
                            candidates.push(e.path().to_string_lossy().into_owned());
                        }
                    }
                }
            }
            if !candidates.is_empty() {
                candidates.sort();
                println!("     Candidates: {}", candidates.join(", "));
            }
            let renu = format!("{}-renu", svc.script);
            if Path::new(&renu).exists() {
                println!("     Fix: symlink {} → {renu}", svc.script);
            }
        }
        println!();
    }
    false
}

pub fn check_all_heartbeats(c: &Ctx) -> bool {
    let dir = heartbeat_dir(c);
    println!("Watcher heartbeats in {}:\n", dir.display());
    let beats = read_heartbeats(&dir, Local::now());
    if beats.is_empty() {
        println!("  No heartbeat files");
        return true;
    }
    for h in &beats {
        println!("{}", heartbeat_line(h));
    }
    !beats.iter().any(|b| b.problem())
}

pub fn suggest_fixes(c: &Ctx) -> bool {
    let services = discover(c);
    let missing: Vec<&Service> = services.iter().filter(|s| !s.script_exists && s.loaded).collect();
    let errored: Vec<&Service> = services.iter().filter(|s| s.errored()).collect();
    let beats = read_heartbeats(&heartbeat_dir(c), Local::now());
    let bad: Vec<&Heartbeat> = beats.iter().filter(|b| b.problem()).collect();

    if missing.is_empty() && errored.is_empty() && bad.is_empty() {
        println!("✅ No fixes needed");
        return true;
    }
    let mut n = 1;
    if !missing.is_empty() {
        println!("Missing scripts:\n");
        for svc in &missing {
            let renu = format!("{}-renu", svc.script);
            if Path::new(&renu).exists() {
                println!("  {n}. {}: create symlink", svc.label());
                println!("     ln -s {renu} {}", svc.script);
            } else {
                println!("  {n}. {}: script not found at {}", svc.label(), svc.script);
                println!("     Either create the script or disable the service:");
                if cfg!(target_os = "macos") {
                    println!("     launchctl unload {}", svc.unit_path.display());
                } else {
                    println!("     systemctl --user disable {}.service", svc.name);
                }
            }
            n += 1;
            println!();
        }
    }
    if !errored.is_empty() {
        println!("Errored services:\n");
        for svc in &errored {
            let code = svc.last_exit.unwrap_or(1);
            println!("  {n}. {}: {}", svc.label(), exit_desc(code, true));
            if code == 78 || code == 127 {
                println!("     Likely cause: script missing or PATH not set in plist");
                println!("     Check ProgramArguments in the plist uses absolute paths");
            }
            if cfg!(target_os = "macos") {
                println!("     Restart: launchctl unload {p} && launchctl load {p}", p = svc.unit_path.display());
            } else if !svc.last_exit_at.is_empty() {
                println!("     Last run ended {}; see: journalctl --user -u {}.service", svc.last_exit_at, svc.name);
            }
            n += 1;
            println!();
        }
    }
    if !bad.is_empty() {
        println!("Watchers not checking in:\n");
        for h in &bad {
            println!("  {n}. {}", heartbeat_line(h).trim_start());
            println!("     Check its unit/agent is running, then its journal; a stale interval watcher needs a restart");
            n += 1;
            println!();
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::Kind;
    use chrono::TimeZone;
    use std::path::PathBuf;

    fn svc(name: &str, kind: Kind, loaded: bool, pid: Option<u32>, last_exit: Option<i32>, script_exists: bool) -> Service {
        Service {
            name: name.into(),
            kind,
            script: format!("/x/{name}"),
            unit_path: PathBuf::from(format!("/u/{name}")),
            loaded,
            pid,
            last_exit,
            script_exists,
            last_exit_at: String::new(),
        }
    }

    fn at(s: &str) -> DateTime<Local> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Local)
    }

    fn beat(name: &str, beat: Beat) -> Heartbeat {
        Heartbeat { name: name.into(), beat }
    }

    #[test]
    fn heartbeat_rules_match_check_nine() {
        let now = Local.with_ymd_and_hms(2026, 9, 22, 13, 0, 0).unwrap();
        let fresh: serde_json::Value = serde_json::from_str(r#"{"last_cycle":"2026-09-22T12:58:00+01:00","interval_secs":120}"#).unwrap();
        // 2 min old, interval 120 s: allowance is max(360, 900) = 15 min.
        assert!(matches!(classify_heartbeat(now, Some(&fresh)), Beat::Alive { interval_secs: 120, .. }));
        let stale: serde_json::Value = serde_json::from_str(r#"{"last_cycle":"2026-09-18T11:02:00+01:00","interval_secs":120}"#).unwrap();
        assert!(matches!(classify_heartbeat(now, Some(&stale)), Beat::Stale { .. }));
        // Event-driven: four days quiet is not dead.
        let quiet: serde_json::Value = serde_json::from_str(r#"{"last_cycle":"2026-09-18T11:02:00+01:00","interval_secs":0}"#).unwrap();
        assert!(matches!(classify_heartbeat(now, Some(&quiet)), Beat::EventDriven { age_min: Some(_) }));
        let err: serde_json::Value = serde_json::from_str(r#"{"last_cycle":"2026-09-22T12:58:00+01:00","interval_secs":120,"last_error":"boom"}"#).unwrap();
        assert_eq!(classify_heartbeat(now, Some(&err)), Beat::Error("boom".into()));
        let no_cycle: serde_json::Value = serde_json::from_str(r#"{"interval_secs":60}"#).unwrap();
        assert_eq!(classify_heartbeat(now, Some(&no_cycle)), Beat::Unreadable("no parseable last_cycle"));
        assert_eq!(classify_heartbeat(now, None), Beat::Unreadable("not valid JSON"));
        let _ = at("2026-09-22T13:00:00+01:00");
    }

    #[test]
    fn read_heartbeats_from_dir_sorted_with_invalid_flagged() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.json"), r#"{"last_cycle":"2026-09-22T12:59:00+01:00","interval_secs":60}"#).unwrap();
        std::fs::write(dir.path().join("a.json"), "nope").unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "").unwrap();
        let now = Local.with_ymd_and_hms(2026, 9, 22, 13, 0, 0).unwrap();
        let v = read_heartbeats(dir.path(), now);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "a");
        assert!(v[0].problem());
        assert_eq!(v[1].name, "b");
        assert!(!v[1].problem());
    }

    #[test]
    fn known_red_failed_timer_fired_unit_breaks_the_verdict() {
        let services = vec![
            svc("drift", Kind::Timer, true, Some(1), Some(0), true),
            svc("some-job", Kind::TimerFired, true, None, Some(1), true),
        ];
        let s = summarise(&services, &[]);
        assert_eq!(s, Summary { healthy: 1, degraded: 0, broken: 1, missing_script: 0, reporting: 0, bad_heartbeats: 0 });
        assert!(s.red());
    }

    #[test]
    fn checker_exit_one_is_yellow_not_red() {
        let services = vec![
            svc("mailcurator-drift", Kind::TimerFired, true, None, Some(1), true),
            svc("capture", Kind::TimerFired, true, None, Some(0), true),
        ];
        let s = summarise(&services, &[]);
        assert_eq!(s.reporting, 1);
        assert_eq!(s.broken, 0);
        assert!(!s.red() && !s.green());
    }

    #[test]
    fn dead_watcher_is_red_and_event_driven_quiet_is_green() {
        let services = vec![svc("daemon", Kind::Service, true, Some(7), Some(0), true)];
        let quiet = beat("forge-md-revs", Beat::EventDriven { age_min: Some(5000) });
        let alive = beat("git-auto-pull-watcher", Beat::Alive { age_min: 1, interval_secs: 120 });
        assert!(summarise(&services, &[quiet.clone(), alive.clone()]).green());
        let dead = beat("git-auto-pull-watcher", Beat::Stale { age_min: 5876, interval_secs: 120 });
        let s = summarise(&services, &[quiet, dead]);
        assert_eq!(s.bad_heartbeats, 1);
        assert!(s.red());
    }

    #[test]
    fn missing_script_and_unloaded_are_degraded_and_counted() {
        let services = vec![
            svc("gone", Kind::Agent, true, Some(3), Some(0), false),
            svc("off", Kind::Agent, false, None, None, true),
        ];
        let s = summarise(&services, &[]);
        assert_eq!(s, Summary { healthy: 0, degraded: 2, broken: 0, missing_script: 1, reporting: 0, bad_heartbeats: 0 });
        assert!(s.red());
        // A unit deliberately not enabled here, whose binary is absent here
        // too (nimbini's zotero-pdf-watcher), is degraded but not red.
        let parked = vec![svc("zotero-pdf-watcher", Kind::Service, false, None, None, false)];
        let s = summarise(&parked, &[]);
        assert_eq!((s.degraded, s.missing_script), (1, 0));
        assert!(!s.red());
    }
}
