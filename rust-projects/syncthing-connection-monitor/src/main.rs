//! syncthing-connection-monitor — keep the LOCAL Syncthing daemon's API alive.
//!
//! Rust port 2026-09-02 of `scripts/syncthing-connection-monitor` (Nushell).
//! The load-bearing rule is unchanged: Syncthing owns peer reconnection, and
//! mobile peers are expected to be offline for long stretches, so an offline
//! PEER is logged (`WARN: Offline devices; no restart: …`) and never causes a
//! restart. Only an unreachable local REST API triggers a service restart
//! through the platform service manager (launchd kickstart / systemd --user).
//!
//! Additions over the oracle: PID lock (stale iff the PID is dead), the
//! `/rest/system/status` probe whose `myID` is excluded from the peer lists,
//! `<gui><address>` honoured instead of a hard-coded localhost:8384,
//! `--once`/`--dry-run`/`--tick`, and the heartbeat JSON in
//! `~/.local/state/watchers/` (written at startup and after every cycle).

mod heartbeat;

use anyhow::{bail, Context, Result};
use clap::Parser;
use heartbeat::Heartbeat;
use serde_json::Value;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const NAME: &str = "syncthing-connection-monitor";

#[derive(Parser, Debug)]
#[command(name = NAME, version, about)]
struct Cli {
    /// Seconds between checks
    #[arg(long, default_value_t = 300)]
    tick: u64,
    /// Seconds to let the daemon come up after a restart before the next check
    #[arg(long, default_value_t = 30)]
    settle: u64,
    /// Run one check and exit (exit 1 if the local API was unreachable)
    #[arg(long)]
    once: bool,
    /// Report what would happen; never restart the service
    #[arg(long)]
    dry_run: bool,
    /// Syncthing config.xml to read the API key and GUI address from
    /// (default: the platform's standard locations, first that exists)
    #[arg(long)]
    config: Option<PathBuf>,
    /// Log file (appended)
    #[arg(long, default_value_os_t = default_log())]
    log: PathBuf,
    /// Directory for the heartbeat JSON (<state-dir>/syncthing-connection-monitor.json)
    #[arg(long, default_value_os_t = default_state_dir())]
    state_dir: PathBuf,
    /// PID lock file (not taken with --once)
    #[arg(long, default_value = "/tmp/syncthing-connection-monitor.lock")]
    lock: PathBuf,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}
fn default_log() -> PathBuf {
    home().join(".local/share/syncthing-monitor.log")
}
fn default_state_dir() -> PathBuf {
    home().join(".local/state/watchers")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let logger = Logger { path: cli.log.clone() };
    if let Some(dir) = cli.log.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let mut hb = Heartbeat::new(&cli.state_dir, NAME, env!("CARGO_PKG_VERSION"), cli.tick);

    if !cli.once {
        take_lock(&cli.lock, &logger)?;
    }
    // Startup heartbeat: alive, no cycle yet (last_action null, actions 0).
    if let Err(e) = hb.write() {
        logger.error(&format!("heartbeat write failed: {e:#}"));
    }

    let config_path = match cli.config.clone().or_else(|| find_config(&home())) {
        Some(p) => p,
        None => {
            logger.error("Failed to get Syncthing API key");
            hb.set_error(Some("no Syncthing config.xml found".into()));
            hb.write().ok();
            std::process::exit(1);
        }
    };
    let gui = std::fs::read_to_string(&config_path).ok().and_then(|xml| parse_gui(&xml));
    let gui = match gui {
        Some(g) if !g.apikey.is_empty() => g,
        _ => {
            logger.error("Failed to get Syncthing API key");
            hb.set_error(Some(format!("no <gui><apikey> in {}", config_path.display())));
            hb.write().ok();
            std::process::exit(1);
        }
    };
    let base = api_base(&gui.address);
    let api = Rest { base: base.clone(), key: gui.apikey.clone() };
    let service = PlatformService;

    logger.info("🔍 Starting Syncthing connection monitor");
    logger.info(&format!("📊 API Key: {}...", gui.apikey.chars().take(8).collect::<String>()));
    logger.info(&format!("📝 Logging to: {}", cli.log.display()));
    if !cli.once {
        logger.info(&format!("🔁 {} {} — API {base}, tick {}s{}, heartbeat {}", NAME, env!("CARGO_PKG_VERSION"), cli.tick, if cli.dry_run { ", dry-run" } else { "" }, hb.path().display()));
    }

    loop {
        let outcome = cycle(&api, &service, cli.dry_run, &logger);
        match &outcome {
            Ok(Outcome::Restarted) => {
                hb.record_action();
                hb.set_error(Some("Syncthing API unreachable; local service restart requested".into()));
            }
            Ok(Outcome::DryRunRestart) => hb.set_error(Some("Syncthing API unreachable (dry-run; no restart)".into())),
            Ok(Outcome::RestartFailed(e)) => hb.set_error(Some(format!("Syncthing API unreachable; restart failed: {e}"))),
            Ok(Outcome::Healthy | Outcome::Degraded) => hb.set_error(None),
            Err(e) => {
                logger.error(&format!("cycle failed: {e:#}"));
                hb.set_error(Some(format!("{e:#}")));
            }
        }
        if let Err(e) = hb.write() {
            logger.error(&format!("heartbeat write failed: {e:#}"));
        }
        if cli.once {
            let unreachable = matches!(outcome, Ok(Outcome::Restarted | Outcome::RestartFailed(_) | Outcome::DryRunRestart));
            std::process::exit(if unreachable { 1 } else { 0 });
        }
        if matches!(outcome, Ok(Outcome::Restarted)) {
            // Give the daemon time to start and expose its API.
            std::thread::sleep(Duration::from_secs(cli.settle));
        }
        std::thread::sleep(Duration::from_secs(cli.tick));
    }
}

// ── lock ────────────────────────────────────────────────────────────
/// PID lock; stale iff the recorded PID is dead (a SIGKILL never runs cleanup).
fn take_lock(lock: &Path, logger: &Logger) -> Result<()> {
    if let Ok(s) = std::fs::read_to_string(lock) {
        if let Ok(pid) = s.trim().parse::<u32>() {
            if pid != std::process::id() && pid_alive(pid) {
                logger.error(&format!("already running — pid {pid}"));
                std::process::exit(1);
            }
            logger.warn(&format!("Removing stale lock file — pid {pid} not running"));
        }
    }
    std::fs::write(lock, std::process::id().to_string()).with_context(|| format!("writing {}", lock.display()))
}
fn pid_alive(pid: u32) -> bool {
    Command::new("kill").args(["-0", &pid.to_string()]).output().map(|o| o.status.success()).unwrap_or(false)
}

// ── config.xml ──────────────────────────────────────────────────────
/// Standard config locations, checked in order like the oracle (Mac, then the
/// XDG state dir Syncthing v2 uses on Linux, then the legacy Linux path).
pub fn find_config(home: &Path) -> Option<PathBuf> {
    [
        "Library/Application Support/Syncthing/config.xml",
        ".local/state/syncthing/config.xml",
        ".config/syncthing/config.xml",
    ]
    .iter()
    .map(|p| home.join(p))
    .find(|p| p.exists())
}

#[derive(Debug, PartialEq)]
pub struct Gui {
    pub apikey: String,
    pub address: String,
}

/// `<gui><apikey>` and `<gui><address>` from Syncthing's config.xml. No XML
/// crate: the file is machine-written and the two elements are simple text.
pub fn parse_gui(xml: &str) -> Option<Gui> {
    let start = xml.find("<gui")?;
    let end = xml[start..].find("</gui>").map(|e| start + e).unwrap_or(xml.len());
    let block = &xml[start..end];
    let apikey = element_text(block, "apikey")?;
    let address = element_text(block, "address").unwrap_or_default();
    Some(Gui { apikey, address })
}
fn element_text(block: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let s = block.find(&open)? + open.len();
    let e = block[s..].find(&close)? + s;
    Some(block[s..e].trim().to_string())
}

/// `http://host:port` for the REST API. Wildcard binds (0.0.0.0, ::, empty
/// host) become 127.0.0.1; anything unparseable falls back to the default.
pub fn api_base(address: &str) -> String {
    const DEFAULT: &str = "http://127.0.0.1:8384";
    let addr = address.trim();
    if addr.is_empty() || addr.starts_with('/') || addr.starts_with("unix://") {
        return DEFAULT.into();
    }
    let Some((host, port)) = addr.rsplit_once(':') else { return DEFAULT.into() };
    if port.is_empty() || port.parse::<u16>().is_err() {
        return DEFAULT.into();
    }
    let host = match host {
        "" | "0.0.0.0" | "::" | "[::]" => "127.0.0.1",
        h => h,
    };
    format!("http://{host}:{port}")
}

// ── API + service, behind traits so the decision path is testable ───
pub trait Api {
    /// GET `<base><path>` and parse the JSON body.
    fn get_json(&self, path: &str) -> Result<Value>;
}
pub trait Service {
    /// Restart the local daemon; `Err` carries the service manager's stderr.
    fn restart(&self) -> Result<()>;
}

struct Rest {
    base: String,
    key: String,
}
impl Api for Rest {
    fn get_json(&self, path: &str) -> Result<Value> {
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build();
        let body = agent
            .get(&format!("{}{}", self.base, path))
            .set("X-API-Key", &self.key)
            .call()
            .with_context(|| format!("GET {path}"))?
            .into_string()
            .with_context(|| format!("reading {path}"))?;
        serde_json::from_str(&body).with_context(|| format!("parsing {path}"))
    }
}

/// Restart only through the platform service manager: the REST restart
/// endpoint cannot help when the API is down and, with Homebrew's
/// --no-restart flag, intentionally exits with code 3.
struct PlatformService;
impl Service for PlatformService {
    fn restart(&self) -> Result<()> {
        let out = if cfg!(target_os = "macos") {
            let uid = Command::new("id").arg("-u").output().context("id -u")?;
            let uid = String::from_utf8_lossy(&uid.stdout).trim().to_string();
            Command::new("launchctl").args(["kickstart", "-k", &format!("gui/{uid}/homebrew.mxcl.syncthing")]).output().context("launchctl kickstart")?
        } else {
            Command::new("systemctl").args(["--user", "restart", "syncthing.service"]).output().context("systemctl --user restart")?
        };
        if out.status.success() {
            Ok(())
        } else {
            bail!("{}", String::from_utf8_lossy(&out.stderr).trim())
        }
    }
}

// ── pure core ───────────────────────────────────────────────────────
#[derive(Debug, PartialEq)]
pub enum Verdict {
    /// The local API answered; every peer is connected.
    AllConnected(Vec<String>),
    /// The local API answered; some peers are offline (informational only).
    PeersOffline { connected: Vec<String>, offline: Vec<String> },
    /// The local API did not answer usefully.
    Unreachable(String),
}

/// Given `/rest/system/connections` JSON (and our own ID from
/// `/rest/system/status`, which is excluded from the peer lists), sort peers
/// into connected/offline. Device IDs are the map keys, as in the oracle.
pub fn verdict_from_connections(connections: &Value, my_id: Option<&str>) -> Verdict {
    let Some(map) = connections.get("connections").and_then(Value::as_object) else {
        return Verdict::Unreachable("no `connections` object in response".into());
    };
    let mut connected = Vec::new();
    let mut offline = Vec::new();
    for (id, info) in map {
        if Some(id.as_str()) == my_id {
            continue;
        }
        if info.get("connected").and_then(Value::as_bool) == Some(true) {
            connected.push(id.clone());
        } else {
            offline.push(id.clone());
        }
    }
    connected.sort();
    offline.sort();
    if offline.is_empty() {
        Verdict::AllConnected(connected)
    } else {
        Verdict::PeersOffline { connected, offline }
    }
}

/// Probe the local API: status first (reachability + our own ID), then connections.
pub fn probe(api: &dyn Api) -> Verdict {
    let status = match api.get_json("/rest/system/status") {
        Ok(v) => v,
        Err(e) => return Verdict::Unreachable(format!("{e:#}")),
    };
    let my_id = status.get("myID").and_then(Value::as_str).map(str::to_string);
    match api.get_json("/rest/system/connections") {
        Ok(v) => verdict_from_connections(&v, my_id.as_deref()),
        Err(e) => Verdict::Unreachable(format!("{e:#}")),
    }
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Healthy,
    /// peers offline; logged, no restart
    Degraded,
    DryRunRestart,
    Restarted,
    RestartFailed(String),
}

pub fn cycle(api: &dyn Api, service: &dyn Service, dry_run: bool, logger: &Logger) -> Result<Outcome> {
    match probe(api) {
        Verdict::AllConnected(ids) => {
            logger.info(&format!("✅ All devices connected: {}", ids.join(", ")));
            Ok(Outcome::Healthy)
        }
        Verdict::PeersOffline { offline, .. } => {
            // Peer availability is informational. Syncthing owns reconnection;
            // never bounce the healthy local daemon for an offline peer.
            logger.warn(&format!("Offline devices; no restart: {}", offline.join(", ")));
            Ok(Outcome::Degraded)
        }
        Verdict::Unreachable(why) => {
            logger.error(&format!("Failed to check connections - Syncthing may be down ({why})"));
            if dry_run {
                logger.info("(dry-run) would restart the local Syncthing service");
                return Ok(Outcome::DryRunRestart);
            }
            logger.error("Syncthing API is unavailable; restarting the local service");
            match service.restart() {
                Ok(()) => {
                    logger.info("✅ Local Syncthing service restart requested");
                    Ok(Outcome::Restarted)
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    logger.error(&format!("❌ Local Syncthing service restart failed: {msg}"));
                    Ok(Outcome::RestartFailed(msg))
                }
            }
        }
    }
}

// ── logger ──────────────────────────────────────────────────────────
pub struct Logger {
    path: PathBuf,
}
impl Logger {
    fn info(&self, msg: &str) {
        self.log("INFO", msg)
    }
    fn warn(&self, msg: &str) {
        self.log("WARN", msg)
    }
    fn error(&self, msg: &str) {
        self.log("ERROR", msg)
    }
    fn log(&self, level: &str, msg: &str) {
        let line = format!("[{}] {level}: {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
        println!("{line}");
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{line}");
        }
    }
}

// ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    const CONFIG_XML: &str = r#"<configuration version="37">
    <folder id="x" label="X" path="/tmp/x"></folder>
    <device id="AAAA-BBBB" name="mac"></device>
    <gui enabled="true" tls="false" debugging="false">
        <address>0.0.0.0:8384</address>
        <apikey>abcdefghijklmnop</apikey>
        <theme>default</theme>
    </gui>
</configuration>"#;

    #[test]
    fn parses_apikey_and_address_from_gui_block() {
        assert_eq!(parse_gui(CONFIG_XML), Some(Gui { apikey: "abcdefghijklmnop".into(), address: "0.0.0.0:8384".into() }));
        assert_eq!(parse_gui("<configuration></configuration>"), None);
        assert_eq!(parse_gui("<gui><apikey>k</apikey></gui>"), Some(Gui { apikey: "k".into(), address: String::new() }));
    }

    #[test]
    fn api_base_rewrites_wildcard_binds_and_falls_back() {
        assert_eq!(api_base("0.0.0.0:8384"), "http://127.0.0.1:8384");
        assert_eq!(api_base("127.0.0.1:9000"), "http://127.0.0.1:9000");
        assert_eq!(api_base("[::]:8384"), "http://127.0.0.1:8384");
        assert_eq!(api_base(":8384"), "http://127.0.0.1:8384");
        assert_eq!(api_base(""), "http://127.0.0.1:8384");
        assert_eq!(api_base("/run/syncthing.sock"), "http://127.0.0.1:8384");
        assert_eq!(api_base("garbage"), "http://127.0.0.1:8384");
    }

    #[test]
    fn find_config_prefers_platform_order() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(find_config(d.path()), None);
        let legacy = d.path().join(".config/syncthing/config.xml");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, "").unwrap();
        assert_eq!(find_config(d.path()), Some(legacy.clone()));
        let xdg = d.path().join(".local/state/syncthing/config.xml");
        std::fs::create_dir_all(xdg.parent().unwrap()).unwrap();
        std::fs::write(&xdg, "").unwrap();
        assert_eq!(find_config(d.path()), Some(xdg));
    }

    #[test]
    fn verdict_sorts_peers_and_excludes_self() {
        let v: Value = serde_json::json!({
            "connections": {
                "ZZZ-PHONE": {"connected": false, "paused": false},
                "AAA-NIMBINI": {"connected": true},
                "MMM-ME": {"connected": false}
            },
            "total": {}
        });
        assert_eq!(
            verdict_from_connections(&v, Some("MMM-ME")),
            Verdict::PeersOffline { connected: vec!["AAA-NIMBINI".into()], offline: vec!["ZZZ-PHONE".into()] }
        );
        let all: Value = serde_json::json!({"connections": {"B": {"connected": true}, "A": {"connected": true}}});
        assert_eq!(verdict_from_connections(&all, None), Verdict::AllConnected(vec!["A".into(), "B".into()]));
        // a missing `connected` field counts as offline, as the oracle's `== true` did
        let odd: Value = serde_json::json!({"connections": {"A": {}}});
        assert!(matches!(verdict_from_connections(&odd, None), Verdict::PeersOffline { .. }));
        assert!(matches!(verdict_from_connections(&serde_json::json!({}), None), Verdict::Unreachable(_)));
    }

    // ── fakes ──
    struct FakeApi {
        status: Result<Value, String>,
        connections: Result<Value, String>,
    }
    impl Api for FakeApi {
        fn get_json(&self, path: &str) -> Result<Value> {
            let r = match path {
                "/rest/system/status" => &self.status,
                "/rest/system/connections" => &self.connections,
                p => panic!("unexpected path {p}"),
            };
            r.clone().map_err(|e| anyhow::anyhow!(e))
        }
    }
    struct FakeService {
        calls: Cell<u32>,
        fail: bool,
        log: RefCell<Vec<&'static str>>,
    }
    impl FakeService {
        fn new(fail: bool) -> Self {
            FakeService { calls: Cell::new(0), fail, log: RefCell::new(vec![]) }
        }
    }
    impl Service for FakeService {
        fn restart(&self) -> Result<()> {
            self.calls.set(self.calls.get() + 1);
            self.log.borrow_mut().push("restart");
            if self.fail {
                bail!("Could not find service \"homebrew.mxcl.syncthing\" in domain")
            }
            Ok(())
        }
    }
    fn conns(pairs: &[(&str, bool)]) -> Value {
        let mut m = serde_json::Map::new();
        for (id, c) in pairs {
            m.insert(id.to_string(), serde_json::json!({"connected": c}));
        }
        serde_json::json!({"connections": m})
    }
    fn logger(d: &tempfile::TempDir) -> (Logger, PathBuf) {
        let p = d.path().join("monitor.log");
        (Logger { path: p.clone() }, p)
    }

    #[test]
    fn healthy_cycle_logs_all_connected_and_never_restarts() {
        let d = tempfile::tempdir().unwrap();
        let (lg, path) = logger(&d);
        let api = FakeApi { status: Ok(serde_json::json!({"myID": "ME"})), connections: Ok(conns(&[("ME", false), ("PEER-1", true), ("PEER-2", true)])) };
        let svc = FakeService::new(false);
        assert_eq!(cycle(&api, &svc, false, &lg).unwrap(), Outcome::Healthy);
        assert_eq!(svc.calls.get(), 0);
        let log = std::fs::read_to_string(path).unwrap();
        assert!(log.contains("INFO: ✅ All devices connected: PEER-1, PEER-2"), "{log}");
    }

    #[test]
    fn offline_peers_are_warned_about_but_never_restart() {
        let d = tempfile::tempdir().unwrap();
        let (lg, path) = logger(&d);
        let api = FakeApi { status: Ok(serde_json::json!({"myID": "ME"})), connections: Ok(conns(&[("PHONE", false), ("NIMBINI", true)])) };
        let svc = FakeService::new(false);
        assert_eq!(cycle(&api, &svc, false, &lg).unwrap(), Outcome::Degraded);
        assert_eq!(svc.calls.get(), 0, "must never restart for an offline peer");
        let log = std::fs::read_to_string(path).unwrap();
        assert!(log.contains("WARN: Offline devices; no restart: PHONE"), "{log}");
        assert!(!log.contains("restart requested"));
    }

    #[test]
    fn unreachable_api_restarts_local_service() {
        let d = tempfile::tempdir().unwrap();
        let (lg, path) = logger(&d);
        let api = FakeApi { status: Err("GET /rest/system/status: Connection refused".into()), connections: Ok(conns(&[])) };
        let svc = FakeService::new(false);
        assert_eq!(cycle(&api, &svc, false, &lg).unwrap(), Outcome::Restarted);
        assert_eq!(svc.calls.get(), 1);
        let log = std::fs::read_to_string(path).unwrap();
        assert!(log.contains("ERROR: Failed to check connections - Syncthing may be down"), "{log}");
        assert!(log.contains("ERROR: Syncthing API is unavailable; restarting the local service"), "{log}");
        assert!(log.contains("INFO: ✅ Local Syncthing service restart requested"), "{log}");
    }

    #[test]
    fn connections_endpoint_failing_after_status_also_counts_as_unreachable() {
        let d = tempfile::tempdir().unwrap();
        let (lg, _) = logger(&d);
        let api = FakeApi { status: Ok(serde_json::json!({"myID": "ME"})), connections: Err("GET /rest/system/connections: timeout".into()) };
        let svc = FakeService::new(false);
        assert_eq!(cycle(&api, &svc, false, &lg).unwrap(), Outcome::Restarted);
        assert_eq!(svc.calls.get(), 1);
    }

    #[test]
    fn restart_failure_is_logged_with_stderr() {
        let d = tempfile::tempdir().unwrap();
        let (lg, path) = logger(&d);
        let api = FakeApi { status: Err("refused".into()), connections: Err("refused".into()) };
        let svc = FakeService::new(true);
        match cycle(&api, &svc, false, &lg).unwrap() {
            Outcome::RestartFailed(m) => assert!(m.contains("homebrew.mxcl.syncthing"), "{m}"),
            o => panic!("expected RestartFailed, got {o:?}"),
        }
        let log = std::fs::read_to_string(path).unwrap();
        assert!(log.contains("ERROR: ❌ Local Syncthing service restart failed: Could not find service"), "{log}");
    }

    #[test]
    fn dry_run_never_touches_the_service() {
        let d = tempfile::tempdir().unwrap();
        let (lg, path) = logger(&d);
        let api = FakeApi { status: Err("refused".into()), connections: Err("refused".into()) };
        let svc = FakeService::new(false);
        assert_eq!(cycle(&api, &svc, true, &lg).unwrap(), Outcome::DryRunRestart);
        assert_eq!(svc.calls.get(), 0);
        assert!(std::fs::read_to_string(path).unwrap().contains("(dry-run) would restart"));
    }

    /// Real HTTP against a throwaway mock server on 127.0.0.1:0 — never the live
    /// Syncthing. Checks the header, the path routing and JSON parsing of `Rest`.
    #[test]
    fn rest_client_sends_api_key_and_parses_json() {
        use std::io::{BufRead, BufReader, Read as _};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..2 {
                let (mut s, _) = listener.accept().unwrap();
                let mut r = BufReader::new(&mut s);
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let mut key = String::new();
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if let Some(v) = h.strip_prefix("X-API-Key: ") {
                        key = v.trim().to_string();
                    }
                    if h.trim().is_empty() {
                        break;
                    }
                }
                let body = if line.starts_with("GET /rest/system/status ") {
                    r#"{"myID":"ME"}"#
                } else {
                    r#"{"connections":{"ME":{"connected":false},"P":{"connected":true}}}"#
                };
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                std::io::Write::write_all(&mut s, resp.as_bytes()).unwrap();
                let _ = std::io::Write::flush(&mut s);
                let _ = s.read(&mut [0u8; 1]);
                seen.push((line.trim().to_string(), key));
            }
            seen
        });
        let api = Rest { base: format!("http://127.0.0.1:{port}"), key: "secret-key".into() };
        assert_eq!(probe(&api), Verdict::AllConnected(vec!["P".into()]));
        let seen = server.join().unwrap();
        assert!(seen.iter().all(|(_, k)| k == "secret-key"), "{seen:?}");
        assert!(seen[0].0.starts_with("GET /rest/system/status "));
        assert!(seen[1].0.starts_with("GET /rest/system/connections "));
    }

    #[test]
    fn rest_client_reports_connection_refused_as_unreachable() {
        let l = TcpListenerDrop::new();
        let api = Rest { base: format!("http://127.0.0.1:{}", l.port), key: "k".into() };
        assert!(matches!(probe(&api), Verdict::Unreachable(_)));
    }
    /// Reserve a port then release it so nothing listens there.
    struct TcpListenerDrop {
        port: u16,
    }
    impl TcpListenerDrop {
        fn new() -> Self {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = l.local_addr().unwrap().port();
            drop(l);
            TcpListenerDrop { port }
        }
    }

    #[test]
    fn stale_lock_is_replaced_and_live_lock_is_ours() {
        let d = tempfile::tempdir().unwrap();
        let (lg, path) = logger(&d);
        let lock = d.path().join("lock");
        std::fs::write(&lock, "999999999").unwrap();
        take_lock(&lock, &lg).unwrap();
        assert_eq!(std::fs::read_to_string(&lock).unwrap(), std::process::id().to_string());
        assert!(std::fs::read_to_string(path).unwrap().contains("Removing stale lock file — pid 999999999 not running"));
        // re-taking our own lock is fine
        take_lock(&lock, &lg).unwrap();
    }
}
