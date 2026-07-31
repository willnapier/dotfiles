//! rust-redeploy — turn `cross-machine-sync-check`'s drift report into a fix.
//!
//! # Why this exists
//!
//! Source for the `~/dotfiles/rust-projects` tools is shared by git, but the *binaries*
//! are per-machine. The Mac rebuilds naturally because that is where edits happen —
//! edit, `cargo install`, deploy. Nimbini only ever *pulls* source, and nothing there
//! rebuilds, so every Mac-side change left nimbini's binary stale **by construction**.
//! On 2026-07-31 that had reached 92 days on `dev-catchup`, across 8 tools at once.
//!
//! Detection was already automatic. This closes the loop so the fix is one command
//! rather than an audit.
//!
//! # Why it is NOT wired to the auto-pull watcher
//!
//! `git-auto-pull-watcher` pulls every 2 minutes. Rebuilding from it would turn a bad
//! commit on one machine into a broken binary on the other within 2 minutes, unattended,
//! including an automatic stop/start of a live service — and it would put multi-minute
//! cargo builds on a 2-minute timer. The failure mode is worse than the staleness.
//! Detection stays automatic; the fix stays deliberate.

use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Tools that must carry a Developer ID signature on macOS or the kernel SIGKILLs them
/// on launch (`cs_invalid_page`). Source of truth is the deploy runbook in
/// `senior-dev/TECHNICAL-ENVIRONMENT.md`; keep the two in step.
const MACOS_SIGNED_TOOLS: &[&str] = &[
    "practiceforge",
    "pizauth",
    "tm3-diary-capture",
    "mailcurator",
    "continuum-claude",
    "continuum-grok",
    "dev-catchup",
];

const MACOS_SIGNING_IDENTITY: &str = "Developer ID Application: William Napier (LU3TB2NLTD)";

#[derive(Parser)]
#[command(
    name = "rust-redeploy",
    about = "Rebuild and redeploy the ~/dotfiles/rust-projects binaries reported stale on this machine"
)]
struct Cli {
    /// Show the plan and exit. Builds nothing, deploys nothing.
    #[arg(short = 'n', long)]
    dry_run: bool,

    /// Restrict to these projects (repeatable). Default: everything reported stale.
    #[arg(short, long)]
    only: Vec<String>,

    /// Value for CARGO_BUILD_JOBS. Capped low by default: an uncapped workspace build
    /// OOM-crashed nimbini on 2026-07-25 and took two reboots to recover.
    #[arg(short, long, default_value_t = 4)]
    jobs: usize,

    /// Ignore the drift report and consider every deployed project. Slow; for recovery
    /// when mtimes are untrustworthy (a restore, a clock jump, a bulk `touch`).
    #[arg(long)]
    all: bool,

    /// Machine-readable summary on stdout.
    #[arg(long)]
    json: bool,
}

#[derive(Deserialize)]
struct CheckResult {
    name: String,
    status: String,
}

#[derive(Debug, serde::Serialize)]
struct Outcome {
    project: String,
    result: String,
    detail: String,
}

fn home() -> PathBuf {
    dirs::home_dir().expect("cannot determine home directory")
}

fn bin_dir() -> PathBuf {
    home().join(".local/bin")
}

fn projects_dir() -> PathBuf {
    home().join("dotfiles/rust-projects")
}

/// If `path` is a symlink resolving outside `~/.local/bin`, return the target.
///
/// This is the `forge-metadata-backup` class: a project can exist both as Rust and as a
/// nushell script of the same name, with `~/.local/bin/<name>` symlinked to
/// `~/dotfiles/scripts/<name>`. Deploying onto that path with a plain `cp` **follows the
/// link and overwrites the script in the dotfiles source tree** — which is exactly how a
/// 1.5 MB ELF landed on top of a 7 KB nu script on 2026-07-31.
fn link_escaping_bin_dir(path: &Path) -> Option<PathBuf> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_symlink() {
        return None;
    }
    let target = std::fs::canonicalize(path).unwrap_or(std::fs::read_link(path).ok()?);
    let bin = std::fs::canonicalize(bin_dir()).unwrap_or_else(|_| bin_dir());
    if target.starts_with(&bin) {
        None
    } else {
        Some(target)
    }
}

fn sha256_of(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

/// PIDs currently executing `path`.
///
/// Linux reads `/proc/<pid>/exe`, which is exact. macOS has no `/proc`, so it falls back
/// to matching the full path in `pgrep -f` — good enough to *refuse*, which is all the
/// macOS branch does with the answer.
fn processes_running(path: &Path) -> Vec<String> {
    let mut pids = Vec::new();

    if cfg!(target_os = "linux") {
        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                if let Ok(exe) = std::fs::read_link(entry.path().join("exe")) {
                    if exe == path {
                        pids.push(name);
                    }
                }
            }
        }
    } else if let Ok(out) = Command::new("pgrep").args(["-f", &path.to_string_lossy()]).output() {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            pids.push(line.trim().to_string());
        }
    }

    pids
}

/// An active `<name>.service` systemd --user unit, if one exists (Linux only).
fn active_user_unit(name: &str) -> Option<String> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let unit = format!("{}.service", name);
    let out = Command::new("systemctl")
        .args(["--user", "is-active", &unit])
        .output()
        .ok()?;
    if String::from_utf8_lossy(&out.stdout).trim() == "active" {
        Some(unit)
    } else {
        None
    }
}

fn systemctl(action: &str, unit: &str) -> Result<()> {
    let status = Command::new("systemctl")
        .args(["--user", action, unit])
        .status()
        .with_context(|| format!("failed to run systemctl {} {}", action, unit))?;
    if !status.success() {
        bail!("systemctl {} {} failed", action, unit);
    }
    Ok(())
}

/// Which projects to act on: the drift report by default, everything deployed under `--all`.
fn select_projects(cli: &Cli) -> Result<Vec<String>> {
    let mut names = if cli.all {
        let mut all = Vec::new();
        for entry in std::fs::read_dir(projects_dir())?.flatten() {
            if !entry.path().join("Cargo.toml").exists() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if bin_dir().join(&name).exists() {
                all.push(name);
            }
        }
        all
    } else {
        let out = Command::new("cross-machine-sync-check")
            .args(["--json", "--local-only"])
            .output()
            .context("failed to run cross-machine-sync-check — is it on PATH?")?;
        let results: Vec<CheckResult> = serde_json::from_slice(&out.stdout)
            .context("could not parse cross-machine-sync-check --json output")?;
        results
            .iter()
            .filter(|r| r.status == "Drift")
            .filter_map(|r| r.name.strip_prefix("rust-binary/").map(str::to_string))
            .collect()
    };

    if !cli.only.is_empty() {
        names.retain(|n| cli.only.contains(n));
        for wanted in &cli.only {
            if !names.contains(wanted) {
                eprintln!("note: {} is not in the current drift set — skipping", wanted);
            }
        }
    }

    names.sort();
    names.dedup();
    Ok(names)
}

/// Copy `from` over `to`, **removing the destination first**.
///
/// The removal is the entire point. `cp src dest` where `dest` is a symlink writes through
/// the link into whatever it targets; removing first replaces the link itself. Callers are
/// expected to have refused escaping symlinks already — this is the second line of defence.
fn deploy_file(from: &Path, to: &Path) -> Result<()> {
    if std::fs::symlink_metadata(to).is_ok() {
        std::fs::remove_file(to).with_context(|| format!("cannot remove {}", to.display()))?;
    }
    std::fs::copy(from, to)
        .with_context(|| format!("cannot copy {} -> {}", from.display(), to.display()))?;
    Ok(())
}

fn codesign(path: &Path) -> Result<()> {
    let status = Command::new("codesign")
        .args([
            "--force",
            "--sign",
            MACOS_SIGNING_IDENTITY,
            "--options",
            "runtime",
            "--timestamp",
        ])
        .arg(path)
        .status()
        .context("failed to run codesign")?;
    if !status.success() {
        bail!("codesign failed for {}", path.display());
    }
    let verify = Command::new("codesign")
        .args(["--verify", "--strict"])
        .arg(path)
        .status()
        .context("failed to run codesign --verify")?;
    if !verify.success() {
        bail!("codesign --verify --strict failed for {}", path.display());
    }
    Ok(())
}

fn redeploy_one(name: &str, cli: &Cli) -> Outcome {
    let project_dir = projects_dir().join(name);
    let installed = home().join(".cargo/bin").join(name);
    let deployed = bin_dir().join(name);

    // --- refusals, checked before anything is built ---

    if let Some(target) = link_escaping_bin_dir(&deployed) {
        return Outcome {
            project: name.to_string(),
            result: "refused".to_string(),
            detail: format!(
                "~/.local/bin/{} is a symlink to {} — that is someone else's artifact, \
                 and deploying onto it would overwrite the target, not the link",
                name,
                target.display()
            ),
        };
    }

    let running = processes_running(&deployed);
    let unit = active_user_unit(name);
    if !running.is_empty() && unit.is_none() {
        return Outcome {
            project: name.to_string(),
            result: "refused".to_string(),
            detail: format!(
                "running as PID(s) {} with no systemd --user unit to cycle — stop it yourself, \
                 or on macOS use the atomic-swap recipe (sign a copy, then rename it into place)",
                running.join(", ")
            ),
        };
    }

    let signed_tool = cfg!(target_os = "macos") && MACOS_SIGNED_TOOLS.contains(&name);
    if signed_tool && !running.is_empty() {
        return Outcome {
            project: name.to_string(),
            result: "refused".to_string(),
            detail: format!(
                "{} needs a Developer ID signature and is currently running (PID(s) {}). \
                 `codesign --force` rewrites the file in place and the kernel SIGKILLs the \
                 running process on its next page fault — sign a copy and rename it in",
                name,
                running.join(", ")
            ),
        };
    }

    if cli.dry_run {
        let mut plan = format!("would rebuild and deploy to {}", deployed.display());
        if let Some(u) = &unit {
            plan.push_str(&format!("; would stop/start {}", u));
        }
        if signed_tool {
            plan.push_str("; would re-sign with Developer ID");
        }
        return Outcome {
            project: name.to_string(),
            result: "planned".to_string(),
            detail: plan,
        };
    }

    // --- build ---

    let build = Command::new("cargo")
        .args(["install", "--path", "."])
        .current_dir(&project_dir)
        .env("CARGO_BUILD_JOBS", cli.jobs.to_string())
        .status();
    match build {
        Ok(s) if s.success() => {}
        Ok(s) => {
            return Outcome {
                project: name.to_string(),
                result: "build-failed".to_string(),
                detail: format!("cargo install exited {}", s),
            }
        }
        Err(e) => {
            return Outcome {
                project: name.to_string(),
                result: "build-failed".to_string(),
                detail: format!("could not run cargo: {}", e),
            }
        }
    }

    // --- deploy ---

    // Rollback copy. Only for a regular file: a symlink was refused above, and copying a
    // dangling one would fail for no benefit.
    if deployed.is_file() {
        let prev = bin_dir().join(format!("{}.prev", name));
        if let Err(e) = std::fs::copy(&deployed, &prev) {
            return Outcome {
                project: name.to_string(),
                result: "failed".to_string(),
                detail: format!("could not write rollback copy {}: {}", prev.display(), e),
            };
        }
    }

    // A running service holds its binary open: `cp` onto it fails with "Text file busy",
    // and a restart afterwards silently relaunches the OLD binary. Stop, deploy, start.
    if let Some(u) = &unit {
        if let Err(e) = systemctl("stop", u) {
            return Outcome {
                project: name.to_string(),
                result: "failed".to_string(),
                detail: format!("{}", e),
            };
        }
    }

    let deploy_result = deploy_file(&installed, &deployed);

    if let Some(u) = &unit {
        if let Err(e) = systemctl("start", u) {
            return Outcome {
                project: name.to_string(),
                result: "failed".to_string(),
                detail: format!("deployed, but could not restart {}: {}", u, e),
            };
        }
    }

    if let Err(e) = deploy_result {
        return Outcome {
            project: name.to_string(),
            result: "failed".to_string(),
            detail: format!("{:#}", e),
        };
    }

    if signed_tool {
        if let Err(e) = codesign(&deployed) {
            return Outcome {
                project: name.to_string(),
                result: "failed".to_string(),
                detail: format!("deployed but signing failed — do NOT run it: {:#}", e),
            };
        }
    }

    // --- verify ---

    // Hash equality only. There is deliberately no smoke test: `--help` is not universal,
    // and `ai-export-watcher --help` actually STARTS the watcher rather than printing usage.
    // Verifying a binary by running it is not safe for this population.
    //
    // A signed tool is exempt: codesign rewrites the file, so the deployed copy legitimately
    // no longer matches ~/.cargo/bin. `codesign --verify --strict` above is its check.
    if !signed_tool {
        match (sha256_of(&installed), sha256_of(&deployed)) {
            (Ok(a), Ok(b)) if a == b => {}
            (Ok(a), Ok(b)) => {
                return Outcome {
                    project: name.to_string(),
                    result: "failed".to_string(),
                    detail: format!("hash mismatch after deploy: {} vs {}", &a[..12], &b[..12]),
                }
            }
            _ => {
                return Outcome {
                    project: name.to_string(),
                    result: "failed".to_string(),
                    detail: "could not hash binaries to verify the deploy".to_string(),
                }
            }
        }
    }

    let mut detail = format!("deployed to {}", deployed.display());
    if let Some(u) = &unit {
        detail.push_str(&format!("; {} restarted", u));
    }
    if signed_tool {
        detail.push_str("; Developer ID signature applied and verified");
    }

    Outcome {
        project: name.to_string(),
        result: "ok".to_string(),
        detail,
    }
}

fn main() {
    let cli = Cli::parse();

    let names = match select_projects(&cli) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("error: {:#}", e);
            std::process::exit(2);
        }
    };

    if names.is_empty() {
        if cli.json {
            println!("[]");
        } else {
            println!("Nothing stale — no binaries to redeploy.");
        }
        return;
    }

    if !cli.json {
        let verb = if cli.dry_run { "Would process" } else { "Processing" };
        println!("{} {} project(s): {}\n", verb, names.len(), names.join(", "));
    }

    let outcomes: Vec<Outcome> = names
        .iter()
        .map(|name| {
            let outcome = redeploy_one(name, &cli);
            if !cli.json {
                let icon = match outcome.result.as_str() {
                    "ok" => "✅",
                    "planned" => "· ",
                    "refused" => "⛔",
                    _ => "❌",
                };
                println!("{} {} — {}", icon, outcome.project, outcome.detail);
            }
            outcome
        })
        .collect();

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&outcomes).unwrap_or_default());
    } else {
        let failed = outcomes.iter().filter(|o| o.result.contains("fail")).count();
        let refused = outcomes.iter().filter(|o| o.result == "refused").count();
        let ok = outcomes.iter().filter(|o| o.result == "ok").count();
        println!("\n{} deployed, {} refused, {} failed", ok, refused, failed);
    }

    // Refusals are not failures — they are the tool declining to do damage, and they are
    // expected steady state (forge-metadata-backup refuses on every run). Only real
    // failures set a non-zero exit.
    if outcomes.iter().any(|o| o.result.contains("fail")) {
        std::process::exit(1);
    }
}
