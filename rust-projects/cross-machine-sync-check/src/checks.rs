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

/// Newest mtime among every file under `dir`, recursively.
///
/// Recursion is load-bearing, not tidiness. This used to read only the top level of
/// `src/`, which on 2026-07-31 meant it was blind to most of the source it was supposed
/// to be watching: `fd-budget` keeps 19 files with 3 at the top level, `mailcurator`
/// 26 with 18, `pageprobe` 16 with 4, `sr` 11 with 4. An edit to any nested module was
/// invisible, so the binary could be arbitrarily stale and still report clean.
fn newest_mtime_recursive(dir: &Path, newest: &mut Option<i64>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_dir() {
            newest_mtime_recursive(&entry.path(), newest)?;
        } else if meta.is_file() {
            let mtime = meta
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs() as i64;
            *newest = Some(newest.map_or(mtime, |n: i64| n.max(mtime)));
        }
    }
    Ok(())
}

fn newest_source_mtime(project_dir: &Path) -> Result<Option<i64>> {
    let src_dir = project_dir.join("src");
    if !src_dir.exists() {
        return Ok(None);
    }

    let mut newest: Option<i64> = None;
    newest_mtime_recursive(&src_dir, &mut newest)?;

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

/// If `path` is a symlink whose resolved target lies outside `bin_dir`, return that target.
///
/// Returns `None` for a regular file, and also for a symlink that stays inside `bin_dir`
/// (a versioned-name link next to its binary is still this project's artifact).
pub fn resolved_link_outside_bin_dir(path: &Path, bin_dir: &Path) -> Option<PathBuf> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_symlink() {
        return None;
    }
    // Fall back to the raw link text if the target does not resolve — a dangling link
    // pointing out of the directory is still not ours to overwrite.
    let target = std::fs::canonicalize(path).unwrap_or(std::fs::read_link(path).ok()?);
    let bin_dir = std::fs::canonicalize(bin_dir).unwrap_or_else(|_| bin_dir.to_path_buf());
    if target.starts_with(&bin_dir) {
        None
    } else {
        Some(target)
    }
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

        // A same-named script can occupy the binary's deploy path. `forge-metadata-backup`
        // exists both as a Rust project (never deployed) and as ~/dotfiles/scripts/<name>
        // (nushell, the one actually in use), reached by a symlink at ~/.local/bin/<name>.
        // The "not deployed -> skip" test above is just `path exists`, which such a symlink
        // satisfies — so we used to compare the Rust source's mtime against the SCRIPT and
        // report drift for a binary that has never been deployed at all.
        //
        // This is not cosmetic. On 2026-07-31 that false positive was acted on, and the
        // standard deploy step `cp ~/.cargo/bin/<name> ~/.local/bin/` followed the symlink
        // and overwrote the nu script in the dotfiles source tree with a 1.5 MB ELF.
        //
        // A symlink whose target leaves ~/.local/bin is somebody else's artifact. Say so
        // out loud rather than measuring it.
        if let Some(target) = resolved_link_outside_bin_dir(&binary_path, &bin_dir) {
            results.push(CheckResult {
                name: format!("rust-binary/{}", project_name),
                status: Status::Skipped,
                details: vec![
                    format!("~/.local/bin/{} is a symlink to {}", project_name, target.display()),
                    "that target is outside ~/.local/bin, so it is not this project's binary"
                        .to_string(),
                    "freshness not measured; do NOT `cp` onto this path (cp writes through the link)"
                        .to_string(),
                ],
            });
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

    // Only report if there's drift — don't clutter output with N clean binaries.
    // Keyed on absence of Drift, not absence of results: a Skipped entry (symlinked
    // deploy path) must not suppress the "all fresh" summary line.
    if !results.iter().any(|r| r.status == Status::Drift) {
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
    // ⚠️ This probe must be POSIX-portable. It runs on whichever machine is the REMOTE,
    // so from nimbini it executes on macOS, where `find -printf` and `stat -c` do not
    // exist. The original used both. On macOS they failed, `src_time` came back empty,
    // it parsed to 0.0, and `src_time > bin_time` was therefore never true — so
    // **nimbini's remote binary check silently reported "clean" for the Mac no matter
    // what**, which is the worst possible direction for a detector to fail in. It was
    // found on 2026-07-31 only because the Mac's own local run disagreed with it.
    //
    // `stat -c %Y || stat -f %m` covers GNU then BSD: GNU succeeds first, and GNU never
    // reaches `-f` (which would mean something else entirely there).
    //
    // Fields: name|newest_src_mtime|bin_mtime|escaping_symlink_target
    let projects_output = match ssh_cmd(
        remote,
        concat!(
            "for d in \"$HOME\"/dotfiles/rust-projects/*/; do ",
            "  name=$(basename \"$d\"); ",
            "  [ -f \"$d/Cargo.toml\" ] || continue; ",
            "  newest=0; ",
            "  for f in $(find \"$d/src\" -type f 2>/dev/null) \"$d/Cargo.toml\"; do ",
            "    [ -f \"$f\" ] || continue; ",
            "    t=$(stat -c %Y \"$f\" 2>/dev/null || stat -f %m \"$f\" 2>/dev/null || echo 0); ",
            "    [ \"$t\" -gt \"$newest\" ] 2>/dev/null && newest=\"$t\"; ",
            "  done; ",
            "  bin=\"$HOME/.local/bin/$name\"; bin_time=0; link=; ",
            "  if [ -e \"$bin\" ] || [ -L \"$bin\" ]; then ",
            "    bin_time=$(stat -c %Y \"$bin\" 2>/dev/null || stat -f %m \"$bin\" 2>/dev/null || echo 0); ",
            "    if [ -L \"$bin\" ]; then ",
            "      tgt=$(readlink -f \"$bin\" 2>/dev/null || readlink \"$bin\" 2>/dev/null); ",
            "      case \"$tgt\" in \"$HOME/.local/bin/\"*) ;; *) link=\"$tgt\" ;; esac; ",
            "    fi; ",
            "  fi; ",
            "  printf '%s|%s|%s|%s\\n' \"$name\" \"$newest\" \"$bin_time\" \"$link\"; ",
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
        if parts.len() != 4 {
            continue;
        }
        let name = parts[0];
        let src_time: f64 = parts[1].parse().unwrap_or(0.0);
        let bin_time: f64 = parts[2].parse().unwrap_or(0.0);
        let link_target = parts[3].trim();

        // A source tree we could not read at all. Say so rather than silently calling it
        // clean — that silence is precisely what hid the BSD/GNU breakage described above.
        if src_time == 0.0 {
            results.push(CheckResult {
                name: format!("rust-binary-remote/{}", name),
                status: Status::Skipped,
                details: vec!["could not read any source mtime on remote".to_string()],
            });
            continue;
        }

        // Same rule as the local check: a symlink leaving ~/.local/bin is not this
        // project's artifact, so its mtime says nothing about this project's freshness.
        if !link_target.is_empty() {
            results.push(CheckResult {
                name: format!("rust-binary-remote/{}", name),
                status: Status::Skipped,
                details: vec![
                    format!("remote ~/.local/bin/{} is a symlink to {}", name, link_target),
                    "outside ~/.local/bin, so it is not this project's binary".to_string(),
                ],
            });
            continue;
        }

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

    // Keyed on absence of Drift, not absence of results — a Skipped entry must not
    // suppress the summary line (same rule as the local check).
    if !results.iter().any(|r| r.status == Status::Drift) {
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

/// Walk the board's `### YYYY-MM-DD — device` sections. Returns
/// (stale descriptions, fresh-message count, forum-pointer count). A section
/// whose first content line starts with `FORUM OPEN` / `FORUM COMPLETE` is a
/// forum pointer and never counts as stale.
pub fn messageboard_sections(content: &str, today: chrono::NaiveDate) -> (Vec<String>, usize, usize) {
    let mut stale = Vec::new();
    let mut fresh = 0usize;
    let mut pointers = 0usize;

    let mut lines = content.lines().peekable();
    while let Some(line) = lines.next() {
        if !line.starts_with("### ") {
            continue;
        }
        // first non-empty line of the body, without consuming the next header
        let mut body_first = "";
        while let Some(next) = lines.peek() {
            if next.starts_with("### ") {
                break;
            }
            let next = lines.next().unwrap_or("");
            if !next.trim().is_empty() {
                body_first = next.trim();
                break;
            }
        }
        if body_first.starts_with("FORUM OPEN") || body_first.starts_with("FORUM COMPLETE") {
            pointers += 1;
            continue;
        }
        let date_part: Vec<&str> = line
            .trim_start_matches("### ")
            .split(|c: char| c == ' ' || c == '\u{2014}' || c == '-')
            .take(3)
            .collect();
        let Some(date) = (date_part.len() >= 3)
            .then(|| format!("{}-{}-{}", date_part[0], date_part[1], date_part[2]))
            .and_then(|s| chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok())
        else {
            continue;
        };
        let age = (today - date).num_days();
        if age > 7 {
            stale.push(format!("{} ({} days old)", line.trim(), age));
        } else {
            fresh += 1;
        }
    }
    (stale, fresh, pointers)
}

#[cfg(test)]
mod messageboard_tests {
    use super::messageboard_sections;

    #[test]
    fn fresh_pointer_and_stale_are_told_apart() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 9, 2).unwrap();
        let board = "# Messageboard\n\n## Messages\n\n### 2026-09-01 — Mac\n\nfresh work order\n\n---\n\n### 2026-08-23 — nimbini\n\nFORUM OPEN — `meta-x` (architecture): pointer\n\n---\n\n### 2026-08-02 — nimbini\n\nold task\n";
        let (stale, fresh, pointers) = messageboard_sections(board, today);
        assert_eq!(fresh, 1);
        assert_eq!(pointers, 1);
        assert_eq!(stale, vec!["### 2026-08-02 — nimbini (31 days old)"]);
    }
}

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
    let today = chrono::Local::now().date_naive();
    let (stale_messages, fresh, pointers) = messageboard_sections(&content, today);

    // Fresh pending messages are the board doing its job, not drift. The old
    // rule reported Drift whenever ANY message existed, so this check could
    // never pass while the board was in use (review 2.2a class). Forum
    // pointers are exempt from the age rule: the board header says they are
    // attention gateways retired only when the thread closes.
    if stale_messages.is_empty() {
        Ok(CheckResult {
            name: "messageboard".to_string(),
            status: Status::Clean,
            details: if fresh + pointers == 0 {
                vec![]
            } else {
                vec![format!(
                    "{fresh} pending message(s) within 7 days, {pointers} forum pointer(s) (exempt)"
                )]
            },
        })
    } else {
        Ok(CheckResult {
            name: "messageboard".to_string(),
            status: Status::Drift,
            details: stale_messages,
        })
    }
}

// --- 5. Unmanaged scheduled jobs ---

/// Third-party and OS-provided job prefixes. These are installed by their own vendors
/// and are correctly NOT in dotfiles — flagging them would be noise that trains the
/// reader to ignore the check.
const VENDOR_JOB_PREFIXES: &[&str] = &[
    "com.apple.",
    "com.dropbox.",
    "com.google.",
    "com.microsoft.",
    "homebrew.mxcl.",
    "org.mozilla.",
    // Linux: shipped by packages into the user unit search path
    "dropbox.",
    "nushell-env.",
    "ssh-import-env.",
    "pipewire",
    "wireplumber",
    "xdg-",
    "gpg-agent",
    "dbus",
    "podman",
];

fn is_vendor_job(name: &str) -> bool {
    VENDOR_JOB_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Scheduled jobs that exist on disk but are NOT dotter symlinks.
///
/// # The gap this closes
///
/// `dotter-orphan-detector-v2` scans in ONE direction — dotfiles → deployed, asking
/// "is every file in the repo mapped?" Nothing asked the reverse: **"is every deployed
/// job backed by the repo?"** So a job installed by hand, or left behind when its
/// successor was brought under dotter, was invisible to every check on the system.
///
/// Found the hard way on 2026-07-31: `com.napier.forge-metadata-backup.plist`, an
/// unmanaged regular file from Nov 2025, had been firing monthly for ~9 months
/// alongside its dotter-managed replacement. It was failing every single run (exit 127
/// — it lacked the `PATH` its managed twin carries, so the wrapper's `#!/usr/bin/env nu`
/// shebang could not resolve), and **nothing reported that**, because no tool knew the
/// job existed.
///
/// A dotter-managed job is a symlink into `~/dotfiles`. A regular file is therefore the
/// signal: it is deployed, it runs, and nothing tracks it.
///
/// Reports Drift, not an error — an unmanaged job may be a deliberate machine-local
/// choice. The point is that the decision becomes visible instead of silent.
pub fn unmanaged_scheduled_jobs() -> Result<CheckResult> {
    let (dir, extensions) = if cfg!(target_os = "macos") {
        (home_dir().join("Library/LaunchAgents"), vec!["plist"])
    } else {
        (
            home_dir().join(".config/systemd/user"),
            vec!["service", "timer"],
        )
    };

    if !dir.exists() {
        return Ok(CheckResult {
            name: "unmanaged-jobs".to_string(),
            status: Status::Skipped,
            details: vec![format!("{} not found", dir.display())],
        });
    }

    let mut unmanaged = Vec::new();
    for entry in std::fs::read_dir(&dir)?.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        // Only real job files. A retired plist renamed out of the `.plist` suffix is
        // inert to launchd and deliberately ignored here too.
        let ext_ok = path
            .extension()
            .map(|e| extensions.contains(&e.to_string_lossy().as_ref()))
            .unwrap_or(false);
        if !ext_ok || is_vendor_job(&name) {
            continue;
        }

        // symlink_metadata does NOT follow the link — a dangling dotter symlink is still
        // managed, and is a different fault for a different tool to report.
        if let Ok(meta) = std::fs::symlink_metadata(&path) {
            if !meta.file_type().is_symlink() {
                unmanaged.push(name);
            }
        }
    }

    unmanaged.sort();

    if unmanaged.is_empty() {
        Ok(CheckResult {
            name: "unmanaged-jobs".to_string(),
            status: Status::Clean,
            details: vec![],
        })
    } else {
        // Wording matters here. Asked plainly on 2026-08-01, "unmanaged" was read as
        // "broken" — it is not, and every job in this list was verified running at the
        // time it was introduced. This check measures REPRODUCIBILITY, not health:
        // whether the job can be rebuilt from the repo on a fresh machine. Unit health
        // is system-health-check's job. Say so in the output so the next reader cannot
        // make the same inference.
        let mut details = vec![format!(
            "{} scheduled job(s) deployed but NOT dotter-managed:",
            unmanaged.len()
        )];
        details.extend(unmanaged.iter().map(|n| format!("  {}", n)));
        details.push(
            "NOT a health check — these may be running perfectly. They are simply not in \
             ~/dotfiles, so they would not survive a rebuild and cannot be reviewed in git."
                .to_string(),
        );
        details.push("for whether they actually RUN, see system-health-check".to_string());
        Ok(CheckResult {
            name: "unmanaged-jobs".to_string(),
            status: Status::Drift,
            details,
        })
    }
}
