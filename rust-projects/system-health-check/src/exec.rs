//! Command execution behind a trait, so every check can be unit-tested with
//! canned `systemctl` / `launchctl` / `git` output instead of a live machine.

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

pub trait Exec {
    fn run(&self, program: &str, args: &[&str]) -> CmdResult;
    /// Is `bin` on PATH?
    fn which(&self, bin: &str) -> bool;
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

    fn which(&self, bin: &str) -> bool {
        std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
            .unwrap_or(false)
    }
}

/// Canned responses keyed by `"program arg1 arg2 …"`. Anything not scripted
/// returns exit 127 with an empty stdout, so a test that forgets a command
/// fails loudly rather than passing on an accidental empty string.
#[cfg(test)]
#[derive(Default)]
pub struct Fake {
    pub responses: HashMap<String, CmdResult>,
    pub on_path: Vec<String>,
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
}

#[cfg(test)]
impl Exec for Fake {
    fn run(&self, program: &str, args: &[&str]) -> CmdResult {
        let k = Self::key(program, args);
        self.calls.borrow_mut().push(k.clone());
        self.responses.get(&k).cloned().unwrap_or_else(|| CmdResult::failure(127, "unscripted command"))
    }
    fn which(&self, bin: &str) -> bool {
        self.on_path.iter().any(|b| b == bin)
    }
}
