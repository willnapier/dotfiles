//! `pageprobe reap` — kill orphaned headless Chrome processes and delete
//! their throwaway profiles. See `orphans.rs` for the selection rule and the
//! 2026-09-11 incident that motivated it.
use anyhow::{Context, Result, bail};
use std::time::Duration;
use tokio::process::Command;

use crate::{chrome, orphans, state};

#[derive(serde::Serialize)]
struct Report {
    pid: u32,
    age_secs: u64,
    user_data_dir: Option<String>,
    action: &'static str,
    profile_removed: bool,
}

pub async fn run(dry_run: bool, json: bool, min_age_secs: u64) -> Result<()> {
    let out = Command::new("ps")
        .args(["-axo", "pid=,ppid=,etime=,command="])
        .output()
        .await
        .context("invoking ps")?;
    if !out.status.success() {
        bail!("ps exited {}", out.status);
    }
    let all = orphans::headless_browsers(&String::from_utf8_lossy(&out.stdout));
    let strays = orphans::strays(&all, min_age_secs);
    let own_profile = state::default_user_data_dir().ok();

    let mut reports = Vec::with_capacity(strays.len());
    for p in &strays {
        let action = if dry_run { "would kill" } else { "killed" };
        if !dry_run {
            chrome::stop(p.pid, Duration::from_secs(3)).await?;
        }
        let mut profile_removed = false;
        if let Some(dir) = &p.user_data_dir
            && !dry_run
            && orphans::is_temp_profile(dir)
            && own_profile.as_deref() != Some(dir.as_path())
            && dir.exists()
        {
            match std::fs::remove_dir_all(dir) {
                Ok(()) => profile_removed = true,
                Err(e) => eprintln!("could not remove {}: {e}", dir.display()),
            }
        }
        reports.push(Report {
            pid: p.pid,
            age_secs: p.age_secs,
            user_data_dir: p.user_data_dir.as_ref().map(|d| d.display().to_string()),
            action,
            profile_removed,
        });
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
        return Ok(());
    }
    if reports.is_empty() {
        let live = all.len();
        if live == 0 {
            println!("no headless Chrome running.");
        } else {
            println!(
                "no stray headless Chrome ({live} running with a live parent or younger than {min_age_secs}s, left alone)."
            );
        }
        return Ok(());
    }
    for r in &reports {
        let profile = match &r.user_data_dir {
            Some(d) if r.profile_removed => format!(" profile {d} [removed]"),
            Some(d) => format!(" profile {d}"),
            None => String::new(),
        };
        println!(
            "{} PID {} (age {}, parent 1){profile}",
            r.action,
            r.pid,
            orphans::human_age(r.age_secs)
        );
    }
    Ok(())
}
