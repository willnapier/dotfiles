//! ghostty-config-validate — validates the Ghostty config after an app
//! update. Rust port (2026-09-23) of the bash script of the same name;
//! triggered by `com.williamnapier.ghostty-config-validate` (launchd
//! WatchPaths on /Applications/Ghostty.app, Mac only), whose ExecStart path
//! is unchanged.
//!
//! 1. If the Ghostty binary is absent, exit 0 silently (nothing to validate).
//! 2. Heal `~/Library/Application Support/com.mitchellh.ghostty/config`:
//!    Ghostty auto-creates a template there on macOS when it finds no config,
//!    and that template has been corrupted by stray keystrokes before
//!    (2025-12-05). A regular file is moved aside
//!    (`config.regenerated-YYYYmmdd-HHMMSS`) and a symlink to the
//!    dotter-managed `~/.config/ghostty/config` put in its place; a missing
//!    path just gets the symlink.
//! 3. `ghostty +show-config`: non-zero → `CONFIG ERROR: …` log line and the
//!    same "Ghostty Config Broken" dialog (a user-facing `osascript` dialog,
//!    kept as-is); zero → `OK — <version>` log line.
//!
//! Semantic differences from the script:
//! - A broken config is now exit 1 (the script exited 0 whatever it found),
//!   with `last_error` in the recorded outcome
//!   (`~/.local/state/watchers/ghostty-config-validate.json`, interval 0:
//!   event-driven, so never "stale").
//! - A failed heal (rename/symlink) is logged as `HEAL FAILED: …`, recorded
//!   as `last_error`, and the run continues to validation; exit 1.
//! - The tool's own log `~/.local/share/ghostty-config-validate.log` is capped
//!   at 5 MB by `logkeep` (one predecessor `.log.1`).
//!
//! Exit status: 0 = binary absent, or config valid; 1 = config invalid or a
//! heal step failed.

mod exec;
mod outcome;

use clap::Parser;
use exec::Exec;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "ghostty-config-validate", version, about = "Heal the Ghostty Application Support config link and validate the config after an app update")]
struct Cli {}

const NAME: &str = "ghostty-config-validate";
const GHOSTTY_BIN: &str = "/Applications/Ghostty.app/Contents/MacOS/ghostty";

pub struct Paths {
    pub home: PathBuf,
    pub ghostty: PathBuf,
}

impl Paths {
    fn log(&self) -> PathBuf {
        self.home.join(".local/share/ghostty-config-validate.log")
    }
    fn app_support_config(&self) -> PathBuf {
        self.home.join("Library/Application Support/com.mitchellh.ghostty/config")
    }
    fn real_config(&self) -> PathBuf {
        self.home.join(".config/ghostty/config")
    }
}

fn append_log(path: &Path, line: &str) {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {line}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Heal {
    /// Already a symlink (or whatever is there is not a regular file) — untouched.
    Nothing,
    /// A regular file was moved aside to the returned path and re-linked.
    RelinkedRegular(PathBuf),
    /// Nothing was there; the link was created.
    LinkedMissing,
}

/// Step 2. Returns the log line to write (if any) and what happened.
pub fn heal(paths: &Paths, stamp: &str) -> Result<Heal, String> {
    let app = paths.app_support_config();
    let real = paths.real_config();
    let meta = std::fs::symlink_metadata(&app);
    match meta {
        Ok(m) if m.file_type().is_symlink() => Ok(Heal::Nothing),
        Ok(m) if m.is_file() => {
            let aside = PathBuf::from(format!("{}.regenerated-{stamp}", app.display()));
            std::fs::rename(&app, &aside).map_err(|e| format!("move {} aside: {e}", app.display()))?;
            std::os::unix::fs::symlink(&real, &app).map_err(|e| format!("symlink {} → {}: {e}", app.display(), real.display()))?;
            Ok(Heal::RelinkedRegular(aside))
        }
        Ok(_) => Ok(Heal::Nothing),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(dir) = app.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
            }
            std::os::unix::fs::symlink(&real, &app).map_err(|e| format!("symlink {} → {}: {e}", app.display(), real.display()))?;
            Ok(Heal::LinkedMissing)
        }
        Err(e) => Err(format!("stat {}: {e}", app.display())),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok(String),
    Broken(String),
}

/// Step 3: the validation verdict from `ghostty +show-config` (and `--version` when valid).
pub fn validate(exec: &dyn Exec, ghostty: &str) -> Verdict {
    let r = exec.run(ghostty, &["+show-config"]);
    if r.ok() {
        let v = exec.run(ghostty, &["--version"]);
        let first = v.stdout.lines().next().unwrap_or("").to_string();
        Verdict::Ok(first)
    } else {
        Verdict::Broken(r.stderr.trim_end().to_string())
    }
}

fn dialog_script(error: &str) -> String {
    format!(
        "display dialog \"Ghostty config is invalid after update. Fix ~/dotfiles/ghostty/config before launching.\n\nError: {error}\" with title \"Ghostty Config Broken\" buttons {{\"OK\"}} default button \"OK\" with icon caution giving up after 30"
    )
}

/// The whole run. Returns the exit code.
pub fn run(exec: &dyn Exec, paths: &Paths) -> i32 {
    if !paths.ghostty.is_file() {
        return 0;
    }
    let started = outcome::now_rfc3339();
    let log = paths.log();
    if let Err(e) = logkeep::cap(&log, 5 * logkeep::MB) {
        eprintln!("{NAME}: could not cap {}: {e}", log.display());
    }
    let mut errors: Vec<String> = vec![];
    let mut actions = 0;
    let mut last_action = None;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    match heal(paths, &stamp) {
        Ok(Heal::RelinkedRegular(_)) => {
            append_log(&log, &format!("HEALED — Application Support config was a regular file, symlinked to {}", paths.real_config().display()));
            actions += 1;
            last_action = Some("healed regular file".to_string());
        }
        Ok(Heal::LinkedMissing) => {
            append_log(&log, &format!("HEALED — Application Support config was missing, symlinked to {}", paths.real_config().display()));
            actions += 1;
            last_action = Some("healed missing link".to_string());
        }
        Ok(Heal::Nothing) => {}
        Err(e) => {
            append_log(&log, &format!("HEAL FAILED: {e}"));
            errors.push(format!("heal failed: {e}"));
        }
    }
    match validate(exec, &paths.ghostty.to_string_lossy()) {
        Verdict::Ok(version) => {
            append_log(&log, &format!("OK — {version}"));
            actions += 1;
            last_action = Some(format!("validated {version}"));
        }
        Verdict::Broken(err) => {
            append_log(&log, &format!("CONFIG ERROR: {err}"));
            let _ = exec.run("osascript", &["-e", &dialog_script(&err)]);
            errors.push(format!("config invalid: {}", err.lines().next().unwrap_or("")));
        }
    }
    let last_error = if errors.is_empty() { None } else { Some(errors.join("; ")) };
    let code = if last_error.is_some() { 1 } else { 0 };
    let o = outcome::Outcome { name: NAME, interval_secs: 0, started_at: started, last_action, actions, last_error };
    if let Err(e) = outcome::record(&outcome::state_dir(&paths.home), &o) {
        eprintln!("{NAME}: heartbeat write failed: {e}");
    }
    code
}

fn main() {
    let _ = Cli::parse();
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/Users/williamnapier"));
    let paths = Paths { home, ghostty: PathBuf::from(GHOSTTY_BIN) };
    std::process::exit(run(&exec::Real, &paths));
}

#[cfg(test)]
mod tests {
    use super::*;
    use exec::{CmdResult, Fake};

    /// A tempdir home with a fake ghostty binary path that exists, the real
    /// config present, and no Application Support entry.
    fn fixture() -> (tempfile::TempDir, Paths) {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().to_path_buf();
        std::fs::create_dir_all(home.join(".config/ghostty")).unwrap();
        std::fs::write(home.join(".config/ghostty/config"), "font-size = 14\n").unwrap();
        let ghostty = home.join("ghostty-bin");
        std::fs::write(&ghostty, "").unwrap();
        (d, Paths { home, ghostty })
    }

    fn ok_exec(ghostty: &Path) -> Fake {
        let mut f = Fake::default();
        let g = ghostty.to_string_lossy().to_string();
        f.respond(&g, &["+show-config"], CmdResult::success("font-size = 14\n"));
        f.respond(&g, &["--version"], CmdResult::success("Ghostty 1.2.0\n\nBuild Config\n"));
        f
    }

    fn log_text(p: &Paths) -> String {
        std::fs::read_to_string(p.log()).unwrap_or_default()
    }

    #[test]
    fn heal_moves_a_regular_file_aside_and_links() {
        let (_d, p) = fixture();
        let app = p.app_support_config();
        std::fs::create_dir_all(app.parent().unwrap()).unwrap();
        std::fs::write(&app, "garbage").unwrap();
        let r = heal(&p, "20260923-013000").unwrap();
        let aside = app.with_file_name("config.regenerated-20260923-013000");
        assert_eq!(r, Heal::RelinkedRegular(aside.clone()));
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), "garbage");
        assert_eq!(std::fs::read_link(&app).unwrap(), p.real_config());
        assert_eq!(heal(&p, "x").unwrap(), Heal::Nothing, "idempotent once linked");
    }

    #[test]
    fn heal_creates_the_link_when_missing() {
        let (_d, p) = fixture();
        assert_eq!(heal(&p, "s").unwrap(), Heal::LinkedMissing);
        assert_eq!(std::fs::read_link(p.app_support_config()).unwrap(), p.real_config());
    }

    #[test]
    fn valid_config_logs_ok_green_control() {
        let (_d, p) = fixture();
        let f = ok_exec(&p.ghostty);
        assert_eq!(run(&f, &p), 0);
        let log = log_text(&p);
        assert!(log.contains("HEALED — Application Support config was missing, symlinked to"), "{log}");
        assert!(log.contains("OK — Ghostty 1.2.0\n"), "{log}");
        assert!(f.calls.borrow().iter().all(|c| !c.starts_with("osascript")), "no dialog");
        let hb = std::fs::read_to_string(outcome::state_dir(&p.home).join("ghostty-config-validate.json")).unwrap();
        assert!(hb.contains("\"last_error\":null") && hb.contains("\"interval_secs\":0") && hb.contains("\"actions\":2"), "{hb}");
    }

    #[test]
    fn broken_config_dialogs_and_exits_one_red_control() {
        let (_d, p) = fixture();
        let mut f = Fake::default();
        let g = p.ghostty.to_string_lossy().to_string();
        f.respond(&g, &["+show-config"], CmdResult::failure(1, "error: unknown key \"fnt-size\"\n"));
        assert_eq!(run(&f, &p), 1);
        let log = log_text(&p);
        assert!(log.contains("CONFIG ERROR: error: unknown key \"fnt-size\"\n"), "{log}");
        let calls = f.calls.borrow();
        let dialog = calls.iter().find(|c| c.starts_with("osascript -e display dialog")).expect("dialog shown");
        assert!(dialog.contains("Ghostty Config Broken") && dialog.contains("Error: error: unknown key"), "{dialog}");
        let hb = std::fs::read_to_string(outcome::state_dir(&p.home).join("ghostty-config-validate.json")).unwrap();
        assert!(hb.contains("\"last_error\":\"config invalid: error: unknown key"), "{hb}");
    }

    #[test]
    fn missing_binary_is_a_silent_zero() {
        let (_d, mut p) = fixture();
        p.ghostty = p.home.join("no-such-ghostty");
        let f = Fake::default();
        assert_eq!(run(&f, &p), 0);
        assert!(f.calls.borrow().is_empty());
        assert!(!p.log().exists());
        assert!(!outcome::state_dir(&p.home).join("ghostty-config-validate.json").exists());
    }

    #[test]
    fn heal_failure_is_recorded_and_validation_still_runs() {
        let (_d, p) = fixture();
        // A regular file where the App Support parent directory should be:
        // every heal step fails with ENOTDIR.
        std::fs::create_dir_all(p.home.join("Library")).unwrap();
        std::fs::write(p.home.join("Library/Application Support"), "not a dir").unwrap();
        let f = ok_exec(&p.ghostty);
        assert_eq!(run(&f, &p), 1);
        let log = log_text(&p);
        assert!(log.contains("HEAL FAILED: "), "{log}");
        assert!(log.contains("OK — Ghostty 1.2.0"), "validation still ran: {log}");
    }
}
