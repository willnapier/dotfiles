mod exec;
use anyhow::{bail, Context, Result};
use exec::{Exec, Real};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
fn run(home: &Path, ex: &dyn Exec) -> Result<()> {
    let path = home.join(".local/share/collect-entries-cron.log");
    fs::create_dir_all(path.parent().context("log parent")?)?;
    let mut log = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(
        log,
        "=== {} ===\noutcome=running collect-projects-cron",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    )?;
    let cmd = home.join(".local/bin/collect-entries");
    let result = ex.run(cmd.to_str().context("command path")?, &["--verbose"]);
    write!(log, "{}{}", result.stdout, result.stderr)?;
    writeln!(
        log,
        "\noutcome={} collect-entries exit={}\n",
        if result.ok() { "success" } else { "error" },
        result.exit_code
    )?;
    log.sync_data()?;
    if !result.ok() {
        bail!(
            "collect-entries failed (exit {}); see collect-entries-cron.log",
            result.exit_code
        )
    }
    Ok(())
}
fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--help"] || args == ["-h"] {
        println!("Usage: collect-projects-cron\nRun collect-entries --verbose; log to ~/.local/share/collect-entries-cron.log.");
        return 0.into();
    }
    if args == ["--version"] {
        println!("collect-projects-cron {}", env!("CARGO_PKG_VERSION"));
        return 0.into();
    }
    if !args.is_empty() {
        eprintln!("Usage: collect-projects-cron");
        return 2.into();
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        eprintln!("HOME not set");
        return 1.into();
    };
    match run(&home, &Real) {
        Ok(()) => {
            println!("collect-projects-cron: outcome=success");
            0.into()
        }
        Err(e) => {
            eprintln!("collect-projects-cron: {e:#}");
            let _ = Real.run(
                home.join(".local/bin/notify-user").to_str().unwrap(),
                &[
                    "--tool",
                    "collect-projects-cron",
                    "DayPage collection",
                    "Collection failed; see collect-entries-cron.log",
                ],
            );
            1.into()
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use exec::{CmdResult, Fake};
    #[test]
    fn known_green_and_known_red_exit_and_log() {
        let t = tempfile::tempdir().unwrap();
        let h = t.path();
        let cmd = h.join(".local/bin/collect-entries");
        let mut f = Fake::default();
        f.respond(
            cmd.to_str().unwrap(),
            &["--verbose"],
            CmdResult::success("Collected 2 synthetic entries\n"),
        );
        assert!(run(h, &f).is_ok());
        let log = h.join(".local/share/collect-entries-cron.log");
        assert!(fs::read_to_string(&log)
            .unwrap()
            .contains("outcome=success collect-entries exit=0"));
        f.respond(
            cmd.to_str().unwrap(),
            &["--verbose"],
            CmdResult::failure(3, "entry store unavailable"),
        );
        assert!(run(h, &f).is_err());
        assert!(fs::read_to_string(log)
            .unwrap()
            .contains("outcome=error collect-entries exit=3"));
    }
    #[test]
    fn missing_executable_is_red() {
        let t = tempfile::tempdir().unwrap();
        assert!(run(t.path(), &Fake::default()).is_err());
    }
    #[test]
    fn unwritable_log_prevents_mutating_collection() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join(".local"), "blocked").unwrap();
        let f = Fake::default();
        assert!(run(t.path(), &f).is_err());
        assert!(f.calls.borrow().is_empty());
    }
}
