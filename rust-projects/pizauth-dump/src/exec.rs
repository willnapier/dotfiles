//! Command execution behind a trait, so discovery can be unit-tested with
//! canned `systemctl` / `launchctl` / `PlistBuddy` output instead of a live
//! machine. Same shape as system-health-check's exec.rs, minus `which`.

#[cfg(test)]
use std::collections::HashMap;
use std::process::Command;

#[derive(Debug, Clone, Default)]
pub struct CmdResult {
    pub exit_code: i32,
    pub stdout: String,
    /// Kept for diagnostics when debugging a fake or a failed command.
    #[allow(dead_code)]
    pub stderr: String,
}

impl CmdResult {
    pub fn ok(&self) -> bool {
        self.exit_code == 0
    }
    /// Trimmed stdout.
    #[allow(dead_code)]
    pub fn out(&self) -> String {
        self.stdout.trim().to_string()
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

/// Raw stdout for commands whose output is bytes, not text (an encrypted
/// dump). `CmdResult` goes through `from_utf8_lossy`, which silently replaces
/// every invalid sequence with U+FFFD — the 2026-09-23 first install wrote a
/// dump `pizauth restore` could not read that way.
#[derive(Debug, Clone, Default)]
pub struct RawResult {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
}

impl RawResult {
    pub fn ok(&self) -> bool {
        self.exit_code == 0
    }
}

pub trait Exec {
    fn run(&self, program: &str, args: &[&str]) -> CmdResult;
    fn run_raw(&self, program: &str, args: &[&str]) -> RawResult;
}

/// Runs real commands.
pub struct Real;

impl Exec for Real {
    fn run(&self, program: &str, args: &[&str]) -> CmdResult {
        match Command::new(program).args(args).output() {
            Ok(o) => CmdResult {
                exit_code: o.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            },
            // 127 mirrors the shell's "command not found"
            Err(e) => CmdResult { exit_code: 127, stdout: String::new(), stderr: e.to_string() },
        }
    }

    fn run_raw(&self, program: &str, args: &[&str]) -> RawResult {
        match Command::new(program).args(args).output() {
            Ok(o) => RawResult { exit_code: o.status.code().unwrap_or(-1), stdout: o.stdout },
            Err(_) => RawResult { exit_code: 127, stdout: Vec::new() },
        }
    }
}

/// Canned responses keyed by `"program arg1 arg2 …"`. Anything not scripted
/// returns exit 127 with an empty stdout, so a test that forgets a command
/// fails loudly rather than passing on an accidental empty string.
#[cfg(test)]
#[derive(Default)]
pub struct Fake {
    pub responses: HashMap<String, CmdResult>,
    /// Byte responses for `run_raw`; a key absent here falls back to `responses`.
    pub raw_responses: HashMap<String, RawResult>,
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
    pub fn respond_raw(&mut self, program: &str, args: &[&str], exit_code: i32, stdout: &[u8]) -> &mut Self {
        self.raw_responses.insert(Self::key(program, args), RawResult { exit_code, stdout: stdout.to_vec() });
        self
    }
}

#[cfg(test)]
impl Exec for Fake {
    fn run(&self, program: &str, args: &[&str]) -> CmdResult {
        let k = Self::key(program, args);
        self.calls.borrow_mut().push(k.clone());
        self.responses.get(&k).cloned().unwrap_or_else(|| CmdResult::failure(127, "unscripted command"))
    }
    fn run_raw(&self, program: &str, args: &[&str]) -> RawResult {
        let k = Self::key(program, args);
        self.calls.borrow_mut().push(k.clone());
        if let Some(r) = self.raw_responses.get(&k) {
            return r.clone();
        }
        match self.responses.get(&k) {
            Some(c) => RawResult { exit_code: c.exit_code, stdout: c.stdout.as_bytes().to_vec() },
            None => RawResult { exit_code: 127, stdout: Vec::new() },
        }
    }
}
