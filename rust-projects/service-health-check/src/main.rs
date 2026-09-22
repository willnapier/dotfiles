//! service-health-check — service integrity validator, ported from the Nushell
//! script of the same name on 2026-09-22 (Rust by default; see
//! feedback_oxidise_by_default). Same five actions and output shape; the exit
//! code now carries the verdict (0 healthy or minor, 1 issues) — the script
//! always exited 0.
//!
//! What changed in behaviour: on Linux a service that is fired by a timer is
//! now reported on its own last result (`systemctl show` Result/ExecMainStatus)
//! instead of being dropped because its timer is healthy. That was audit
//! finding D2-18 (2026-08-01): a oneshot failing on every fire behind an active
//! timer was invisible — exactly how nimbini's `tm3-diary-capture` failed five
//! times in a row on 2026-09-22 while this check said "0 errored".

mod exec;
mod report;
mod services;

use clap::{Parser, ValueEnum};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "service-health-check", about = "Comprehensive service integrity validator")]
struct Cli {
    /// full (default) | quick | missing | locks | fix
    #[arg(value_enum, default_value_t = Action::Full)]
    action: Action,
}

#[derive(Clone, Copy, ValueEnum)]
enum Action {
    /// Full health check of all services
    Full,
    /// Quick summary: counts only
    Quick,
    /// List services whose scripts are missing
    Missing,
    /// Check all watcher lock files
    Locks,
    /// Suggest fixes for broken services
    Fix,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        eprintln!("HOME is not set");
        return ExitCode::from(2);
    };
    let ctx = services::Ctx { exec: Box::new(exec::Real), home };
    let ok = match cli.action {
        Action::Full => report::full_check(&ctx),
        Action::Quick => report::quick_check(&ctx),
        Action::Missing => report::check_missing_scripts(&ctx),
        Action::Locks => report::check_all_locks(&ctx),
        Action::Fix => report::suggest_fixes(&ctx),
    };
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
