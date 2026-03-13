use anyhow::{Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Status {
    Clean,
    Drift,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub name: String,
    pub status: Status,
    pub details: Vec<String>,
}

fn home_dir() -> PathBuf {
    dirs::home_dir().expect("cannot determine home directory")
}

fn ssh_cmd(remote: &str, cmd: &str) -> Result<String> {
    // Use bash -c with single-quote wrapping; escape any inner single quotes
    let escaped = cmd.replace('\'', "'\\''");
    let output = Command::new("ssh")
        .args(["-o", "ConnectTimeout=5", "-o", "BatchMode=yes", remote])
        .arg(format!("bash -c '{}'", escaped))
        .output()
        .context("failed to run ssh")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ssh command failed: {}", stderr.trim());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// --- 1. Dotfiles git sync ---

pub fn dotfiles_uncommitted() -> Result<CheckResult> {
    let dotfiles = home_dir().join("dotfiles");
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&dotfiles)
        .output()
        .context("failed to run git status")?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if stdout.is_empty() {
        Ok(CheckResult {
            name: "dotfiles-local-uncommitted".to_string(),
            status: Status::Clean,
            details: vec![],
        })
    } else {
        let files: Vec<String> = stdout.lines().map(|l| l.to_string()).collect();
        Ok(CheckResult {
            name: "dotfiles-local-uncommitted".to_string(),
            status: Status::Drift,
            details: files,
        })
    }
}

pub fn dotfiles_remote_sync(remote: &str) -> Result<CheckResult> {
    let dotfiles = home_dir().join("dotfiles");

    // Get local HEAD
    let local_head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&dotfiles)
        .output()
        .context("failed to get local git HEAD")?;
    let local_head = String::from_utf8_lossy(&local_head.stdout).trim().to_string();

    // Get remote HEAD
    let remote_head = match ssh_cmd(remote, "cd ~/dotfiles && git rev-parse HEAD") {
        Ok(h) => h,
        Err(e) => {
            return Ok(CheckResult {
                name: "dotfiles-remote-sync".to_string(),
                status: Status::Skipped,
                details: vec![format!("SSH failed: {}", e)],
            });
        }
    };

    // Check remote uncommitted
    let remote_status = ssh_cmd(remote, "cd ~/dotfiles && git status --porcelain")
        .unwrap_or_default();

    let mut details = Vec::new();
    let mut has_drift = false;

    if local_head != remote_head {
        has_drift = true;
        details.push(format!("local HEAD: {}", &local_head[..8]));
        details.push(format!("remote HEAD: {}", &remote_head[..std::cmp::min(8, remote_head.len())]));
    }

    if !remote_status.is_empty() {
        has_drift = true;
        details.push("remote has uncommitted changes:".to_string());
        for line in remote_status.lines() {
            details.push(format!("  {}", line));
        }
    }

    Ok(CheckResult {
        name: "dotfiles-remote-sync".to_string(),
        status: if has_drift { Status::Drift } else { Status::Clean },
        details,
    })
}

// --- 2. Rust binary freshness ---

fn newest_source_mtime(project_dir: &Path) -> Result<Option<i64>> {
    let src_dir = project_dir.join("src");
    if !src_dir.exists() {
        return Ok(None);
    }

    let mut newest: Option<i64> = None;
    for entry in std::fs::read_dir(&src_dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            let mtime = meta
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs() as i64;
            newest = Some(newest.map_or(mtime, |n: i64| n.max(mtime)));
        }
    }

    // Also check Cargo.toml
    let cargo_toml = project_dir.join("Cargo.toml");
    if cargo_toml.exists() {
        let meta = std::fs::metadata(&cargo_toml)?;
        let mtime = meta
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        newest = Some(newest.map_or(mtime, |n: i64| n.max(mtime)));
    }

    Ok(newest)
}

pub fn rust_binary_freshness() -> Result<Vec<CheckResult>> {
    let rust_projects = home_dir().join("dotfiles/rust-projects");
    let bin_dir = home_dir().join(".local/bin");
    let mut results = Vec::new();

    if !rust_projects.exists() {
        return Ok(results);
    }

    let mut entries: Vec<_> = std::fs::read_dir(&rust_projects)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("Cargo.toml").exists())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let project_name = entry.file_name().to_string_lossy().to_string();
        let project_dir = entry.path();

        let source_mtime = match newest_source_mtime(&project_dir)? {
            Some(t) => t,
            None => continue,
        };

        let binary_path = bin_dir.join(&project_name);
        if !binary_path.exists() {
            // Not deployed — intentional for one-off/migration tools, skip silently
            continue;
        }

        let bin_meta = std::fs::metadata(&binary_path)?;
        let bin_mtime = bin_meta
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;

        if source_mtime > bin_mtime {
            let stale_days = (source_mtime - bin_mtime) / 86400;
            results.push(CheckResult {
                name: format!("rust-binary/{}", project_name),
                status: Status::Drift,
                details: vec![format!(
                    "source newer than binary by {} day(s)",
                    stale_days.max(1)
                )],
            });
        }
    }

    // Only report if there's drift — don't clutter output with N clean binaries
    if results.is_empty() {
        results.push(CheckResult {
            name: "rust-binaries-local".to_string(),
            status: Status::Clean,
            details: vec![],
        });
    }

    Ok(results)
}

pub fn rust_binary_freshness_remote(remote: &str) -> Result<Vec<CheckResult>> {
    // Get list of projects and their newest source mtime
    let projects_output = match ssh_cmd(
        remote,
        concat!(
            "for d in ~/dotfiles/rust-projects/*/; do ",
            "  name=$(basename \"$d\"); ",
            "  if [ -f \"$d/Cargo.toml\" ]; then ",
            "    src_time=$(find \"$d/src\" \"$d/Cargo.toml\" -type f -printf '%T@\\n' 2>/dev/null | sort -rn | head -1); ",
            "    bin_time=$(stat -c '%Y' ~/.local/bin/$name 2>/dev/null || echo 0); ",
            "    echo \"$name|$src_time|$bin_time\"; ",
            "  fi; ",
            "done"
        ),
    ) {
        Ok(o) => o,
        Err(e) => {
            return Ok(vec![CheckResult {
                name: "rust-binaries-remote".to_string(),
                status: Status::Skipped,
                details: vec![format!("SSH failed: {}", e)],
            }]);
        }
    };

    let mut results = Vec::new();

    for line in projects_output.lines() {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() != 3 {
            continue;
        }
        let name = parts[0];
        let src_time: f64 = parts[1].parse().unwrap_or(0.0);
        let bin_time: f64 = parts[2].parse().unwrap_or(0.0);

        if bin_time == 0.0 {
            // Not deployed on remote — intentional for one-off tools, skip
            continue;
        } else if src_time > bin_time {
            let stale_days = ((src_time - bin_time) / 86400.0) as i64;
            results.push(CheckResult {
                name: format!("rust-binary-remote/{}", name),
                status: Status::Drift,
                details: vec![format!(
                    "source newer than binary by {} day(s) on remote",
                    stale_days.max(1)
                )],
            });
        }
    }

    if results.is_empty() {
        results.push(CheckResult {
            name: "rust-binaries-remote".to_string(),
            status: Status::Clean,
            details: vec![],
        });
    }

    Ok(results)
}

// --- 3. Skill file parity ---

pub fn skill_parity(remote: &str) -> Result<Vec<CheckResult>> {
    let skills_dir = home_dir().join(".claude/skills");
    if !skills_dir.exists() {
        return Ok(vec![CheckResult {
            name: "skill-parity".to_string(),
            status: Status::Skipped,
            details: vec!["~/.claude/skills/ not found locally".to_string()],
        }]);
    }

    // Get all .md files in skills dirs locally (follow symlinks)
    let local_output = Command::new("fd")
        .args([
            "-e", "md", "-t", "f", "-L", ".",
            skills_dir.to_str().unwrap(),
        ])
        .output()?;
    let local_files: Vec<String> = String::from_utf8_lossy(&local_output.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect();

    if local_files.is_empty() {
        return Ok(vec![CheckResult {
            name: "skill-parity".to_string(),
            status: Status::Skipped,
            details: vec!["no skill .md files found".to_string()],
        }]);
    }

    // Build a map of relative path -> sha256 for local
    let mut local_hashes = std::collections::HashMap::new();
    for file in &local_files {
        let rel = file
            .strip_prefix(skills_dir.to_str().unwrap())
            .unwrap_or(file)
            .trim_start_matches('/');
        let output = Command::new("shasum")
            .args(["-a", "256"])
            .arg(file)
            .output();
        if let Ok(o) = output {
            let line = String::from_utf8_lossy(&o.stdout);
            if let Some(hash) = line.split_whitespace().next() {
                local_hashes.insert(rel.to_string(), hash.to_string());
            }
        }
    }

    // Get remote hashes (shasum works on both macOS and Linux)
    let remote_output = match ssh_cmd(
        remote,
        "fd -e md -t f . $HOME/.claude/skills -L | xargs sha256sum 2>/dev/null || fd -e md -t f . $HOME/.claude/skills -L | xargs shasum -a 256",
    ) {
        Ok(o) => o,
        Err(e) => {
            return Ok(vec![CheckResult {
                name: "skill-parity".to_string(),
                status: Status::Skipped,
                details: vec![format!("SSH failed: {}", e)],
            }]);
        }
    };

    let mut remote_hashes = std::collections::HashMap::new();
    for line in remote_output.lines() {
        // shasum output: "hash  path" (two spaces)
        let mut parts = line.splitn(2, char::is_whitespace);
        let hash = parts.next().unwrap_or("").trim();
        let path = parts.next().unwrap_or("").trim();
        if !hash.is_empty() && !path.is_empty() {
            // Extract relative path after .claude/skills/
            if let Some(idx) = path.find(".claude/skills/") {
                let rel = &path[idx + ".claude/skills/".len()..];
                remote_hashes.insert(rel.to_string(), hash.to_string());
            }
        }
    }

    let mut drifted = Vec::new();

    // Compare local -> remote
    for (rel, local_hash) in &local_hashes {
        match remote_hashes.get(rel) {
            Some(remote_hash) if remote_hash != local_hash => {
                drifted.push(format!("{} — hash mismatch", rel));
            }
            None => {
                drifted.push(format!("{} — missing on remote", rel));
            }
            _ => {}
        }
    }

    // Check for remote-only files
    for rel in remote_hashes.keys() {
        if !local_hashes.contains_key(rel) {
            drifted.push(format!("{} — only on remote", rel));
        }
    }

    if drifted.is_empty() {
        Ok(vec![CheckResult {
            name: "skill-parity".to_string(),
            status: Status::Clean,
            details: vec![],
        }])
    } else {
        Ok(vec![CheckResult {
            name: "skill-parity".to_string(),
            status: Status::Drift,
            details: drifted,
        }])
    }
}

// --- 4. Messageboard staleness ---

pub fn messageboard_staleness() -> Result<CheckResult> {
    let messageboard = home_dir().join("Assistants/shared/MESSAGEBOARD.md");

    if !messageboard.exists() {
        return Ok(CheckResult {
            name: "messageboard".to_string(),
            status: Status::Skipped,
            details: vec!["MESSAGEBOARD.md not found".to_string()],
        });
    }

    let content = std::fs::read_to_string(&messageboard)?;

    // Check if there are any dated message headers
    let today = chrono::Local::now().date_naive();
    let mut stale_messages = Vec::new();

    for line in content.lines() {
        if line.starts_with("### ") {
            // Try to parse date from "### YYYY-MM-DD — device"
            let date_part = line
                .trim_start_matches("### ")
                .split(|c: char| c == ' ' || c == '\u{2014}' || c == '-')
                .take(3)
                .collect::<Vec<&str>>();

            if date_part.len() >= 3 {
                let date_str = format!("{}-{}-{}", date_part[0], date_part[1], date_part[2]);
                if let Ok(date) = chrono::NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
                    let age = today - date;
                    if age.num_days() > 7 {
                        stale_messages
                            .push(format!("{} ({} days old)", line.trim(), age.num_days()));
                    }
                }
            }
        }
    }

    // Also check if there are any messages at all (non-empty)
    let has_messages = content.contains("### ") && !content.contains("*(No messages)*");

    if !has_messages {
        Ok(CheckResult {
            name: "messageboard".to_string(),
            status: Status::Clean,
            details: vec![],
        })
    } else if !stale_messages.is_empty() {
        Ok(CheckResult {
            name: "messageboard".to_string(),
            status: Status::Drift,
            details: stale_messages,
        })
    } else {
        Ok(CheckResult {
            name: "messageboard".to_string(),
            status: Status::Drift,
            details: vec!["has pending messages".to_string()],
        })
    }
}
