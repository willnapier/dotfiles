mod exec;
use anyhow::{bail, Context, Result};
use exec::{Exec, Real};
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, SystemTime},
};
struct Paths {
    home: PathBuf,
    config: PathBuf,
    grace: PathBuf,
}
#[derive(Debug)]
struct Config {
    pmi: String,
    width: u32,
    height: u32,
}
fn parse_config(text: &str) -> Config {
    let mut c = Config {
        pmi: String::new(),
        width: 648,
        height: 480,
    };
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v: String = v.chars().filter(|c| !c.is_whitespace()).collect();
        match k.trim() {
            "pmi" => c.pmi = v,
            "width" => c.width = v.parse().ok().filter(|v| *v > 0).unwrap_or(648),
            "height" => c.height = v.parse().ok().filter(|v| *v > 0).unwrap_or(480),
            _ => {}
        }
    }
    c
}
fn config(p: &Paths) -> Result<Config> {
    if !p.config.exists() {
        let dir = p.config.parent().context("config directory")?;
        fs::create_dir_all(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        let zoom = fs::read_to_string(p.home.join(".config/zoomus.conf")).unwrap_or_default();
        let guess = zoom
            .lines()
            .find_map(|l| l.strip_prefix("currentMeetingId="))
            .unwrap_or("");
        let guess: String = guess.chars().filter(|c| c.is_ascii_digit()).collect();
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&p.config)?;
        writeln!(
            f,
            "# zoom-niri local config — not in git\npmi={guess}\nwidth=648\nheight=480"
        )?;
    }
    Ok(parse_config(&fs::read_to_string(&p.config)?))
}
fn stamp() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_secs())
}
fn launching(p: &Paths) -> Result<bool> {
    match fs::read_to_string(&p.grace) {
        Ok(s) => {
            let t = s
                .trim()
                .parse::<u64>()
                .context("invalid Zoom launch marker")?;
            let now = stamp()?;
            Ok(t <= now && now - t < 20)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn mark_launching(p: &Paths) -> Result<()> {
    fs::create_dir_all(p.grace.parent().context("runtime dir")?)?;
    fs::write(&p.grace, stamp()?.to_string())?;
    Ok(())
}
fn zoom_url(u: &str) -> bool {
    let Ok(url) = url::Url::parse(u) else {
        return false;
    };
    match url.scheme() {
        "zoommtg" | "zoomus" => true,
        "https" | "http" => url
            .host_str()
            .is_some_and(|h| h == "zoom.us" || h.ends_with(".zoom.us")),
        _ => false,
    }
}
#[derive(Debug, Deserialize)]
struct Window {
    id: u64,
    app_id: Option<String>,
    title: Option<String>,
}
fn windows(ex: &dyn Exec) -> Result<Vec<Window>> {
    let r = ex.run("niri", &["msg", "--json", "windows"]);
    if !r.ok() {
        bail!("Niri window query failed (exit {})", r.exit_code)
    };
    serde_json::from_str(&r.stdout).context("invalid Niri window response")
}
fn ids(w: &[Window], prefix: &str) -> Vec<u64> {
    w.iter()
        .filter(|w| {
            w.app_id.as_deref() == Some("zoom")
                && w.title.as_deref().is_some_and(|s| s.starts_with(prefix))
        })
        .map(|w| w.id)
        .collect()
}
fn action(ex: &dyn Exec, name: &str, id: u64, value: Option<u32>) -> Result<()> {
    let id = id.to_string();
    let val = value.map(|v| v.to_string());
    let mut a = vec!["msg", "action", name, "--id", &id];
    if let Some(v) = val.as_deref() {
        a.push(v)
    };
    if !ex.run("niri", &a).ok() {
        bail!("Niri {name} failed")
    };
    Ok(())
}
fn layout(ex: &dyn Exec, id: u64, c: &Config) -> Result<()> {
    action(ex, "move-window-to-tiling", id, None)?;
    action(ex, "set-window-width", id, Some(c.width))?;
    action(ex, "set-window-height", id, Some(c.height))?;
    action(ex, "focus-window", id, None)
}
fn zoom_pids(ex: &dyn Exec) -> Result<Vec<u32>> {
    let uid = ex.run("id", &["-u"]);
    if !uid.ok() {
        bail!("cannot determine Zoom process owner")
    };
    let r = ex.run("ps", &["-u", &uid.out(), "-o", "pid=,args="]);
    if !r.ok() || r.out().is_empty() {
        bail!("Zoom process discovery failed")
    };
    let mut pids = Vec::new();
    for line in r.stdout.lines().filter(|s| !s.trim().is_empty()) {
        let mut fields = line.split_whitespace();
        let pid = fields.next().context("missing PID")?.parse::<u32>()?;
        let exe = fields.next().context("missing command")?;
        if pid > 1 && (exe == "/usr/bin/zoom" || exe.starts_with("/opt/zoom/")) {
            pids.push(pid)
        }
    }
    Ok(pids)
}
fn kill_zoom(p: &Paths, ex: &dyn Exec) -> Result<()> {
    let initial = zoom_pids(ex)?;
    for pid in &initial {
        if !ex.run("kill", &["-TERM", &pid.to_string()]).ok() && zoom_pids(ex)?.contains(pid) {
            bail!("could not terminate Zoom")
        }
    }
    if !initial.is_empty() {
        for _ in 0..25 {
            if zoom_pids(ex)?.is_empty() {
                break;
            }
            if !ex.run("sleep", &["0.1"]).ok() {
                bail!("Zoom termination wait failed")
            }
        }
    }
    // Only escalate original targets still matching Zoom; do not kill a newly
    // launched session that appeared during termination.
    for pid in zoom_pids(ex)? {
        if !initial.contains(&pid) {
            bail!("another Zoom process started during cleanup")
        };
        if !ex.run("kill", &["-KILL", &pid.to_string()]).ok() && zoom_pids(ex)?.contains(&pid) {
            bail!("could not kill Zoom survivor")
        }
    }
    if !initial.is_empty() {
        let _ = ex.run("sleep", &["0.2"]);
        if !zoom_pids(ex)?.is_empty() {
            bail!("Zoom survived cleanup")
        }
    }
    let dir = p.home.join(".config/zoom");
    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let e = entry?;
            if e.file_name()
                .to_string_lossy()
                .starts_with("qtsingleapp-zoom-")
            {
                fs::remove_file(e.path()).context("cannot remove Zoom single-instance socket")?
            }
        }
    }
    Ok(())
}
#[derive(Default)]
struct Watch {
    had_meeting: bool,
    laid_out: HashSet<u64>,
}
impl Watch {
    fn update(&mut self, p: &Paths, c: &Config, ex: &dyn Exec) -> Result<()> {
        // Query errors must never become an empty meeting list (which kills Zoom).
        let w = windows(ex)?;
        let meetings = ids(&w, "Meeting");
        if !meetings.is_empty() {
            self.had_meeting = true;
            self.laid_out.retain(|id| meetings.contains(id));
            for id in meetings {
                if !self.laid_out.contains(&id) {
                    layout(ex, id, c)?;
                    for id in ids(&w, "Zoom Workplace") {
                        action(ex, "close-window", id, None)?
                    }
                    self.laid_out.insert(id);
                }
            }
        } else if self.had_meeting && !launching(p)? {
            eprintln!("zoom-niri watch: meeting ended, killing Zoom");
            kill_zoom(p, ex)?;
            self.had_meeting = false;
            self.laid_out.clear();
        }
        Ok(())
    }
}
fn heartbeat(p: &Paths, status: &str, error: Option<&str>) -> Result<()> {
    let dir = p.home.join(".local/state/watchers");
    fs::create_dir_all(&dir)?;
    let file = dir.join("zoom-niri.json");
    let tmp = dir.join(format!(".zoom-niri-{}.tmp", std::process::id()));
    fs::write(
        &tmp,
        serde_json::to_vec(
            &serde_json::json!({"watcher":"zoom-niri","version":env!("CARGO_PKG_VERSION"),"pid":std::process::id(),"interval_secs":30,"last_cycle":chrono::Utc::now().to_rfc3339(),"status":status,"last_error":error}),
        )?,
    )?;
    fs::rename(tmp, file)?;
    Ok(())
}
struct Stream {
    child: Child,
    rx: mpsc::Receiver<Result<()>>,
}
impl Stream {
    fn start() -> Result<Self> {
        let mut child = Command::new("niri")
            .args(["msg", "--json", "event-stream"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let out = child.stdout.take().context("event pipe")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let r = line.map(|_| ()).map_err(anyhow::Error::from);
                let failed = r.is_err();
                if tx.send(r).is_err() || failed {
                    break;
                }
            }
        });
        Ok(Self { child, rx })
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn watch(p: &Paths, ex: &dyn Exec) -> Result<()> {
    let mut wait = 0;
    while !ex.run("niri", &["msg", "version"]).ok() {
        heartbeat(p, "waiting", Some("waiting for Niri"))?;
        if wait % 30 == 0 {
            eprintln!("zoom-niri watch: waiting for niri")
        };
        std::thread::sleep(Duration::from_secs(1));
        wait += 1;
    }
    let stream = Stream::start()?;
    let mut w = Watch::default();
    w.update(p, &config(p)?, ex)?;
    heartbeat(p, "connected", None)?;
    eprintln!("zoom-niri watch: connected; outcome=success");
    loop {
        match stream.rx.recv_timeout(Duration::from_secs(30)) {
            Ok(r) => r?,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => bail!("Niri event stream ended"),
        };
        w.update(p, &config(p)?, ex)?;
        heartbeat(p, "connected", None)?;
    }
}
fn layout_meetings(p: &Paths, ex: &dyn Exec) -> Result<bool> {
    let c = config(p)?;
    let w = windows(ex)?;
    let meetings = ids(&w, "Meeting");
    for id in &meetings {
        layout(ex, *id, &c)?
    }
    if !meetings.is_empty() {
        for id in ids(&w, "Zoom Workplace") {
            action(ex, "close-window", id, None)?
        }
    }
    Ok(!meetings.is_empty())
}
fn launch(p: &Paths, url: Option<&str>, workplace: bool, ex: &dyn Exec) -> Result<()> {
    let c = config(p)?;
    let url = url.filter(|s| !s.is_empty() && *s != "%u" && *s != "%U");
    let url = if workplace {
        None
    } else if let Some(u) = url {
        if !zoom_url(u) {
            bail!("Refusing to pass a non-Zoom URL to Zoom")
        };
        Some(u.to_string())
    } else {
        if c.pmi.is_empty() || !c.pmi.chars().all(|c| c.is_ascii_digit()) {
            bail!("Set pmi= in the local zoom-niri config")
        };
        Some(format!(
            "zoommtg://zoom.us/join?action=join&confno={}",
            c.pmi
        ))
    };
    // Validate the request before touching an existing session.
    if !Path::new("/usr/bin/zoom").is_file() {
        bail!("Zoom is not installed (/usr/bin/zoom)")
    }
    mark_launching(p)?;
    kill_zoom(p, ex)?;
    std::thread::sleep(Duration::from_millis(200));
    let args: Vec<_> = url.as_deref().into_iter().collect();
    if !ex.launch(&args) {
        bail!("Zoom launch failed")
    }
    if !workplace {
        for _ in 0..50 {
            if layout_meetings(p, ex)? {
                return Ok(());
            };
            std::thread::sleep(Duration::from_millis(500));
        }
        eprintln!("zoom-niri: Meeting window not seen yet; watcher will layout it");
    }
    Ok(())
}
const HELP:&str="zoom-niri — fresh per-client Zoom session on Niri\nUsage: zoom-niri [URL] | launch [URL] | workplace | layout | kill | watch\nNo URL joins pmi= from ~/.config/zoom-niri/config. Defaults: width=648 height=480.\nWatch records ~/.local/state/watchers/zoom-niri.json every 30 seconds.";
fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("launch");
    if ["--help", "-h", "help"].contains(&command) {
        println!("{HELP}");
        return 0.into();
    };
    if command == "--version" {
        println!("zoom-niri {}", env!("CARGO_PKG_VERSION"));
        return 0.into();
    }
    let known = ["launch", "workplace", "layout", "kill", "watch"].contains(&command);
    let direct = command.starts_with("http:")
        || command.starts_with("https:")
        || command.starts_with("zoommtg:")
        || command.starts_with("zoomus:")
        || command == "%u"
        || command == "%U";
    if (!known && !direct)
        || (command == "launch" && args.len() > 2)
        || (command != "launch" && args.len() > 1)
    {
        eprintln!("{HELP}");
        return 2.into();
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        eprintln!("HOME not set");
        return 1.into();
    };
    let p = Paths {
        config: std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("zoom-niri/config"),
        grace: std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("zoom-niri-launching"),
        home,
    };
    let result = match command {
        "watch" => watch(&p, &Real),
        "kill" => kill_zoom(&p, &Real),
        "layout" => layout_meetings(&p, &Real).and_then(|found| {
            if found {
                Ok(())
            } else {
                bail!("no Meeting window")
            }
        }),
        "workplace" => launch(&p, None, true, &Real),
        "launch" => launch(&p, args.get(1).map(String::as_str), false, &Real),
        _ => launch(&p, Some(command), false, &Real),
    };
    match result {
        Ok(()) => {
            eprintln!("zoom-niri: outcome=success {command}");
            0.into()
        }
        Err(e) => {
            eprintln!("zoom-niri: {e:#}");
            if command == "watch" {
                let _ = heartbeat(&p, "error", Some(&format!("{e:#}")));
            }
            let _ = Real.run(
                p.home.join(".local/bin/notify-user").to_str().unwrap(),
                &["--tool", "zoom-niri", "Zoom", &format!("{e:#}")],
            );
            1.into()
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use exec::{CmdResult, Fake};
    fn paths(t: &tempfile::TempDir) -> Paths {
        Paths {
            home: t.path().into(),
            config: t.path().join("config"),
            grace: t.path().join("launching"),
        }
    }
    #[test]
    fn urls_and_configuration() {
        assert!(zoom_url("https://example.zoom.us/j/123"));
        assert!(zoom_url("zoommtg://zoom.us/join?confno=123"));
        for u in [
            "https://evilzoom.us/j/1",
            "https://zoom.us.evil/j/1",
            "file:///x",
            "https://zoom.us@evil/a",
        ] {
            assert!(!zoom_url(u));
        }
        let c = parse_config("pmi=12 34\nwidth=bad\nheight=520 # good");
        assert_eq!(c.pmi, "1234");
        assert_eq!((c.width, c.height), (648, 520));
    }
    fn no_zoom(f: &mut Fake) {
        f.respond("id", &["-u"], CmdResult::success("1000"));
        f.respond(
            "ps",
            &["-u", "1000", "-o", "pid=,args="],
            CmdResult::success("1 /bin/init\n100 /bin/bash -c /usr/bin/zoom"),
        );
    }
    #[test]
    fn failed_or_malformed_window_query_never_kills() {
        let t = tempfile::tempdir().unwrap();
        let p = paths(&t);
        let mut w = Watch {
            had_meeting: true,
            ..Default::default()
        };
        let mut f = Fake::default();
        for result in [
            CmdResult::failure(1, "IPC down"),
            CmdResult::success("garbage"),
            CmdResult::success("{}"),
        ] {
            f.respond("niri", &["msg", "--json", "windows"], result);
            assert!(w.update(&p, &parse_config(""), &f).is_err());
            assert!(w.had_meeting);
            assert!(!f.calls.borrow().iter().any(|s| s.starts_with("kill ")));
        }
    }
    #[test]
    fn known_green_meeting_layout_once_then_hangup() {
        let t = tempfile::tempdir().unwrap();
        let p = paths(&t);
        let mut f = Fake::default();
        f.respond("niri",&["msg","--json","windows"],CmdResult::success(r#"[{"id":7,"app_id":"zoom","title":"Meeting synthetic"},{"id":8,"app_id":"zoom","title":"Zoom Workplace"}]"#));
        for a in [
            vec!["msg", "action", "move-window-to-tiling", "--id", "7"],
            vec!["msg", "action", "set-window-width", "--id", "7", "648"],
            vec!["msg", "action", "set-window-height", "--id", "7", "480"],
            vec!["msg", "action", "focus-window", "--id", "7"],
            vec!["msg", "action", "close-window", "--id", "8"],
        ] {
            f.respond("niri", &a, CmdResult::success(""));
        }
        let mut w = Watch::default();
        w.update(&p, &parse_config(""), &f).unwrap();
        assert!(w.had_meeting);
        assert_eq!(w.laid_out.len(), 1);
        w.update(&p, &parse_config(""), &f).unwrap();
        assert_eq!(
            f.calls
                .borrow()
                .iter()
                .filter(|c| c.contains("move-window-to-tiling"))
                .count(),
            1
        );
        f.respond(
            "niri",
            &["msg", "--json", "windows"],
            CmdResult::success("[]"),
        );
        no_zoom(&mut f);
        w.update(&p, &parse_config(""), &f).unwrap();
        assert!(!w.had_meeting);
    }
    #[test]
    fn action_failure_is_red_and_retryable() {
        let t = tempfile::tempdir().unwrap();
        let mut f = Fake::default();
        f.respond(
            "niri",
            &["msg", "--json", "windows"],
            CmdResult::success(r#"[{"id":7,"app_id":"zoom","title":"Meeting"}]"#),
        );
        let mut w = Watch::default();
        assert!(w.update(&paths(&t), &parse_config(""), &f).is_err());
        assert!(w.laid_out.is_empty());
    }
    #[test]
    fn launch_grace_defers_hangup_and_heartbeat_is_readable() {
        let t = tempfile::tempdir().unwrap();
        let p = paths(&t);
        mark_launching(&p).unwrap();
        let mut f = Fake::default();
        f.respond(
            "niri",
            &["msg", "--json", "windows"],
            CmdResult::success("[]"),
        );
        let mut w = Watch {
            had_meeting: true,
            ..Default::default()
        };
        w.update(&p, &parse_config(""), &f).unwrap();
        assert!(w.had_meeting);
        assert_eq!(f.calls.borrow().len(), 1);
        heartbeat(&p, "connected", None).unwrap();
        let v: serde_json::Value = serde_json::from_slice(
            &fs::read(p.home.join(".local/state/watchers/zoom-niri.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(v["status"], "connected");
        assert!(v["last_error"].is_null());
    }
    #[test]
    fn process_discovery_failure_is_not_no_processes() {
        let mut f = Fake::default();
        assert!(zoom_pids(&f).is_err());
        no_zoom(&mut f);
        assert!(zoom_pids(&f).unwrap().is_empty());
    }
}
