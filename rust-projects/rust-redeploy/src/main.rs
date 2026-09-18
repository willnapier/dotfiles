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
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const EXTERNAL_PROJECTS_FILE: &str = "rust-projects/external-deploy-projects.tsv";

/// Tools that must carry a Developer ID signature on macOS or the kernel SIGKILLs them
/// on launch (`cs_invalid_page`). This list is the source of truth; the runbook
/// `~/Assistants/shared/RUST-REDEPLOY.md` § "macOS signing" mirrors it — keep the two in step
/// (the senior-dev role pack's TECHNICAL-ENVIRONMENT.md that used to hold it is retired).
///
/// Second reason a tool belongs here (found 2026-09-03 on `continuum-dream`): launchd on
/// macOS 26 pins a "managed LWCR" (lightweight code requirement) to some calendar jobs at
/// registration. An ad-hoc rebuild changes the binary's identity, the pin no longer
/// matches, and the next scheduled run is killed at launch with
/// `last exit reason = OS_REASON_CODESIGNING` — silently, before it writes a line.
/// A Developer ID signature gives a stable identity (team id), so the pin keeps matching
/// across rebuilds. Any tool launched by a `StartCalendarInterval` job should be here.
const MACOS_SIGNED_TOOLS: &[&str] = &[
    "practiceforge",
    "pizauth",
    "tm3-diary-capture",
    "mailcurator",
    "continuum-claude",
    "continuum-grok",
    "dev-catchup",
    "continuum-dream",
    // Every remaining Rust tool launched by a StartCalendarInterval job (2026-09-13).
    // launchd pins a managed LWCR to these; an ad-hoc rebuild breaks the pin and the
    // job dies at spawn with exit 78 EX_CONFIG before it can write a log line —
    // `readwise-sync` failed 17 nights in a row that way after its 2026-09-08 rebuild.
    "bequest",
    "cross-machine-sync-check",
    "dotter-drift-monitor",
    "readwise-sync",
    "state-capture",
    "system-health-check",
];

const MACOS_SIGNING_IDENTITY: &str = "Developer ID Application: William Napier (LU3TB2NLTD)";

#[derive(Parser)]
#[command(
    name = "rust-redeploy",
    about = "Rebuild and redeploy registered Rust binaries reported stale on this machine"
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct Project {
    name: String,
    build_dir: PathBuf,
}

fn home() -> PathBuf {
    dirs::home_dir().expect("cannot determine home directory")
}

fn bin_dir() -> PathBuf {
    home().join(".local/bin")
}

fn safe_home_relative(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path.components().all(|part| {
            matches!(
                part,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
}

fn parse_external_projects(contents: &str, home: &Path) -> Result<Vec<Project>> {
    let mut projects = Vec::new();
    for (index, raw) in contents.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = raw.split('\t').collect();
        if fields.len() != 3 {
            bail!(
                "{} line {}: expected three tab-separated fields",
                EXTERNAL_PROJECTS_FILE,
                index + 1
            );
        }
        let name = fields[0].trim();
        let build_path = fields[1].trim();
        let freshness_path = fields[2].trim();
        if name.is_empty() || name.contains('/') || name.contains('\\') {
            bail!(
                "{} line {}: invalid binary name",
                EXTERNAL_PROJECTS_FILE,
                index + 1
            );
        }
        if !safe_home_relative(build_path) || !safe_home_relative(freshness_path) {
            bail!(
                "{} line {}: paths must stay beneath HOME",
                EXTERNAL_PROJECTS_FILE,
                index + 1
            );
        }
        projects.push(Project {
            name: name.to_string(),
            build_dir: home.join(build_path),
        });
    }
    Ok(projects)
}

fn discover_projects_at(home: &Path) -> Result<BTreeMap<String, Project>> {
    let rust_projects = home.join("dotfiles/rust-projects");
    let mut projects = BTreeMap::new();
    for entry in std::fs::read_dir(&rust_projects)
        .with_context(|| format!("cannot read {}", rust_projects.display()))?
        .flatten()
    {
        let path = entry.path();
        if !path.join("Cargo.toml").exists() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        projects.insert(
            name.clone(),
            Project {
                name,
                build_dir: path,
            },
        );
    }

    let registry = home.join("dotfiles").join(EXTERNAL_PROJECTS_FILE);
    let contents = std::fs::read_to_string(&registry).with_context(|| {
        format!(
            "cannot read external project registry {}",
            registry.display()
        )
    })?;
    for project in parse_external_projects(&contents, home)? {
        if projects
            .insert(project.name.clone(), project.clone())
            .is_some()
        {
            bail!("duplicate deployment project name: {}", project.name);
        }
    }
    Ok(projects)
}

fn discover_projects() -> Result<BTreeMap<String, Project>> {
    discover_projects_at(&home())
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
fn select_projects(cli: &Cli, projects: &BTreeMap<String, Project>) -> Result<Vec<String>> {
    let mut names: Vec<String> = if cli.all {
        projects
            .keys()
            .filter(|name| bin_dir().join(name).exists())
            .cloned()
            .collect()
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

fn redeploy_one(project: &Project, cli: &Cli) -> Outcome {
    let name = &project.name;
    let project_dir = &project.build_dir;
    let installed = home().join(".cargo/bin").join(name);
    let deployed = bin_dir().join(name);

    if !project_dir.join("Cargo.toml").exists() {
        return Outcome {
            project: name.to_string(),
            result: "failed".to_string(),
            detail: format!("registered source is missing: {}", project_dir.display()),
        };
    }

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

    let signed_tool = cfg!(target_os = "macos") && MACOS_SIGNED_TOOLS.contains(&name.as_str());
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
        let mut plan = format!(
            "would rebuild from {} and deploy to {}",
            project_dir.display(),
            deployed.display()
        );
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

    // `--locked`: build exactly the committed Cargo.lock. Without it `cargo install`
    // re-resolves dependencies and takes the newest release of each crate, so a
    // dependency publishing with a raised MSRV breaks the rebuild on whichever host
    // has the older toolchain — found 2026-09-03 when tinyvec 1.13.0 (lock says
    // 1.12.0) failed to compile on nimbini's rustc 1.90. Every project tracks its
    // Cargo.lock, so this is a tightening, not a new requirement.
    let build = Command::new("cargo")
        .args(["install", "--locked", "--path", "."])
        .current_dir(project_dir)
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

    let projects = match discover_projects() {
        Ok(projects) => projects,
        Err(e) => {
            eprintln!("error: {:#}", e);
            std::process::exit(2);
        }
    };

    let names = match select_projects(&cli, &projects) {
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
            let outcome = match projects.get(name) {
                Some(project) => redeploy_one(project, &cli),
                None => Outcome {
                    project: name.clone(),
                    result: "failed".to_string(),
                    detail: "freshness report named an unregistered project".to_string(),
                },
            };
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn external_registry_parses_canonical_code_paths() {
        let home = Path::new("/home/tester");
        let projects = parse_external_projects(
            "practiceforge\tCode/practiceforge/practiceforge\tCode/practiceforge\n\
             tm3-diary-capture\tCode/tm3-diary-capture\tCode/tm3-diary-capture\n",
            home,
        )
        .unwrap();
        assert_eq!(projects.len(), 2);
        assert_eq!(
            projects[0].build_dir,
            home.join("Code/practiceforge/practiceforge")
        );
        assert_eq!(projects[1].build_dir, home.join("Code/tm3-diary-capture"));
    }

    #[test]
    fn external_registry_rejects_paths_outside_home() {
        let error = parse_external_projects("x\t../outside\tCode/x\n", Path::new("/tmp/home"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("beneath HOME"), "{error}");
    }

    #[test]
    fn discovery_combines_dotfiles_and_explicit_code_projects() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        fs::create_dir_all(home.join("dotfiles/rust-projects/local/src")).unwrap();
        fs::write(
            home.join("dotfiles/rust-projects/local/Cargo.toml"),
            "[package]\nname='local'\nversion='0.1.0'\n",
        )
        .unwrap();
        fs::create_dir_all(home.join("Code/external/src")).unwrap();
        fs::write(
            home.join("Code/external/Cargo.toml"),
            "[package]\nname='external'\nversion='0.1.0'\n",
        )
        .unwrap();
        fs::write(
            home.join("dotfiles/rust-projects/external-deploy-projects.tsv"),
            "external\tCode/external\tCode/external\n",
        )
        .unwrap();

        let projects = discover_projects_at(home).unwrap();
        assert_eq!(projects.len(), 2, "{projects:?}");
        assert_eq!(
            projects["local"].build_dir,
            home.join("dotfiles/rust-projects/local")
        );
        assert_eq!(projects["external"].build_dir, home.join("Code/external"));
    }
}
