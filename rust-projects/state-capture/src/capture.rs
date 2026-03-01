use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

use crate::config::Config;

/// Run all captures, writing output files into state_dir.
/// Returns true if all succeeded.
pub fn run_all(config: &Config, dry_run: bool) -> Result<bool> {
    let state_dir = config.state_dir();
    if !state_dir.exists() {
        std::fs::create_dir_all(&state_dir)
            .with_context(|| format!("Failed to create state dir: {}", state_dir.display()))?;
    }

    let mut all_ok = true;

    for cap in &config.captures {
        let out_path = state_dir.join(&cap.output);

        if dry_run {
            println!("[dry-run] {} → {}", cap.name, out_path.display());
            println!("  command: {}", cap.command);
            continue;
        }

        match run_command(&cap.command) {
            Ok(mut output) => {
                if cap.sort {
                    output = sort_lines(&output);
                }
                std::fs::write(&out_path, &output).with_context(|| {
                    format!("Failed to write {}", out_path.display())
                })?;
                println!("  {} → {} ({} bytes)", cap.name, cap.output, output.len());
            }
            Err(e) => {
                eprintln!("  {} FAILED: {}", cap.name, e);
                all_ok = false;
            }
        }
    }

    Ok(all_ok)
}

/// Run a single shell command via sh -c, returning its stdout.
fn run_command(cmd: &str) -> Result<String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .with_context(|| format!("Failed to execute: {}", cmd))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "Command exited with {}: {}",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run a command and return its stdout (for drift checking).
pub fn run_command_live(cmd: &str) -> Result<String> {
    run_command(cmd)
}

fn sort_lines(text: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    lines.sort();
    let mut result = lines.join("\n");
    if !result.is_empty() {
        result.push('\n');
    }
    result
}

/// List all configured captures.
pub fn list_captures(config: &Config) {
    let state_dir = config.state_dir();
    println!("{:<25} {:<40} {}", "NAME", "OUTPUT", "EXISTS");
    println!("{}", "-".repeat(75));
    for cap in &config.captures {
        let out_path = state_dir.join(&cap.output);
        let exists = if out_path.exists() { "yes" } else { "no" };
        println!("{:<25} {:<40} {}", cap.name, cap.output, exists);
    }
}

/// Show the content of a specific capture's baseline file.
pub fn show_capture(config: &Config, name: &str) -> Result<()> {
    let cap = config
        .captures
        .iter()
        .find(|c| c.name == name)
        .with_context(|| format!("No capture named '{}'", name))?;

    let state_dir = config.state_dir();
    let path = state_dir.join(&cap.output);

    if !path.exists() {
        anyhow::bail!(
            "Baseline not found: {} (run 'state-capture capture' first)",
            path.display()
        );
    }

    let content = std::fs::read_to_string(&path)?;
    print!("{}", content);
    Ok(())
}

/// Read a baseline file's content.
pub fn read_baseline(state_dir: &Path, filename: &str) -> Result<Option<String>> {
    let path = state_dir.join(filename);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(std::fs::read_to_string(&path)?))
}
