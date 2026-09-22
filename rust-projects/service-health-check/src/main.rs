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
//!
//! Two more changes the same afternoon: the `/tmp/*.lock` age check is gone —
//! watchers write those once at startup and never again, so every watcher up
//! for more than ten minutes read as STALE — replaced by the heartbeat files in
//! `~/.local/state/watchers` (same rules as system-health-check Check 9); and
//! exit 1 from a *checker* (cross-machine-sync-check, system-health-check,
//! dotter-drift-monitor, mailcurator-drift) means "found something", so it is
//! reported as 🟡 rather than as a broken service.

mod exec;
mod report;
mod services;

use clap::{Parser, ValueEnum};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "service-health-check", about = "Comprehensive service integrity validator")]
struct Cli {
    /// full (default) | quick | missing | heartbeats (alias: locks) | fix
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
    /// Check every watcher heartbeat in ~/.local/state/watchers
    #[value(alias = "locks")]
    Heartbeats,
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
        Action::Heartbeats => report::check_all_heartbeats(&ctx),
        Action::Fix => report::suggest_fixes(&ctx),
    };
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
