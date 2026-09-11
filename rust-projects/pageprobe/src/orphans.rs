//! Detection of stray headless Chrome processes — the pure, testable half of
//! `pageprobe reap` (the IO half lives in `commands/reap.rs`).
//!
//! Why this exists (2026-09-11): a hand-rolled
//! `Google Chrome --headless=new --screenshot=… --user-data-dir=$(mktemp -d)`
//! run from an assistant session wrote its PNG but never exited. Chrome's
//! auto-update then relaunched the zombie with its original argv, so it
//! outlived the update. For two days it sat as a second, windowless
//! "Google Chrome" in the macOS app switcher and — worse — LaunchServices
//! routed URLs and files sent via `open` into it, so they appeared nowhere.
//! `pageprobe shot` is the recipe that does not leak; `pageprobe reap` is the
//! standing remedy when something else does.
//!
//! Selection rule (borrowed from practiceforge's TM3 reaper, 2026-06-23): a
//! headless Chrome *browser* process whose parent is PID 1 has been abandoned
//! by whatever launched it — a live scraper or screenshot job is still its
//! parent. An age floor guards against racing a launcher that double-forks.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChromeProc {
    pub pid: u32,
    pub ppid: u32,
    pub age_secs: u64,
    pub exe: String,
    pub user_data_dir: Option<PathBuf>,
}

/// Chrome-family browser binaries by basename. Helpers (`--type=…`) are
/// excluded separately; this is only about the main process.
const CHROME_BASENAMES: &[&str] = &[
    "Google Chrome",
    "Google Chrome Beta",
    "Google Chrome Dev",
    "Google Chrome Canary",
    "Chromium",
    "chrome",
    "google-chrome",
    "google-chrome-stable",
    "google-chrome-beta",
    "chromium",
    "chromium-browser",
];

/// Parses the output of `ps -axo pid=,ppid=,etime=,command=` and returns the
/// Chrome browser processes (not helpers) that carry a `--headless` flag.
pub fn headless_browsers(ps_output: &str) -> Vec<ChromeProc> {
    ps_output.lines().filter_map(parse_line).collect()
}

/// One `ps` row → a headless Chrome browser process, or `None` if the row is
/// anything else (a helper, a headful Chrome, another program, garbage).
pub fn parse_line(line: &str) -> Option<ChromeProc> {
    let mut it = line.split_whitespace();
    let pid: u32 = it.next()?.parse().ok()?;
    let ppid: u32 = it.next()?.parse().ok()?;
    let age_secs = parse_etime(it.next()?)?;
    let rest: Vec<&str> = it.collect();
    if rest.is_empty() {
        return None;
    }
    let command = rest.join(" ");
    // The executable path may contain spaces (macOS app bundles); flags
    // always start with ` --`, so split there.
    let (exe, args) = match command.find(" --") {
        Some(i) => (&command[..i], &command[i..]),
        None => (command.as_str(), ""),
    };
    if !is_chrome_main(exe) {
        return None;
    }
    let tokens: Vec<&str> = args.split_whitespace().collect();
    if tokens.iter().any(|t| t.starts_with("--type=")) {
        return None; // renderer / gpu / utility helper
    }
    if !tokens
        .iter()
        .any(|t| *t == "--headless" || t.starts_with("--headless="))
    {
        return None;
    }
    let user_data_dir = tokens
        .iter()
        .find_map(|t| t.strip_prefix("--user-data-dir="))
        .map(PathBuf::from);
    Some(ChromeProc {
        pid,
        ppid,
        age_secs,
        exe: exe.to_string(),
        user_data_dir,
    })
}

fn is_chrome_main(exe: &str) -> bool {
    let base = exe.rsplit('/').next().unwrap_or(exe);
    CHROME_BASENAMES.contains(&base)
}

/// `ps` elapsed time: `MM:SS`, `HH:MM:SS` or `DD-HH:MM:SS` → seconds.
pub fn parse_etime(s: &str) -> Option<u64> {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, s),
    };
    let parts: Vec<u64> = rest
        .split(':')
        .map(|p| p.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    let secs = match parts.as_slice() {
        [m, s] => m * 60 + s,
        [h, m, s] => h * 3600 + m * 60 + s,
        _ => return None,
    };
    Some(days * 86_400 + secs)
}

/// The abandoned ones: re-parented to PID 1 and at least `min_age_secs` old.
pub fn strays(procs: &[ChromeProc], min_age_secs: u64) -> Vec<ChromeProc> {
    procs
        .iter()
        .filter(|p| p.ppid == 1 && p.age_secs >= min_age_secs)
        .cloned()
        .collect()
}

/// Is this profile directory somewhere a throwaway profile lives? Only such
/// directories are deleted after a kill; anything else is left for its owner.
pub fn is_temp_profile(dir: &Path) -> bool {
    let s = dir.to_string_lossy();
    let under = |root: &str| s.starts_with(root) && s.len() > root.len();
    if under("/tmp/") || under("/private/tmp/") || under("/var/folders/") || under("/private/var/folders/") {
        return true;
    }
    let sys = std::env::temp_dir();
    dir.starts_with(&sys) && dir != sys
}

/// `2d 10h`, `3h 12m`, `5m`, `40s` — for reports.
pub fn human_age(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3600, (secs % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZOMBIE: &str = "79109     1 02-10:30:28 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --no-startup-window --disable-gpu --headless=new --no-first-run --screenshot=/tmp/pf-functionality-overview.png --user-data-dir=/tmp/pf-doc-chrome.gy5FcR --window-size=1440,1000";
    const HELPER: &str = "79123 79109 02-10:30:47 /Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Versions/152.0.7977.83/Helpers/Google Chrome Helper.app/Contents/MacOS/Google Chrome Helper --type=gpu-process --headless=new --user-data-dir=/tmp/pf-doc-chrome.gy5FcR";
    const HEADFUL: &str = "33684     1       36:31 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --new-window http://127.0.0.1:8765/mail/personal/inbox";
    const LIVE_SCRAPE: &str = "5001  4990       01:12 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --headless --remote-debugging-port=0 --user-data-dir=/var/folders/ab/T/rust-headless-chrome-profileXYZ";
    const LINUX: &str = "2222     1 03:00:00 /opt/google/chrome/chrome --headless=new --user-data-dir=/tmp/x.abc";

    #[test]
    fn etime_formats() {
        assert_eq!(parse_etime("36:31"), Some(36 * 60 + 31));
        assert_eq!(parse_etime("01:02:03"), Some(3723));
        assert_eq!(parse_etime("02-10:30:28"), Some(2 * 86_400 + 10 * 3600 + 30 * 60 + 28));
        assert_eq!(parse_etime("garbage"), None);
        assert_eq!(parse_etime("1:2:3:4"), None);
    }

    #[test]
    fn zombie_is_detected_with_its_profile() {
        let p = parse_line(ZOMBIE).expect("zombie parses");
        assert_eq!(p.pid, 79109);
        assert_eq!(p.ppid, 1);
        assert_eq!(p.age_secs, 2 * 86_400 + 10 * 3600 + 30 * 60 + 28);
        assert_eq!(p.exe, "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
        assert_eq!(p.user_data_dir.as_deref(), Some(Path::new("/tmp/pf-doc-chrome.gy5FcR")));
    }

    #[test]
    fn helpers_headful_and_other_programs_are_ignored() {
        assert!(parse_line(HELPER).is_none(), "helper");
        assert!(parse_line(HEADFUL).is_none(), "headful");
        assert!(parse_line("1 0 00:01 /usr/bin/foo --headless").is_none(), "not chrome");
        assert!(parse_line("").is_none());
        assert!(parse_line("x y z").is_none());
    }

    #[test]
    fn linux_binary_counts() {
        let p = parse_line(LINUX).unwrap();
        assert_eq!(p.exe, "/opt/google/chrome/chrome");
    }

    #[test]
    fn only_orphans_past_the_age_floor_are_strays() {
        let all = headless_browsers(&[ZOMBIE, HELPER, HEADFUL, LIVE_SCRAPE, LINUX].join("\n"));
        assert_eq!(all.len(), 3, "{all:?}");
        let s = strays(&all, 120);
        let pids: Vec<u32> = s.iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![79109, 2222], "live scrape (ppid 4990) must be left alone");
        // A freshly re-parented process is not yet a stray.
        let young = ChromeProc { pid: 9, ppid: 1, age_secs: 30, exe: "chrome".into(), user_data_dir: None };
        assert!(strays(&[young], 120).is_empty());
    }

    #[test]
    fn temp_profile_roots() {
        assert!(is_temp_profile(Path::new("/tmp/pf-doc-chrome.gy5FcR")));
        assert!(is_temp_profile(Path::new("/private/tmp/x")));
        assert!(is_temp_profile(Path::new("/var/folders/ab/T/rust-headless-chrome-profileXYZ")));
        assert!(!is_temp_profile(Path::new("/tmp")));
        assert!(!is_temp_profile(Path::new("/tmp/")));
        assert!(!is_temp_profile(Path::new("/Users/x/.config/pageprobe/chrome-profile")));
        assert!(!is_temp_profile(Path::new("/Users/x/Library/Application Support/Google/Chrome")));
    }

    #[test]
    fn ages_read_well() {
        assert_eq!(human_age(40), "40s");
        assert_eq!(human_age(5 * 60), "5m");
        assert_eq!(human_age(3 * 3600 + 12 * 60), "3h 12m");
        assert_eq!(human_age(2 * 86_400 + 10 * 3600), "2d 10h");
    }
}
