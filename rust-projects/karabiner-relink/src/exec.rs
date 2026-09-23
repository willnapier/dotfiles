//! Command execution behind a trait, so the relink flow can be unit-tested
//! with canned `git` output instead of a live repo. Same shape as
//! service-health-check's exec.rs, plus `run_in` (a working directory) and
//! `run_in_bounded` (a kill deadline — the script's `timeout 30 git push`).

#[cfg(test)]
use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default)]
pub struct CmdResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CmdResult {
    pub fn ok(&self) -> bool {
        self.exit_code == 0
    }
    #[cfg(test)]
    pub fn success(stdout: &str) -> CmdResult {
        CmdResult { exit_code: 0, stdout: stdout.to_string(), stderr: String::new() }
    }
    #[cfg(test)]
    pub fn failure(code: i32, stderr: &str) -> CmdResult {
        CmdResult { exit_code: code, stdout: String::new(), stderr: stderr.to_string() }
    }
}

pub trait Exec {
    fn run(&self, program: &str, args: &[&str]) -> CmdResult;
    /// Same, with a working directory.
    fn run_in(&self, dir: &Path, program: &str, args: &[&str]) -> CmdResult;
    /// Same, killed after `timeout` (exit code 124, as coreutils `timeout`).
    fn run_in_bounded(&self, dir: &Path, program: &str, args: &[&str], timeout: Duration) -> CmdResult;
}

/// Runs real commands.
pub struct Real;

fn finish(o: std::io::Result<std::process::Output>) -> CmdResult {
    match o {
        Ok(o) => CmdResult {
            exit_code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        },
        // 127 mirrors the shell's "command not found"
        Err(e) => CmdResult { exit_code: 127, stdout: String::new(), stderr: e.to_string() },
    }
}

impl Exec for Real {
    fn run(&self, program: &str, args: &[&str]) -> CmdResult {
        finish(Command::new(program).args(args).output())
    }

    fn run_in(&self, dir: &Path, program: &str, args: &[&str]) -> CmdResult {
        finish(Command::new(program).args(args).current_dir(dir).output())
    }

    fn run_in_bounded(&self, dir: &Path, program: &str, args: &[&str], timeout: Duration) -> CmdResult {
        let child = Command::new(program).args(args).current_dir(dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => return CmdResult { exit_code: 127, stdout: String::new(), stderr: e.to_string() },
        };
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return finish(child.wait_with_output()),
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return CmdResult { exit_code: 124, stdout: String::new(), stderr: format!("killed after {}s", timeout.as_secs()) };
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(200)),
                Err(e) => return CmdResult { exit_code: -1, stdout: String::new(), stderr: e.to_string() },
            }
        }
    }
}

/// Canned responses keyed by `"program arg1 arg2 …"` (the working directory and
/// deadline are recorded in `calls` but do not take part in the key). Anything
/// not scripted returns exit 127 with an empty stdout, so a test that forgets
/// a command fails loudly rather than passing on an accidental empty string.
#[cfg(test)]
#[derive(Default)]
pub struct Fake {
    pub responses: HashMap<String, CmdResult>,
    pub calls: std::cell::RefCell<Vec<String>>,
}

#[cfg(test)]
impl Fake {
    pub fn key(program: &str, args: &[&str]) -> String {
        let mut k = program.to_string();
        for a in args {
            k.push(' ');
            k.push_str(a);
        }
        k
    }
    pub fn respond(&mut self, program: &str, args: &[&str], r: CmdResult) -> &mut Self {
        self.responses.insert(Self::key(program, args), r);
        self
    }
    fn record(&self, prefix: &str, k: &str) -> CmdResult {
        self.calls.borrow_mut().push(format!("{prefix}{k}"));
        self.responses.get(k).cloned().unwrap_or_else(|| CmdResult::failure(127, "unscripted command"))
    }
}

#[cfg(test)]
impl Exec for Fake {
    fn run(&self, program: &str, args: &[&str]) -> CmdResult {
        self.record("", &Self::key(program, args))
    }
    fn run_in(&self, dir: &Path, program: &str, args: &[&str]) -> CmdResult {
        self.record(&format!("[in {}] ", dir.display()), &Self::key(program, args))
    }
    fn run_in_bounded(&self, dir: &Path, program: &str, args: &[&str], timeout: Duration) -> CmdResult {
        self.record(&format!("[in {} ≤{}s] ", dir.display(), timeout.as_secs()), &Self::key(program, args))
    }
}
