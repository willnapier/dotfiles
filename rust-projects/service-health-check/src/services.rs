//! Service discovery: one `Service` per launchd agent (macOS) or systemd
//! --user unit (Linux), with the fields the report groups on.

use crate::exec::Exec;
use std::fs;
use std::path::{Path, PathBuf};

pub struct Ctx {
    pub exec: Box<dyn Exec>,
    pub home: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// launchd agent (macOS)
    Agent,
    /// systemd service with no timer of its own
    Service,
    /// systemd timer unit
    Timer,
    /// systemd service that a same-named timer fires (D2-18)
    TimerFired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub kind: Kind,
    /// First token of ExecStart / ProgramArguments:0, specifiers expanded. Empty when unknown.
    pub script: String,
    /// plist on macOS, unit file on Linux
    pub unit_path: PathBuf,
    pub loaded: bool,
    pub pid: Option<u32>,
    pub last_exit: Option<i32>,
    pub script_exists: bool,
    /// When the last run ended (timer-fired units only; empty otherwise).
    pub last_exit_at: String,
}

/// Tools whose exit 1 means "I found issues", not "I broke". Their unit or
/// agent is named after them (`com.williamnapier.<name>` on macOS). Exit 2 and
/// above is still a broken run — cross-machine-sync-check and
/// dotter-drift-monitor use 2 for "could not check".
pub const CHECKERS: [&str; 4] = ["cross-machine-sync-check", "system-health-check", "dotter-drift-monitor", "mailcurator-drift"];

impl Service {
    pub fn running(&self) -> bool {
        self.pid.is_some()
    }
    /// Name without the launchd reverse-DNS prefix.
    pub fn short_name(&self) -> &str {
        self.name.strip_prefix("com.williamnapier.").or_else(|| self.name.strip_prefix("com.user.")).unwrap_or(&self.name)
    }
    fn is_checker(&self) -> bool {
        CHECKERS.contains(&self.short_name())
    }
    /// A checker whose last run exited 1: it reported issues and is itself fine.
    pub fn reports_issues(&self) -> bool {
        self.loaded && self.pid.is_none() && self.last_exit == Some(1) && self.is_checker()
    }
    /// Loaded, not running now, and the last run failed. The one predicate
    /// every action uses — the script's `quick`/`fix` ignored the PID and so
    /// counted a KeepAlive-relaunched agent (last exit −15) as errored while
    /// `full` listed it as running.
    pub fn errored(&self) -> bool {
        self.loaded && self.pid.is_none() && matches!(self.last_exit, Some(c) if c != 0) && !self.reports_issues()
    }
    pub fn loaded_ok(&self) -> bool {
        self.loaded && self.pid.is_none() && self.last_exit == Some(0)
    }
    pub fn label(&self) -> String {
        match self.kind {
            Kind::Timer => format!("{} (timer)", self.name),
            Kind::TimerFired => format!("{} (timer-fired)", self.name),
            _ => self.name.clone(),
        }
    }
}

pub fn discover(c: &Ctx) -> Vec<Service> {
    if cfg!(target_os = "macos") {
        discover_macos(c, &c.home.join("Library/LaunchAgents"))
    } else {
        discover_linux(c, &c.home.join(".config/systemd/user"))
    }
}

// ── macOS ────────────────────────────────────────────────────────────

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

/// `ProgramArguments:0`, else `Program`, else empty.
fn plist_script(c: &Ctx, plist: &str) -> String {
    let r = c.exec.run("/usr/libexec/PlistBuddy", &["-c", "Print :ProgramArguments:0", plist]);
    if r.ok() {
        return r.out();
    }
    let r = c.exec.run("/usr/libexec/PlistBuddy", &["-c", "Print :Program", plist]);
    if r.ok() { r.out() } else { String::new() }
}

pub fn discover_macos(c: &Ctx, agents_dir: &Path) -> Vec<Service> {
    let loaded = parse_launchctl_list(&c.exec.run("launchctl", &["list"]).stdout);
    our_plists(agents_dir)
        .into_iter()
        .map(|plist| {
            let name = plist.file_name().and_then(|n| n.to_str()).unwrap_or("").trim_end_matches(".plist").to_string();
            let script = plist_script(c, &plist.to_string_lossy());
            let info = loaded.iter().find(|e| e.label == name);
            Service {
                script_exists: !script.is_empty() && Path::new(&script).exists(),
                loaded: info.is_some(),
                pid: info.and_then(|e| e.pid),
                last_exit: info.and_then(|e| e.last_exit),
                last_exit_at: String::new(),
                name,
                kind: Kind::Agent,
                script,
                unit_path: plist,
            }
        })
        .collect()
}

// ── Linux ────────────────────────────────────────────────────────────

/// Expand the systemd specifiers the unit files here use: `%h` home, `%H` hostname.
pub fn expand_specifiers(raw: &str, home: &Path, hostname: &str) -> String {
    raw.replace("%h", &home.to_string_lossy()).replace("%H", hostname)
}

/// First token of the first `ExecStart=` line, or empty.
pub fn exec_start_path(unit_contents: &str) -> String {
    unit_contents
        .lines()
        .find_map(|l| l.strip_prefix("ExecStart="))
        .and_then(|v| v.split_whitespace().next())
        .map(|t| t.trim_start_matches(['-', '@', ':', '+', '!']).to_string())
        .unwrap_or_default()
}

/// `KEY=value` lines from `systemctl show`, looked up by key.
pub fn show_value<'a>(show_output: &'a str, key: &str) -> &'a str {
    show_output
        .lines()
        .find_map(|l| l.strip_prefix(key).and_then(|rest| rest.strip_prefix('=')))
        .unwrap_or("")
        .trim()
}

/// How a timer-fired service's last run ended, from its `systemctl show`.
/// Returns (pid, last_exit, last_exit_at).
///
/// A oneshot is `inactive` between fires; what matters is `Result` and
/// `ExecMainStatus` of the last fire. `Result=success` with no exit timestamp
/// means it has not run this boot — reported as OK, not as an error, because
/// the timer check elsewhere covers "never fires".
pub fn classify_timer_fired(show_output: &str) -> (Option<u32>, Option<i32>, String) {
    let active = show_value(show_output, "ActiveState");
    if active == "active" || active == "activating" || active == "deactivating" {
        let pid = show_value(show_output, "MainPID").parse().ok().filter(|p: &u32| *p > 0).or(Some(1));
        return (pid, None, String::new());
    }
    let at = show_value(show_output, "ExecMainExitTimestamp").to_string();
    if show_value(show_output, "Result") == "success" {
        return (None, Some(0), at);
    }
    // exit-code, signal, timeout, core-dump, resources…: surface the code when
    // there is one, else a generic 1 so it lands in the Errored group.
    let code = show_value(show_output, "ExecMainStatus").parse::<i32>().ok().filter(|c| *c != 0).unwrap_or(1);
    (None, Some(code), at)
}

const SHOW_PROPS: [&str; 5] = ["ActiveState", "Result", "ExecMainStatus", "ExecMainExitTimestamp", "MainPID"];

pub fn discover_linux(c: &Ctx, units_dir: &Path) -> Vec<Service> {
    let Ok(rd) = fs::read_dir(units_dir) else {
        println!("No systemd user directory found");
        return vec![];
    };
    let mut paths: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    let services: Vec<&PathBuf> = paths.iter().filter(|p| p.extension().and_then(|e| e.to_str()) == Some("service")).collect();
    let timers: Vec<&PathBuf> = paths.iter().filter(|p| p.extension().and_then(|e| e.to_str()) == Some("timer")).collect();
    let timer_names: Vec<String> = timers.iter().map(|p| stem(p)).collect();
    let hostname = c.exec.run("hostname", &[]).out();

    let mut out = Vec::new();
    for unit in services.iter().chain(timers.iter()) {
        let name = stem(unit);
        let file_name = unit.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        let is_timer = file_name.ends_with(".timer");
        let script = if is_timer {
            String::new()
        } else {
            expand_specifiers(&exec_start_path(&fs::read_to_string(unit).unwrap_or_default()), &c.home, &hostname)
        };
        let script_exists = script.is_empty() || Path::new(&script).exists();

        let timer_fired = !is_timer && timer_names.contains(&name);
        let kind = if is_timer {
            Kind::Timer
        } else if timer_fired {
            Kind::TimerFired
        } else {
            Kind::Service
        };

        let (loaded, pid, last_exit, last_exit_at) = if timer_fired {
            // D2-18: judge the service by its own last result; "loaded" is
            // whether its timer is enabled, since a timer-fired oneshot is
            // never enabled itself.
            let timer_unit = format!("{name}.timer");
            let enabled = c.exec.run("systemctl", &["--user", "is-enabled", &timer_unit]).out() == "enabled";
            let mut args = vec!["--user", "show", &file_name];
            for p in SHOW_PROPS {
                args.push("-p");
                args.push(p);
            }
            let (pid, last_exit, at) = classify_timer_fired(&c.exec.run("systemctl", &args).stdout);
            (enabled, pid, last_exit, at)
        } else {
            let status = c.exec.run("systemctl", &["--user", "is-active", &file_name]).out();
            let enabled = c.exec.run("systemctl", &["--user", "is-enabled", &file_name]).out() == "enabled";
            let pid = if status == "active" { Some(1) } else { None };
            let last_exit = match status.as_str() {
                "active" => Some(0),
                "inactive" => None,
                _ => Some(1),
            };
            (enabled, pid, last_exit, String::new())
        };

        out.push(Service {
            name,
            kind,
            script,
            unit_path: (*unit).clone(),
            loaded,
            pid,
            last_exit,
            script_exists,
            last_exit_at,
        });
    }
    out
}

fn stem(p: &Path) -> String {
    p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::{CmdResult, Fake};

    fn ctx(fake: Fake, home: &Path) -> Ctx {
        Ctx { exec: Box::new(fake), home: home.to_path_buf() }
    }

    fn plain(name: &str, loaded: bool, pid: Option<u32>, last_exit: Option<i32>) -> Service {
        Service {
            name: name.into(),
            kind: Kind::Agent,
            script: String::new(),
            unit_path: PathBuf::new(),
            loaded,
            pid,
            last_exit,
            script_exists: true,
            last_exit_at: String::new(),
        }
    }

    #[test]
    fn checker_exit_one_reports_issues_but_exit_two_and_others_are_errors() {
        let sync = plain("com.williamnapier.cross-machine-sync-check", true, None, Some(1));
        assert!(sync.reports_issues() && !sync.errored());
        let could_not = plain("cross-machine-sync-check", true, None, Some(2));
        assert!(!could_not.reports_issues() && could_not.errored());
        let other = plain("com.user.thing", true, None, Some(1));
        assert!(!other.reports_issues() && other.errored());
    }

    #[test]
    fn running_agent_with_old_nonzero_exit_is_not_errored() {
        // KeepAlive relaunched it after a SIGTERM; launchctl still shows -15.
        let relaunched = plain("com.williamnapier.forge-md-revs", true, Some(88), Some(-15));
        assert!(relaunched.running() && !relaunched.errored() && !relaunched.reports_issues());
        let unloaded = plain("com.user.off", false, None, Some(1));
        assert!(!unloaded.errored());
    }

    #[test]
    fn launchctl_list_parses_our_labels_only() {
        let s = "PID\tStatus\tLabel\n-\t0\tcom.apple.foo\n123\t0\tcom.williamnapier.mailforge\n-\t78\tcom.user.thing\n";
        let v = parse_launchctl_list(s);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], LaunchdEntry { label: "com.williamnapier.mailforge".into(), pid: Some(123), last_exit: Some(0) });
        assert_eq!(v[1], LaunchdEntry { label: "com.user.thing".into(), pid: None, last_exit: Some(78) });
    }

    #[test]
    fn exec_start_first_token_and_specifiers() {
        let unit = "[Service]\nExecStart=%h/.local/bin/tool --flag %H\n";
        let raw = exec_start_path(unit);
        assert_eq!(raw, "%h/.local/bin/tool");
        assert_eq!(expand_specifiers(&raw, Path::new("/home/w"), "nimbini"), "/home/w/.local/bin/tool");
        assert_eq!(exec_start_path("ExecStart=-/usr/bin/x y"), "/usr/bin/x");
        assert_eq!(exec_start_path("[Unit]\nDescription=none\n"), "");
    }

    #[test]
    fn timer_fired_failed_oneshot_is_errored_with_its_exit_code() {
        // Known-red control: the shape nimbini's mailcurator-drift shows.
        let show = "ActiveState=failed\nResult=exit-code\nExecMainStatus=1\nExecMainExitTimestamp=Sun 2026-09-20 09:00:13 BST\nMainPID=0\n";
        assert_eq!(classify_timer_fired(show), (None, Some(1), "Sun 2026-09-20 09:00:13 BST".into()));
        // Signal death has no meaningful status; still errored.
        let show = "ActiveState=failed\nResult=signal\nExecMainStatus=0\nExecMainExitTimestamp=x\nMainPID=0\n";
        assert_eq!(classify_timer_fired(show).1, Some(1));
    }

    #[test]
    fn timer_fired_successful_oneshot_is_ok_and_running_one_has_pid() {
        // Known-green control: tm3-diary-capture after a clean fire.
        let show = "ActiveState=inactive\nResult=success\nExecMainStatus=0\nExecMainExitTimestamp=Tue 2026-09-22 12:11:08 BST\nMainPID=0\n";
        assert_eq!(classify_timer_fired(show), (None, Some(0), "Tue 2026-09-22 12:11:08 BST".into()));
        let show = "ActiveState=active\nResult=success\nExecMainStatus=0\nExecMainExitTimestamp=\nMainPID=4242\n";
        assert_eq!(classify_timer_fired(show), (Some(4242), None, String::new()));
        // Never run this boot: OK, not an error.
        let show = "ActiveState=inactive\nResult=success\nExecMainStatus=0\nExecMainExitTimestamp=\nMainPID=0\n";
        assert_eq!(classify_timer_fired(show), (None, Some(0), String::new()));
    }

    #[test]
    fn linux_discovery_reports_timer_fired_service_on_its_own_result() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::write(home.join(".local/bin/drift"), "").unwrap();
        let units = dir.path().join("units");
        fs::create_dir_all(&units).unwrap();
        fs::write(units.join("drift.service"), "[Service]\nType=oneshot\nExecStart=%h/.local/bin/drift\n").unwrap();
        fs::write(units.join("drift.timer"), "[Timer]\nOnCalendar=daily\n").unwrap();
        fs::write(units.join("daemon.service"), "[Service]\nExecStart=%h/.local/bin/missing-daemon\n").unwrap();

        let mut fake = Fake::default();
        fake.respond("hostname", &[], CmdResult::success("box\n"));
        fake.respond("systemctl", &["--user", "is-enabled", "drift.timer"], CmdResult::success("enabled\n"));
        fake.respond(
            "systemctl",
            &["--user", "show", "drift.service", "-p", "ActiveState", "-p", "Result", "-p", "ExecMainStatus", "-p", "ExecMainExitTimestamp", "-p", "MainPID"],
            CmdResult::success("ActiveState=failed\nResult=exit-code\nExecMainStatus=1\nExecMainExitTimestamp=Sun 2026-09-20 09:00:13 BST\nMainPID=0\n"),
        );
        fake.respond("systemctl", &["--user", "is-active", "drift.timer"], CmdResult::success("active\n"));
        fake.respond("systemctl", &["--user", "is-enabled", "drift.timer"], CmdResult::success("enabled\n"));
        fake.respond("systemctl", &["--user", "is-active", "daemon.service"], CmdResult::success("active\n"));
        fake.respond("systemctl", &["--user", "is-enabled", "daemon.service"], CmdResult::success("enabled\n"));

        let found = discover_linux(&ctx(fake, &home), &units);
        let by_name = |n: &str, k: Kind| found.iter().find(|s| s.name == n && s.kind == k).cloned().unwrap();

        let fired = by_name("drift", Kind::TimerFired);
        assert!(fired.errored(), "failed oneshot behind a healthy timer must be Errored (D2-18)");
        assert_eq!(fired.last_exit, Some(1));
        assert!(fired.script_exists);
        assert_eq!(fired.script, home.join(".local/bin/drift").to_string_lossy());

        let timer = by_name("drift", Kind::Timer);
        assert!(timer.running());

        let daemon = by_name("daemon", Kind::Service);
        assert!(daemon.running());
        assert!(!daemon.script_exists, "ExecStart target absent → missing script");
        assert_eq!(found.len(), 3);
    }
}
