mod exec;
use anyhow::{bail, Context, Result};
use exec::{Exec, Real};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
fn log(file: &Path, msg: &str) -> Result<()> {
    fs::create_dir_all(file.parent().context("log parent")?)?;
    writeln!(
        OpenOptions::new().create(true).append(true).open(file)?,
        "[{}] {msg}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    )?;
    Ok(())
}
fn run(home: &Path, ex: &dyn Exec) -> Result<String> {
    let logfile = home.join(".local/share/continuum/auto-import.log");
    log(&logfile, "outcome=running continuum-auto-import")?;
    let command = home.join(".local/bin/continuum");
    let mut imported = 0;
    let mut failed = 0;
    let mut eligible = 0;
    for (name, present) in [
        ("codex", home.join(".codex/sessions").is_dir()),
        (
            "goose",
            home.join(".local/share/goose/sessions/sessions.db")
                .is_file(),
        ),
    ] {
        if !present {
            log(
                &logfile,
                &format!("SKIP {name}: native session store absent"),
            )?;
            continue;
        }
        eligible += 1;
        let r = ex.run(
            command.to_str().context("continuum path")?,
            &["import", "--assistant", name],
        );
        // Keep native command output at the existing local log path, never in alerts.
        log(&logfile, &format!("{}{}", r.stdout, r.stderr))?;
        if r.ok() {
            imported += 1;
            log(&logfile, &format!("✓ Imported {name} session"))?;
        } else {
            failed += 1;
            log(
                &logfile,
                &format!("ERROR {name}: import exit={}", r.exit_code),
            )?;
        }
    }
    let summary = format!("eligible={eligible} imported={imported} failed={failed}");
    if failed > 0 {
        log(&logfile, &format!("outcome=error {summary}"))?;
        bail!("{summary}")
    }
    log(&logfile, &format!("outcome=success {summary}"))?;
    Ok(summary)
}
fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--help"] || args == ["-h"] {
        println!("Usage: continuum-auto-import\nImport latest Codex/Goose sessions when their native stores exist.");
        return 0.into();
    }
    if args == ["--version"] {
        println!("continuum-auto-import {}", env!("CARGO_PKG_VERSION"));
        return 0.into();
    }
    if !args.is_empty() {
        eprintln!("Usage: continuum-auto-import");
        return 2.into();
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        eprintln!("HOME not set");
        return 1.into();
    };
    match run(&home, &Real) {
        Ok(s) => {
            println!("continuum-auto-import: outcome=success {s}");
            0.into()
        }
        Err(e) => {
            eprintln!("continuum-auto-import: {e:#}");
            let _ = Real.run(
                home.join(".local/bin/notify-user").to_str().unwrap(),
                &[
                    "--tool",
                    "continuum-auto-import",
                    "Continuum import",
                    "Scheduled import failed; see auto-import.log",
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
    fn known_red_import_failure_and_known_green() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path();
        fs::create_dir_all(home.join(".codex/sessions")).unwrap();
        let mut f = Fake::default();
        let cmd = home.join(".local/bin/continuum");
        f.respond(
            cmd.to_str().unwrap(),
            &["import", "--assistant", "codex"],
            CmdResult::failure(1, "database unavailable"),
        );
        assert!(run(home, &f).is_err());
        assert!(
            fs::read_to_string(home.join(".local/share/continuum/auto-import.log"))
                .unwrap()
                .contains("outcome=error")
        );
        f.respond(
            cmd.to_str().unwrap(),
            &["import", "--assistant", "codex"],
            CmdResult::success("Imported 3 messages"),
        );
        assert_eq!(run(home, &f).unwrap(), "eligible=1 imported=1 failed=0");
    }
    #[test]
    fn both_adapters_attempted_even_when_first_fails() {
        let t = tempfile::tempdir().unwrap();
        let h = t.path();
        fs::create_dir_all(h.join(".codex/sessions")).unwrap();
        fs::create_dir_all(h.join(".local/share/goose/sessions")).unwrap();
        fs::write(h.join(".local/share/goose/sessions/sessions.db"), "").unwrap();
        let f = Fake::default();
        assert!(run(h, &f).is_err());
        assert_eq!(f.calls.borrow().len(), 2);
    }
    #[test]
    fn absent_stores_are_explicit_noop_and_log_failure_is_red() {
        let t = tempfile::tempdir().unwrap();
        let f = Fake::default();
        assert_eq!(run(t.path(), &f).unwrap(), "eligible=0 imported=0 failed=0");
        assert!(f.calls.borrow().is_empty());
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join(".local"), "blocked").unwrap();
        assert!(run(t.path(), &f).is_err());
    }
}
