//! The five actions. Output shape follows the Nushell original so the eye
//! (and any grep) finds the same groups; exit status is the verdict.

use crate::services::{discover, Ctx, Service};
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Watcher lock files whose age says whether the watcher is alive.
pub const KNOWN_LOCKS: [(&str, &str); 10] = [
    ("git-auto-pull-watcher", "/tmp/git-auto-pull-watcher.lock"),
    ("git-auto-push-watcher", "/tmp/git-auto-push-watcher.lock"),
    ("git-auto-push-watcher-macos", "/tmp/git-auto-push-watcher-macos.lock"),
    ("assistants-git-sync", "/tmp/assistants-git-sync.lock"),
    ("dotter-sync-watcher", "/tmp/dotter-sync-watcher.lock"),
    ("dotter-realtime-watcher", "/tmp/dotter-realtime-watcher.lock"),
    ("dotter-drift-watcher", "/tmp/dotter-drift-watcher.lock"),
    ("citation-watcher", "/tmp/citation-watcher.lock"),
    ("activity-watcher", "/tmp/activity-watcher.lock"),
    ("zellij-zombie-watcher", "/tmp/zellij-zombie-watcher.lock"),
];

const STALE_AFTER: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lock {
    pub name: &'static str,
    pub path: &'static str,
    /// None when the file is absent.
    pub age: Option<Duration>,
}

impl Lock {
    pub fn present(&self) -> bool {
        self.age.is_some()
    }
    pub fn stale(&self) -> bool {
        matches!(self.age, Some(a) if a > STALE_AFTER)
    }
    pub fn age_min(&self) -> u64 {
        self.age.map(|a| (a.as_secs_f64() / 60.0).round() as u64).unwrap_or(0)
    }
}

pub fn lock_status() -> Vec<Lock> {
    KNOWN_LOCKS
        .iter()
        .map(|(name, path)| Lock { name, path, age: lock_age(Path::new(path)) })
        .collect()
}

fn lock_age(path: &Path) -> Option<Duration> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(SystemTime::now().duration_since(modified).unwrap_or_default())
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub healthy: usize,
    pub degraded: usize,
    pub broken: usize,
    pub missing_script: usize,
    pub stale_locks: usize,
}

impl Summary {
    /// 🔴 — the exit-1 condition.
    pub fn red(&self) -> bool {
        self.broken > 0 || self.missing_script > 0
    }
    pub fn green(&self) -> bool {
        !self.red() && self.stale_locks == 0
    }
}

/// Same bookkeeping as the full report, without printing — so the verdict can
/// be tested against known-red and known-green service lists.
pub fn summarise(services: &[Service], locks: &[Lock]) -> Summary {
    let mut s = Summary::default();
    for svc in services {
        if svc.running() || svc.loaded_ok() {
            if svc.script_exists {
                s.healthy += 1;
            } else {
                s.degraded += 1;
                s.missing_script += 1;
            }
        } else if svc.errored() {
            s.broken += 1;
            if !svc.script_exists {
                s.missing_script += 1;
            }
        } else if !svc.loaded {
            s.degraded += 1;
            if !svc.script_exists {
                s.missing_script += 1;
            }
        }
    }
    s.stale_locks = locks.iter().filter(|l| l.stale()).count();
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
            let when = if svc.last_exit_at.is_empty() { String::new() } else { format!(" [{}]", svc.last_exit_at) };
            println!("  ✅ {}{}{}", svc.label(), when, missing_tag(svc));
        }
        println!();
    }

    let errored: Vec<&Service> = services.iter().filter(|s| s.errored()).collect();
    if !errored.is_empty() {
        println!("── Errored ({}) ──", errored.len());
        for svc in &errored {
            let when = if svc.last_exit_at.is_empty() { String::new() } else { format!(" [{}]", svc.last_exit_at) };
            println!("  ❌ {} — {}{}{}", svc.label(), exit_desc(svc.last_exit.unwrap_or(1), false), when, missing_tag(svc));
        }
        println!();
    }

    let not_loaded: Vec<&Service> = services.iter().filter(|s| !s.loaded).collect();
    if !not_loaded.is_empty() {
        println!("── Not loaded ({}) ──", not_loaded.len());
        for svc in &not_loaded {
            println!("  ⚪ {}{}", svc.label(), missing_tag(svc));
        }
        println!();
    }

    println!("── Lock files ──");
    let locks = lock_status();
    let present: Vec<&Lock> = locks.iter().filter(|l| l.present()).collect();
    for lock in &present {
        if lock.stale() {
            println!("  🔴 {} — STALE [{}m]", lock.name, lock.age_min());
        } else {
            println!("  🟢 {} — active [{}m]", lock.name, lock.age_min());
        }
    }
    if present.is_empty() {
        println!("  No lock files present");
    }

    let s = summarise(&services, &locks);
    println!("\n── Summary ──");
    println!("  Healthy:        {}", s.healthy);
    println!("  Degraded:       {}", s.degraded);
    println!("  Broken:         {}", s.broken);
    println!("  Missing scripts: {}", s.missing_script);
    println!("  Stale locks:    {}", s.stale_locks);

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
    let locks = lock_status();
    let running = services.iter().filter(|s| s.running()).count();
    let loaded_ok = services.iter().filter(|s| s.loaded && s.last_exit == Some(0)).count();
    let errored = services.iter().filter(|s| s.loaded && matches!(s.last_exit, Some(c) if c != 0)).count();
    let not_loaded = services.iter().filter(|s| !s.loaded).count();
    let missing = services.iter().filter(|s| !s.script_exists).count();
    let stale = locks.iter().filter(|l| l.stale()).count();

    println!(
        "Services: {} total | {running} running | {loaded_ok} ok | {errored} errored | {not_loaded} unloaded",
        services.len()
    );
    println!("Scripts:  {missing} missing");
    println!("Locks:    {stale} stale");

    if errored == 0 && missing == 0 && stale == 0 {
        println!("🟢 Healthy");
        true
    } else {
        println!("🔴 Issues detected");
        false
    }
}

pub fn check_missing_scripts(c: &Ctx) -> bool {
    let services: Vec<Service> = discover(c).into_iter().filter(|s| !s.script_exists).collect();
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

pub fn check_all_locks(_c: &Ctx) -> bool {
    println!("Lock file status:\n");
    for lock in lock_status() {
        match (lock.present(), lock.stale()) {
            (false, _) => println!("  ⚪ {} — no lock", lock.name),
            (true, true) => println!("  🔴 {} — STALE [{}m]", lock.name, lock.age_min()),
            (true, false) => println!("  🟢 {} — active [{}m]", lock.name, lock.age_min()),
        }
    }
    true
}

pub fn suggest_fixes(c: &Ctx) -> bool {
    let services = discover(c);
    let missing: Vec<&Service> = services.iter().filter(|s| !s.script_exists).collect();
    let errored: Vec<&Service> = services.iter().filter(|s| s.loaded && matches!(s.last_exit, Some(c) if c != 0)).collect();
    let locks = lock_status();
    let stale: Vec<&Lock> = locks.iter().filter(|l| l.stale()).collect();

    if missing.is_empty() && errored.is_empty() {
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
    if !stale.is_empty() {
        println!("Stale lock files:\n");
        for lock in &stale {
            println!("  {n}. rm -f {}", lock.path);
            n += 1;
        }
        println!();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::Kind;
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

    #[test]
    fn known_red_failed_timer_fired_unit_breaks_the_verdict() {
        let services = vec![
            svc("drift", Kind::Timer, true, Some(1), Some(0), true),
            svc("drift", Kind::TimerFired, true, None, Some(1), true),
        ];
        let s = summarise(&services, &[]);
        assert_eq!(s, Summary { healthy: 1, degraded: 0, broken: 1, missing_script: 0, stale_locks: 0 });
        assert!(s.red());
    }

    #[test]
    fn known_green_and_stale_lock_is_yellow_not_red() {
        let services = vec![
            svc("capture", Kind::TimerFired, true, None, Some(0), true),
            svc("daemon", Kind::Service, true, Some(7), Some(0), true),
        ];
        let fresh = Lock { name: "a", path: "/tmp/a", age: Some(Duration::from_secs(30)) };
        let stale = Lock { name: "b", path: "/tmp/b", age: Some(Duration::from_secs(3600)) };
        let absent = Lock { name: "c", path: "/tmp/c", age: None };
        let s = summarise(&services, &[fresh.clone(), absent]);
        assert!(s.green());
        let s = summarise(&services, &[fresh, stale]);
        assert_eq!(s.stale_locks, 1);
        assert!(!s.red() && !s.green());
    }

    #[test]
    fn missing_script_and_unloaded_are_degraded_and_counted() {
        let services = vec![
            svc("gone", Kind::Agent, true, Some(3), Some(0), false),
            svc("off", Kind::Agent, false, None, None, true),
        ];
        let s = summarise(&services, &[]);
        assert_eq!(s, Summary { healthy: 0, degraded: 2, broken: 0, missing_script: 1, stale_locks: 0 });
        assert!(s.red());
    }
}
