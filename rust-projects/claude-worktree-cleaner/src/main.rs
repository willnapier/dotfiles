mod exec;
use anyhow::{bail, Context, Result};
use clap::Parser;
use exec::{Exec, Real};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[arg(long)]
    apply: bool,
    #[arg(long, default_value_t = 7)]
    age_days: u64,
    /// cargo-sweep keeps layers used within this many days. 30 let `~/Code/practiceforge/target`
    /// reach 182 GB by 2026-09-23 (disk 96 %); the cleaner runs daily, so a week is the ceiling now.
    #[arg(long, default_value_t = 7)]
    sweep_days: u64,
    #[arg(long,default_value_t=85,value_parser=clap::value_parser!(u8).range(1..=100))]
    disk_warn_pct: u8,
}
fn log(home: &Path, msg: &str) -> Result<()> {
    let p = home.join(".local/share/claude-worktree-cleaner.log");
    fs::create_dir_all(p.parent().unwrap())?;
    let line = format!("{} {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    println!("{line}");
    writeln!(
        OpenOptions::new().create(true).append(true).open(p)?,
        "{line}"
    )?;
    Ok(())
}
fn git(ex: &dyn Exec, repo: &Path, args: &[&str]) -> exec::CmdResult {
    let mut all = vec!["-C", repo.to_str().unwrap_or("")];
    all.extend(args);
    ex.run("git", &all)
}
fn repo_dirs(home: &Path) -> Result<Vec<PathBuf>> {
    let mut roots = vec![home.to_path_buf()];
    for sub in ["Code", "projects"] {
        let p = home.join(sub);
        if p.is_dir() {
            roots.push(p)
        }
    }
    let mut out = Vec::new();
    for root in roots {
        for entry in fs::read_dir(root)? {
            let e = entry?;
            if e.file_type()?.is_dir() && e.path().join(".claude/worktrees").is_dir() {
                out.push(e.path())
            }
        }
    }
    Ok(out)
}
fn worktrees(data: &str, repo: &Path) -> Result<Vec<(PathBuf, bool)>> {
    let mut result = Vec::new();
    let mut current: Option<(PathBuf, bool)> = None;
    // -z avoids ambiguous quoted/newline-containing paths.
    for field in data.split('\0') {
        if let Some(path) = field.strip_prefix("worktree ") {
            if let Some(c) = current.take() {
                result.push(c)
            };
            let p = PathBuf::from(path);
            current = p
                .starts_with(repo.join(".claude/worktrees"))
                .then_some((p, false));
        } else if field == "locked" || field.starts_with("locked ") {
            if let Some((_, locked)) = current.as_mut() {
                *locked = true
            }
        }
    }
    if let Some(c) = current {
        result.push(c)
    }
    if !data.starts_with("worktree ") {
        bail!("git worktree list returned no valid records")
    }
    Ok(result)
}
fn newest(wt: &Path) -> Result<SystemTime> {
    let mut newest = fs::metadata(wt)?.modified()?;
    for e in walkdir::WalkDir::new(wt)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || (e.file_name() != ".git" && e.file_name() != "target"))
    {
        let e = e?;
        let m = fs::symlink_metadata(e.path())?.modified()?;
        newest = newest.max(m);
    }
    Ok(newest)
}
fn eligible(ex: &dyn Exec, wt: &Path, cutoff: SystemTime) -> Result<Option<&'static str>> {
    let r = git(ex, wt, &["status", "--porcelain", "--untracked-files=all"]);
    if !r.ok() {
        bail!("git status failed")
    };
    if !r.out().is_empty() {
        return Ok(Some("dirty"));
    }
    // Clean does not mean saved elsewhere: preserve detached/local-only commits.
    let r = git(ex, wt, &["rev-list", "HEAD", "--not", "--remotes"]);
    if !r.ok() {
        bail!("cannot prove commits are on a remote")
    };
    if !r.out().is_empty() {
        return Ok(Some("unpublished commits"));
    }
    let r = git(ex, wt, &["reflog", "-1", "--format=%ct"]);
    if !r.ok() {
        bail!("cannot inspect worktree activity")
    };
    let secs = r
        .out()
        .parse::<u64>()
        .context("missing or invalid reflog timestamp")?;
    if SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(secs))
        .context("invalid reflog time")?
        > cutoff
        || newest(wt)? > cutoff
    {
        return Ok(Some("recent"));
    }
    Ok(None)
}
fn disk_pct(text: &str) -> Result<u8> {
    let last = text
        .lines()
        .filter(|s| !s.trim().is_empty())
        .next_back()
        .context("empty df output")?;
    let value = last
        .split_whitespace()
        .find_map(|s| s.strip_suffix('%'))
        .context("no disk percentage")?
        .parse::<u8>()?;
    if value > 100 {
        bail!("invalid disk percentage")
    };
    Ok(value)
}
fn run(home: &Path, c: &Cli, ex: &dyn Exec) -> Result<()> {
    log(
        home,
        &format!(
            "=== claude-worktree-cleaner start [{}] ===",
            if c.apply { "APPLY" } else { "DRY-RUN" }
        ),
    )?;
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(
            c.age_days.checked_mul(86400).context("age too large")?,
        ))
        .context("age too large")?;
    let mut removed = 0;
    let mut skipped = 0;
    let mut errors = 0;
    let mut examined = 0;
    for repo in repo_dirs(home)? {
        let r = git(ex, &repo, &["worktree", "list", "--porcelain", "-z"]);
        let wts = if r.ok() {
            worktrees(&r.stdout, &repo)
        } else {
            Err(anyhow::anyhow!("git worktree list failed"))
        };
        let wts = match wts {
            Ok(v) => v,
            Err(e) => {
                errors += 1;
                log(home, &format!("ERROR {}: {e}", repo.display()))?;
                continue;
            }
        };
        for (wt, locked) in wts {
            examined += 1;
            if !wt.exists() {
                skipped += 1;
                log(home, &format!("SKIP missing {}", wt.display()))?;
                continue;
            }
            match eligible(ex, &wt, cutoff) {
                Ok(Some(why)) => {
                    skipped += 1;
                    log(home, &format!("SKIP {why} {}", wt.display()))?;
                    continue;
                }
                Err(e) => {
                    errors += 1;
                    log(home, &format!("ERROR {}: {e}", wt.display()))?;
                    continue;
                }
                Ok(None) => {}
            }
            if !c.apply {
                removed += 1;
                log(home, &format!("WOULD-REMOVE {}", wt.display()))?;
                continue;
            }
            if locked
                && !git(
                    ex,
                    &repo,
                    &["worktree", "unlock", wt.to_str().context("worktree path")?],
                )
                .ok()
            {
                errors += 1;
                log(home, "ERROR unlocking eligible worktree")?;
                continue;
            }
            // No --force: Git gets a final chance to reject newly dirtied trees.
            let r = git(
                ex,
                &repo,
                &["worktree", "remove", wt.to_str().context("worktree path")?],
            );
            if r.ok() {
                removed += 1;
                log(home, &format!("REMOVED {}", wt.display()))?
            } else {
                errors += 1;
                if locked {
                    let _ = git(ex, &repo, &["worktree", "lock", wt.to_str().unwrap()]);
                }
                log(home, &format!("REMOVE-FAILED {}", wt.display()))?
            }
        }
    }
    log(
        home,
        &format!(
            "worktrees: examined={examined} {}={removed} skipped={skipped} errors={errors}",
            if c.apply { "removed" } else { "would-remove" }
        ),
    )?;
    let sweep = ex.run("cargo-sweep", &["--version"]);
    if sweep.exit_code == 127 {
        log(
            home,
            "cargo-sweep not installed — skipping build-artifact sweep",
        )?
    } else if !sweep.ok() {
        errors += 1;
        log(home, "ERROR cargo-sweep preflight failed")?
    } else {
        for root in [
            home.join("Code"),
            home.join("dotfiles/rust-projects"),
            home.join("rust-learning"),
        ] {
            if !root.is_dir() {
                continue;
            }
            let days = c.sweep_days.to_string();
            let mut args = vec!["sweep"];
            if !c.apply {
                args.push("--dry-run")
            }
            args.extend([
                "--recursive",
                "--time",
                &days,
                root.to_str().context("sweep root")?,
            ]);
            let r = ex.run("cargo-sweep", &args);
            log(
                home,
                &format!("cargo-sweep {} exit={}", root.display(), r.exit_code),
            )?;
            if !r.ok() {
                errors += 1;
            }
        }
    }
    let r = ex.run(
        "df",
        &[
            "-P",
            if cfg!(target_os = "macos") {
                "/System/Volumes/Data"
            } else {
                "/"
            },
        ],
    );
    let disk = if r.ok() {
        disk_pct(&r.stdout)
    } else {
        Err(anyhow::anyhow!("df failed"))
    };
    match disk {
        Ok(used) if used < c.disk_warn_pct => log(home, &format!("disk OK ({used}%)"))?,
        Ok(used) => {
            errors += 1;
            log(home, &format!("WARN disk {used}% >= {}%", c.disk_warn_pct))?
        }
        Err(e) => {
            errors += 1;
            log(home, &format!("ERROR disk check: {e}"))?
        }
    }
    log(
        home,
        &format!(
            "outcome={} errors={errors} === claude-worktree-cleaner end ===",
            if errors == 0 { "success" } else { "error" }
        ),
    )?;
    if errors > 0 {
        bail!("{errors} cleanup/disk checks failed; see claude-worktree-cleaner.log")
    };
    Ok(())
}
fn main() -> std::process::ExitCode {
    let c = Cli::parse();
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        eprintln!("HOME not set");
        return 1.into();
    };
    match run(&home, &c, &Real) {
        Ok(()) => 0.into(),
        Err(e) => {
            let _ = log(&home, &format!("outcome=error {e:#}"));
            let _ = Real.run(
                home.join(".local/bin/notify-user").to_str().unwrap(),
                &[
                    "--tool",
                    "claude-worktree-cleaner",
                    "Worktree cleanup",
                    &format!("{e:#}"),
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
    fn disk_red_green_and_malformed() {
        assert_eq!(
            disk_pct(
                "Filesystem 1024-blocks Used Available Capacity Mounted\n/dev/a 100 80 20 80% /\n"
            )
            .unwrap(),
            80
        );
        assert_eq!(disk_pct("/dev/a 100 99 1 99% /").unwrap(), 99);
        assert!(disk_pct("").is_err());
        assert!(disk_pct("/dev/a 100 5 95 unknown /").is_err())
    }
    fn fake_status(wt: &Path, status: exec::CmdResult, unpushed: &str) -> Fake {
        let mut f = Fake::default();
        f.respond(
            "git",
            &[
                "-C",
                wt.to_str().unwrap(),
                "status",
                "--porcelain",
                "--untracked-files=all",
            ],
            status,
        );
        f.respond(
            "git",
            &[
                "-C",
                wt.to_str().unwrap(),
                "rev-list",
                "HEAD",
                "--not",
                "--remotes",
            ],
            CmdResult::success(unpushed),
        );
        f.respond(
            "git",
            &["-C", wt.to_str().unwrap(), "reflog", "-1", "--format=%ct"],
            CmdResult::success("1"),
        );
        f
    }
    #[test]
    fn clean_old_green_dirty_error_and_unpublished_red() {
        let t = tempfile::tempdir().unwrap();
        let cutoff = SystemTime::now() + Duration::from_secs(2);
        assert!(eligible(
            &fake_status(t.path(), CmdResult::success(""), ""),
            t.path(),
            cutoff
        )
        .unwrap()
        .is_none());
        assert_eq!(
            eligible(
                &fake_status(t.path(), CmdResult::success(" M src/main.rs"), ""),
                t.path(),
                cutoff
            )
            .unwrap(),
            Some("dirty")
        );
        assert!(eligible(
            &fake_status(t.path(), CmdResult::failure(128, "fatal"), ""),
            t.path(),
            cutoff
        )
        .is_err());
        assert_eq!(
            eligible(
                &fake_status(t.path(), CmdResult::success(""), "abc123"),
                t.path(),
                cutoff
            )
            .unwrap(),
            Some("unpublished commits")
        );
        assert_eq!(
            eligible(
                &fake_status(t.path(), CmdResult::success(""), ""),
                t.path(),
                SystemTime::UNIX_EPOCH
            )
            .unwrap(),
            Some("recent")
        );
    }
    #[test]
    fn nul_list_limits_scope_and_keeps_lock() {
        let s="worktree /repo\0HEAD abc\0\0worktree /repo/.claude/worktrees/a b\0HEAD abc\0locked agent\0\0worktree /other/.claude/worktrees/a\0HEAD abc\0";
        assert_eq!(
            worktrees(s, Path::new("/repo")).unwrap(),
            vec![(PathBuf::from("/repo/.claude/worktrees/a b"), true)]
        );
        assert!(worktrees("", Path::new("/repo")).is_err());
    }
    #[test]
    fn end_to_end_known_red_and_green_disk() {
        let t = tempfile::tempdir().unwrap();
        let mut f = Fake::default();
        let c = Cli {
            apply: false,
            age_days: 7,
            sweep_days: 7,
            disk_warn_pct: 85,
        };
        let root = if cfg!(target_os = "macos") {
            "/System/Volumes/Data"
        } else {
            "/"
        };
        f.respond(
            "df",
            &["-P", root],
            CmdResult::success("/dev/a 100 30 70 30% /"),
        );
        assert!(run(t.path(), &c, &f).is_ok());
        f.respond(
            "df",
            &["-P", root],
            CmdResult::success("/dev/a 100 99 1 99% /"),
        );
        assert!(run(t.path(), &c, &f).is_err());
    }
    #[test]
    fn removal_failure_is_red_and_dry_run_never_removes() {
        let t = tempfile::tempdir().unwrap();
        let h = t.path();
        let repo = h.join("Code/repo");
        let wt = repo.join(".claude/worktrees/old");
        fs::create_dir_all(&wt).unwrap();
        let mut f = fake_status(&wt, CmdResult::success(""), "");
        f.respond(
            "git",
            &[
                "-C",
                repo.to_str().unwrap(),
                "worktree",
                "list",
                "--porcelain",
                "-z",
            ],
            CmdResult::success(&format!(
                "worktree {}\0HEAD abc\0\0worktree {}\0HEAD abc\0",
                repo.display(),
                wt.display()
            )),
        );
        let root = if cfg!(target_os = "macos") {
            "/System/Volumes/Data"
        } else {
            "/"
        };
        f.respond(
            "df",
            &["-P", root],
            CmdResult::success("/dev/a 100 30 70 30% /"),
        );
        let mut c = Cli {
            apply: false,
            age_days: 0,
            sweep_days: 7,
            disk_warn_pct: 85,
        };
        assert!(run(h, &c, &f).is_ok());
        assert!(!f
            .calls
            .borrow()
            .iter()
            .any(|s| s.contains("worktree remove")));
        c.apply = true;
        assert!(run(h, &c, &f).is_err());
        assert!(wt.is_dir());
        f.respond(
            "git",
            &[
                "-C",
                repo.to_str().unwrap(),
                "worktree",
                "remove",
                wt.to_str().unwrap(),
            ],
            CmdResult::success(""),
        );
        assert!(run(h, &c, &f).is_ok());
    }
}
