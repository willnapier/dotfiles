mod exec;
use anyhow::{bail, Context, Result};
use exec::{Exec, Real};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
#[derive(Debug, Clone)]
struct Target {
    pid: u32,
    identity: String,
}
fn parse(text: &str, prefix: &str) -> Result<Vec<Target>> {
    let mut targets = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let line = line.trim();
        let (pid, identity) = line
            .split_once(char::is_whitespace)
            .context("invalid process row")?;
        let pid = pid.parse::<u32>().context("invalid PID")?;
        let identity = identity.trim();
        // Five fields of lstart (weekday month day hh:mm:ss year), then argv[0].
        let exe = identity
            .split_whitespace()
            .nth(5)
            .context("missing process command")?;
        if exe.starts_with(prefix)
            && exe.len() > prefix.len()
            && pid > 1
            && pid != std::process::id()
        {
            targets.push(Target {
                pid,
                identity: identity.into(),
            })
        }
    }
    Ok(targets)
}
fn inspect(ex: &dyn Exec, target: &Target) -> Result<bool> {
    let r = ex.run(
        "ps",
        &["-p", &target.pid.to_string(), "-o", "lstart=,args="],
    );
    if r.exit_code == 1 && r.out().is_empty() {
        return Ok(false);
    }
    if !r.ok() {
        bail!("cannot recheck PID {}", target.pid)
    }
    if r.out().is_empty() {
        bail!("empty successful process inspection")
    }
    Ok(r.out() == target.identity)
}
fn terminate(ex: &dyn Exec, targets: &[Target]) -> Result<usize> {
    let mut errors = Vec::new();
    let mut signalled = 0;
    for t in targets {
        match inspect(ex, t) {
            Ok(false) => continue,
            Err(e) => {
                errors.push(e.to_string());
                continue;
            }
            Ok(true) => {}
        }
        let r = ex.run("kill", &["-TERM", &t.pid.to_string()]);
        if !r.ok() {
            if inspect(ex, t).unwrap_or(true) {
                errors.push(format!("TERM failed for PID {}", t.pid));
            }
        } else {
            signalled += 1
        }
    }
    if signalled > 0 && !ex.run("sleep", &["5"]).ok() {
        errors.push("termination grace wait failed".into())
    }
    for t in targets {
        match inspect(ex, t) {
            Ok(false) => continue,
            Err(e) => {
                errors.push(e.to_string());
                continue;
            }
            Ok(true) => {}
        }
        if !ex.run("kill", &["-KILL", &t.pid.to_string()]).ok() && inspect(ex, t).unwrap_or(true) {
            errors.push(format!("KILL failed for PID {}", t.pid))
        }
    }
    if signalled > 0 && !ex.run("sleep", &["0.2"]).ok() {
        errors.push("final wait failed".into())
    }
    for t in targets {
        match inspect(ex, t) {
            Ok(false) => {}
            Ok(true) => errors.push(format!("PID {} still running", t.pid)),
            Err(e) => errors.push(e.to_string()),
        }
    }
    if !errors.is_empty() {
        bail!("{}", errors.join("; "))
    }
    Ok(signalled)
}
fn log(home: &Path, message: &str) -> Result<()> {
    let p = home.join(".local/share/claude-code-nightly-cleanup.log");
    fs::create_dir_all(p.parent().unwrap())?;
    writeln!(
        OpenOptions::new().create(true).append(true).open(p)?,
        "{} {message}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    )?;
    Ok(())
}
fn run(home: &Path, dry: bool, ex: &dyn Exec) -> Result<()> {
    log(home, "outcome=running nightly cleanup")?;
    let uid = ex.run("id", &["-u"]);
    if !uid.ok() {
        bail!("cannot determine process owner")
    }
    let r = ex.run("ps", &["-u", &uid.out(), "-o", "pid=,lstart=,args="]);
    if !r.ok() {
        bail!("process discovery failed")
    }
    if r.out().is_empty() {
        bail!("process discovery returned an empty population")
    }
    let prefix = format!("{}/.local/share/claude/versions/", home.display());
    let targets = parse(&r.stdout, &prefix)?;
    if targets.is_empty() {
        log(
            home,
            "No Claude Code processes found; outcome=success matched=0",
        )?;
        return Ok(());
    }
    log(
        home,
        &format!(
            "{} {} Claude Code process(es): {}",
            if dry { "WOULD-KILL" } else { "Killing" },
            targets.len(),
            targets
                .iter()
                .map(|t| t.pid.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        ),
    )?;
    if dry {
        log(
            home,
            &format!("outcome=success dry-run matched={}", targets.len()),
        )?;
        return Ok(());
    }
    let count = terminate(ex, &targets)?;
    log(
        home,
        &format!(
            "Cleanup complete; outcome=success matched={} signalled={count}",
            targets.len()
        ),
    )?;
    Ok(())
}
fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--help"] || args == ["-h"] {
        println!("Usage: claude-code-nightly-cleanup [--dry-run]\nTerminate this user's Claude Code version-path processes; TERM, 5s grace, then KILL of the same identities.");
        return 0.into();
    };
    if args == ["--version"] {
        println!("claude-code-nightly-cleanup {}", env!("CARGO_PKG_VERSION"));
        return 0.into();
    };
    if !args.is_empty() && args != ["--dry-run"] {
        eprintln!("Usage: claude-code-nightly-cleanup [--dry-run]");
        return 2.into();
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        eprintln!("HOME not set");
        return 1.into();
    };
    match run(&home, !args.is_empty(), &Real) {
        Ok(()) => {
            println!("claude-code-nightly-cleanup: outcome=success");
            0.into()
        }
        Err(e) => {
            let _ = log(&home, &format!("outcome=error {e:#}"));
            eprintln!("claude-code-nightly-cleanup: {e:#}");
            let _ = Real.run(
                home.join(".local/bin/notify-user").to_str().unwrap(),
                &[
                    "--tool",
                    "claude-code-nightly-cleanup",
                    "Claude Code cleanup",
                    "Nightly process cleanup failed; see local log",
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
    use std::cell::RefCell;
    use std::collections::VecDeque;
    const ID: &str = "Wed Sep 23 00:00:00 2026 /home/test/.local/share/claude/versions/2.0";
    #[test]
    fn exact_executable_selection_excludes_shell_mentions() {
        let data=format!("100 {ID}\n101 Wed Sep 23 00:00:00 2026 bash -c /home/test/.local/share/claude/versions/2.0\n102 Wed Sep 23 00:00:00 2026 /other/.local/share/claude/versions/2.0");
        let t = parse(&data, "/home/test/.local/share/claude/versions/").unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].pid, 100);
        assert!(parse("bad", "prefix").is_err());
    }
    struct Sequence {
        answers: RefCell<VecDeque<exec::CmdResult>>,
        calls: RefCell<Vec<String>>,
    }
    impl Exec for Sequence {
        fn run(&self, p: &str, a: &[&str]) -> exec::CmdResult {
            self.calls.borrow_mut().push(Fake::key(p, a));
            self.answers
                .borrow_mut()
                .pop_front()
                .unwrap_or(CmdResult::failure(127, "unscripted"))
        }
    }
    #[test]
    fn term_exit_green_and_unreaped_survivor_red() {
        let target = Target {
            pid: 100,
            identity: ID.into(),
        };
        let s = Sequence {
            answers: RefCell::new(VecDeque::from([
                CmdResult::success(ID),
                CmdResult::success(""),
                CmdResult::success(""),
                CmdResult::failure(1, ""),
                CmdResult::success(""),
                CmdResult::failure(1, ""),
            ])),
            calls: RefCell::new(vec![]),
        };
        assert_eq!(terminate(&s, &[target.clone()]).unwrap(), 1);
        assert!(!s.calls.borrow().iter().any(|c| c.contains("-KILL")));
        let mut f = Fake::default();
        f.respond(
            "ps",
            &["-p", "100", "-o", "lstart=,args="],
            CmdResult::success(ID),
        );
        assert!(terminate(&f, &[target]).is_err());
    }
    #[test]
    fn reused_pid_never_signalled() {
        let t = Target {
            pid: 100,
            identity: ID.into(),
        };
        let mut f = Fake::default();
        f.respond(
            "ps",
            &["-p", "100", "-o", "lstart=,args="],
            CmdResult::success("Wed Sep 23 00:01:00 2026 /bin/editor"),
        );
        assert_eq!(terminate(&f, &[t]).unwrap(), 0);
        assert!(!f.calls.borrow().iter().any(|c| c.starts_with("kill ")));
    }
    #[test]
    fn no_match_green_discovery_failure_red() {
        let h = tempfile::tempdir().unwrap();
        let mut f = Fake::default();
        f.respond("id", &["-u"], CmdResult::success("1000"));
        f.respond(
            "ps",
            &["-u", "1000", "-o", "pid=,lstart=,args="],
            CmdResult::success("100 Wed Sep 23 00:00:00 2026 /bin/sh"),
        );
        assert!(run(h.path(), false, &f).is_ok());
        f.respond(
            "ps",
            &["-u", "1000", "-o", "pid=,lstart=,args="],
            CmdResult::failure(2, "ps failed"),
        );
        assert!(run(h.path(), false, &f).is_err());
    }
}
