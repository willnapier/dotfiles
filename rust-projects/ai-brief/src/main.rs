//! ai-brief — renders and verifies the effective assistant startup contract.
//!
//! Rust port (2026-09-01) of `scripts/ai-brief.nu`. Same CLI including the
//! compatibility forms (`ai-brief codex`, `ai-brief render codex`), same
//! section order, same byte-accurate capping, same header/hash/byte-field
//! scheme — so a payload rendered by either implementation verifies under
//! the other. The Nushell version was the oracle: markdown output diffed
//! byte-identical before the swap.
//!
//! Payload = header (schema, harness, host, sha256 of body, byte count,
//! budget) + body. Body = ORIENTATION kernel + machine layer + harness
//! adapter + Messageboard head + host health + open forum summary + forum
//! inbox, each live surface capped to its own byte budget. The whole thing
//! must fit the kernel's declared hard budget or rendering fails (and the
//! claude-hook format falls back to a short "read these files" notice).

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const ORIENTATION_SCHEMA: i64 = 1;
const DEFAULT_BUDGET: usize = 18000;
const HARNESSES: [&str; 4] = ["codex", "claude-code", "grok-build", "api"];
const MESSAGEBOARD_BUDGET: usize = 4500;
const FORUM_INDEX_BUDGET: usize = 3500;
const FORUM_INBOX_BUDGET: usize = 2000;
const HEALTH_BUDGET: usize = 1200;
const HEALTH_STALE_HOURS: i64 = 26;
const MARKER: &str = "## Vendor-neutral kernel";
const PLACEHOLDER: &str = "########";

#[derive(Parser, Debug)]
#[command(name = "ai-brief", version, about = "Render / verify the vendor-neutral assistant startup contract")]
struct Cli {
    /// `render` or `doctor`; a harness name here is the compatibility form of `render --harness <name>`
    #[arg(default_value = "render")]
    action: String,
    /// Harness (compatibility positional, e.g. `ai-brief render codex`)
    assistant: Option<String>,
    /// codex | claude-code | grok-build | api
    #[arg(long, default_value = "")]
    harness: String,
    /// macos | nimbini (auto-detected when omitted)
    #[arg(long, default_value = "")]
    host: String,
    /// Hard byte budget for the rendered payload
    #[arg(long, default_value_t = DEFAULT_BUDGET)]
    budget: usize,
    /// markdown | claude-hook
    #[arg(long, default_value = "markdown")]
    format: String,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let compatibility_harness = if HARNESSES.contains(&cli.action.as_str()) { cli.action.clone() } else { String::new() };
    let operation = if compatibility_harness.is_empty() { cli.action.clone() } else { "render".to_string() };
    let selected = if !cli.harness.trim().is_empty() {
        cli.harness.to_lowercase()
    } else if !compatibility_harness.is_empty() {
        compatibility_harness
    } else {
        cli.assistant.clone().unwrap_or_default().to_lowercase()
    };
    let home = home_dir();

    match operation.as_str() {
        "render" => {
            if selected.trim().is_empty() {
                bail!("usage: ai-brief render --harness <codex|claude-code|grok-build|api> [--host macos|nimbini]");
            }
            match cli.format.as_str() {
                "markdown" => {
                    let machine = resolve_host(&cli.host)?;
                    print!("{}", render_contract(&home, &selected, &machine, cli.budget)?);
                    Ok(())
                }
                "claude-hook" => {
                    let payload = match resolve_host(&cli.host) {
                        Ok(machine) => render_contract(&home, &selected, &machine, cli.budget)
                            .unwrap_or_else(|e| claude_fallback(&machine, &format!("{e:#}"))),
                        Err(e) => claude_fallback("", &format!("{e:#}")),
                    };
                    let json = serde_json::json!({
                        "hookSpecificOutput": {
                            "hookEventName": "SessionStart",
                            "additionalContext": payload
                        }
                    });
                    print!("{}", serde_json::to_string(&json)?);
                    Ok(())
                }
                other => bail!("unknown format: {other}"),
            }
        }
        "doctor" => doctor(&home, &resolve_host(&cli.host)?, cli.budget),
        other => bail!("unknown action: {other}; expected render or doctor"),
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

// ---------------------------------------------------------------------------
// Sources and host
// ---------------------------------------------------------------------------

fn required_text(path: &Path) -> Result<String> {
    if !path.exists() {
        bail!("required orientation source is missing: {}", path.display());
    }
    fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))
}

fn short_hostname() -> String {
    let candidates: [Option<String>; 3] = [
        fs::read_to_string("/etc/hostname").ok(),
        Command::new("hostname").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()),
        std::env::var("HOSTNAME").ok(),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(|h| h.trim().to_lowercase())
        .find(|h| !h.is_empty())
        .unwrap_or_default()
}

fn resolve_host(requested: &str) -> Result<String> {
    if !requested.trim().is_empty() {
        let normalized = requested.to_lowercase();
        if normalized != "macos" && normalized != "nimbini" {
            bail!("unknown host layer: {requested}; expected macos or nimbini");
        }
        return Ok(normalized);
    }
    if cfg!(target_os = "macos") {
        return Ok("macos".to_string());
    }
    let hostname = short_hostname();
    if hostname.contains("nimbini") {
        Ok("nimbini".to_string())
    } else {
        bail!("cannot map host {hostname} / {} to an orientation machine layer", std::env::consts::OS)
    }
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// Keep whole lines until the next one would push the component over its
/// byte budget, then append a truncation notice. Byte counts are UTF-8.
fn cap_component(text: &str, budget: usize, label: &str) -> String {
    if text.len() <= budget {
        return text.to_string();
    }
    let notice = format!("\n… [{label} truncated at startup; load the source on demand]");
    let content_budget = budget.saturating_sub(notice.len());
    let mut kept: Vec<&str> = vec![];
    let mut used = 0usize;
    for line in text.lines() {
        let bytes = if kept.is_empty() { line.len() } else { line.len() + 1 };
        if used + bytes > content_budget {
            break;
        }
        kept.push(line);
        used += bytes;
    }
    format!("{}{notice}", kept.join("\n"))
}

fn messageboard_head(path: &Path) -> Result<String> {
    let raw = required_text(path)?;
    let mut sections = raw.split("\n### ");
    sections.next(); // everything before the first entry
    let Some(first) = sections.next() else {
        return Ok("No current Messageboard entries.".to_string());
    };
    let trimmed = first.trim();
    let body = trimmed.strip_suffix("\n---").unwrap_or(trimmed);
    Ok(cap_component(&format!("### {body}"), MESSAGEBOARD_BUDGET, "Messageboard head"))
}

fn forum_open_summary(path: &Path) -> Result<String> {
    let text = required_text(path)?;
    let mut inside = false;
    let mut rows: Vec<&str> = vec![];
    for line in text.lines() {
        if line == "## Open" {
            inside = true;
        } else if inside && line.starts_with("## ") {
            inside = false;
        } else if inside && line.starts_with("| `") {
            rows.push(line);
        }
    }
    Ok(if rows.is_empty() {
        "No open forum threads.".to_string()
    } else {
        cap_component(&rows.join("\n"), FORUM_INDEX_BUDGET, "forum index summary")
    })
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

fn forum_inbox_summary() -> String {
    if !on_path("forum") {
        return "WARNING: forum CLI unavailable; unread completion state could not be checked.".to_string();
    }
    match Command::new("forum").args(["inbox", "--format", "brief"]).output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let code = out.status.code().unwrap_or(-1);
            if code == 0 && !stdout.trim().is_empty() {
                cap_component(stdout.trim(), FORUM_INBOX_BUDGET, "forum inbox")
            } else if code == 0 {
                "WARNING: forum inbox returned empty output; unread completion state is unknown.".to_string()
            } else {
                format!("WARNING: forum inbox failed with exit {code}: {}", String::from_utf8_lossy(&out.stderr).trim())
            }
        }
        Err(e) => format!("WARNING: forum inbox check failed: {e}"),
    }
}

/// Last system-health-check result per machine, with its age. Files are
/// written by system-health-check to ~/Assistants/health/<host>.json (one
/// writer per file; Syncthing carries the other machine's). Missing or stale
/// is reported as such — "could not check" is never rendered as "fine".
fn host_health_summary(home: &Path) -> String {
    host_health_summary_at(&home.join("Assistants/health"), chrono::Local::now().into())
}

fn host_health_summary_at(dir: &Path, now: chrono::DateTime<chrono::FixedOffset>) -> String {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    if files.is_empty() {
        return format!(
            "WARNING: no host health status under {} — system-health-check has not written one on any machine; treat every host as unchecked.",
            dir.display()
        );
    }

    let rows: Vec<String> = files
        .iter()
        .map(|f| {
            let basename = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let Ok(text) = fs::read_to_string(f) else { return format!("{basename}: unreadable status file") };
            let Ok(s) = serde_json::from_str::<serde_json::Value>(&text) else {
                return format!("{basename}: unreadable status file");
            };
            let host = s.get("host").and_then(|v| v.as_str()).unwrap_or(&basename).to_string();
            let count = s.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
            let age = s
                .get("checked_at")
                .and_then(|v| v.as_str())
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .map(|checked| now.signed_duration_since(checked));
            let age_text = match age {
                None => "unknown age".to_string(),
                Some(a) if a < chrono::Duration::hours(1) => {
                    format!("{}m ago", (a.num_milliseconds() as f64 / 60_000.0).round())
                }
                Some(a) => format!("{}h ago", (a.num_milliseconds() as f64 / 3_600_000.0).round()),
            };
            let stale = match age {
                None => true,
                Some(a) => a > chrono::Duration::hours(HEALTH_STALE_HOURS),
            };
            let head = if count == 0 { format!("{host}: ✅ clean") } else { format!("{host}: 🚨 {count} problems") };
            let when = if stale {
                format!(" — STALE (last check {age_text}; the health check itself may be dead)")
            } else {
                format!(" (checked {age_text})")
            };
            let problems: Vec<String> = s
                .get("problems")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|p| p.as_str()).map(|p| format!("  - {p}")).collect())
                .unwrap_or_default();
            if count == 0 {
                format!("{head}{when}")
            } else {
                format!("{head}{when}\n{}", problems.join("\n"))
            }
        })
        .collect();
    cap_component(&rows.join("\n"), HEALTH_BUDGET, "host health")
}

// ---------------------------------------------------------------------------
// Payload assembly and verification
// ---------------------------------------------------------------------------

fn sha256_hex(s: &str) -> String {
    let digest = Sha256::digest(s.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn assemble_payload(harness: &str, host: &str, budget: usize, body: &str) -> Result<String> {
    let content_hash = sha256_hex(body);
    let header = format!(
        "# Effective Assistant Startup Contract\n\norientation-schema: {ORIENTATION_SCHEMA}\nharness: {harness}\nhost: {host}\ncontent-sha256: {content_hash}\npayload-bytes: {PLACEHOLDER}\nbudget-bytes: {budget}\n"
    );
    let template = format!("{header}\n{body}");
    let total = template.len();
    if total > 99_999_999 {
        bail!("startup payload exceeds the fixed eight-digit byte field");
    }
    if total > budget {
        bail!("startup payload is {total} bytes, exceeding the hard budget of {budget}");
    }
    Ok(template.replacen(PLACEHOLDER, &format!("{total:08}"), 1))
}

fn metadata_value(payload: &str, key: &str) -> Result<String> {
    let prefix = format!("{key}:");
    payload
        .lines()
        .find(|l| l.starts_with(&prefix))
        .map(|l| l[prefix.len()..].trim().to_string())
        .ok_or_else(|| anyhow!("payload lacks metadata line `{key}`"))
}

#[derive(Debug)]
struct Verified {
    bytes: usize,
    hash: String,
}

fn verify_payload(payload: &str, expected_harness: &str, expected_host: &str) -> Result<Verified> {
    let split: Vec<&str> = payload.split(MARKER).collect();
    if split.len() != 2 {
        bail!("effective payload lacks a unique vendor-neutral body marker");
    }
    let body = format!("{MARKER}{}", split[1].trim_end());
    let actual_hash = sha256_hex(&body);
    let claimed_hash = metadata_value(payload, "content-sha256")?;
    let actual_bytes = payload.len();
    let claimed_bytes: usize = metadata_value(payload, "payload-bytes")?.parse().context("payload-bytes is not a number")?;
    let budget: usize = metadata_value(payload, "budget-bytes")?.parse().context("budget-bytes is not a number")?;
    let schema: i64 = metadata_value(payload, "orientation-schema")?.parse().context("orientation-schema is not a number")?;
    let harness = metadata_value(payload, "harness")?;
    let host = metadata_value(payload, "host")?;

    if actual_hash != claimed_hash {
        bail!("content hash mismatch: claimed {claimed_hash}, actual {actual_hash}");
    }
    if actual_bytes != claimed_bytes {
        bail!("payload byte mismatch: claimed {claimed_bytes}, actual {actual_bytes}");
    }
    if actual_bytes > budget {
        bail!("payload is {actual_bytes} bytes, exceeding budget {budget}");
    }
    if schema != ORIENTATION_SCHEMA || harness != expected_harness || host != expected_host {
        bail!("payload schema, harness, or host metadata does not match the render request");
    }
    Ok(Verified { bytes: actual_bytes, hash: actual_hash })
}

fn claude_fallback(host: &str, error_message: &str) -> String {
    let machine_source = if host.is_empty() {
        "the applicable file under `~/Assistants/context/machines/`".to_string()
    } else {
        format!("`~/Assistants/context/machines/{host}.md`")
    };
    format!(
        "# Orientation renderer fallback\n\nThe full startup contract could not be assembled: {error_message}\n\nBefore beginning the task, read `~/Assistants/shared/ORIENTATION.md`, {machine_source}, and `~/Assistants/context/briefings/claude-code.md`; then inspect the current Messageboard head, `design-forum/INDEX.md`, and `forum inbox`. Treat those sources as mandatory context."
    )
}

fn kernel_int(kernel: &str, key: &str) -> Result<i64> {
    kernel
        .lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.rsplit(':').next())
        .map(|v| v.trim())
        .ok_or_else(|| anyhow!("ORIENTATION.md lacks `{key}`"))?
        .parse()
        .with_context(|| format!("ORIENTATION.md `{key}` is not a number"))
}

fn render_contract(home: &Path, harness: &str, host: &str, budget: usize) -> Result<String> {
    if !HARNESSES.contains(&harness) {
        bail!("unknown harness: {harness}; expected {}", HARNESSES.join(", "));
    }
    if budget == 0 {
        bail!("budget must be greater than zero");
    }

    let kernel_path = home.join("Assistants/shared/ORIENTATION.md");
    let machine_path = home.join(format!("Assistants/context/machines/{host}.md"));
    let adapter_path = home.join(format!("Assistants/context/briefings/{harness}.md"));
    let messageboard_path = home.join("Assistants/shared/MESSAGEBOARD.md");
    let index_path = home.join("Assistants/shared/design-forum/INDEX.md");

    let kernel = required_text(&kernel_path)?;
    let declared_schema = kernel_int(&kernel, "orientation_schema:")?;
    if declared_schema != ORIENTATION_SCHEMA {
        bail!("renderer schema {ORIENTATION_SCHEMA} does not match ORIENTATION.md schema {declared_schema}");
    }
    let declared_budget = kernel_int(&kernel, "render_budget_bytes:")?;
    if budget as i64 > declared_budget {
        bail!("requested budget {budget} exceeds ORIENTATION.md hard limit {declared_budget}");
    }

    let sections = [
        MARKER.to_string(),
        kernel.trim().to_string(),
        format!("## Machine layer: {host}"),
        required_text(&machine_path)?.trim().to_string(),
        format!("## Harness adapter: {harness}"),
        required_text(&adapter_path)?.trim().to_string(),
        "## Messageboard head (transient)".to_string(),
        messageboard_head(&messageboard_path)?,
        "## Host health (last system-health-check per machine)".to_string(),
        host_health_summary(home),
        "## Open forum summary (discovery only)".to_string(),
        forum_open_summary(&index_path)?,
        "## Forum inbox".to_string(),
        forum_inbox_summary(),
    ];
    assemble_payload(harness, host, budget, &sections.join("\n\n"))
}

// ---------------------------------------------------------------------------
// Doctor
// ---------------------------------------------------------------------------

struct Row {
    harness: String,
    status: &'static str,
    detail: String,
}

fn verify_startup_surface(label: &str, path: &Path, needle: &str) -> Row {
    if !path.exists() {
        return Row { harness: label.into(), status: "FAIL", detail: format!("missing {}", path.display()) };
    }
    let raw = fs::read_to_string(path).unwrap_or_default();
    // Accept the historical `ai-brief.nu` spelling and the bare binary name.
    let alt = needle.replace("ai-brief.nu", "ai-brief");
    if raw.contains(needle) || raw.contains(&alt) {
        Row { harness: label.into(), status: "ok", detail: format!("startup surface {}", path.display()) }
    } else {
        Row { harness: label.into(), status: "FAIL", detail: format!("startup surface does not reference renderer: {}", path.display()) }
    }
}

fn doctor(home: &Path, host: &str, budget: usize) -> Result<()> {
    let mut rows: Vec<Row> = vec![];
    for harness in HARNESSES {
        let row = match render_contract(home, harness, host, budget).and_then(|p| verify_payload(&p, harness, host)) {
            Ok(v) => Row { harness: harness.into(), status: "ok", detail: format!("verified {} bytes; sha256: {}", v.bytes, v.hash) },
            Err(e) => Row { harness: harness.into(), status: "FAIL", detail: format!("{e:#}") },
        };
        rows.push(row);
    }

    let boundary = (|| -> Result<Verified> {
        let filler = "x".repeat(9760);
        let body = format!("## Vendor-neutral kernel\n\n{filler}");
        let payload = assemble_payload("boundary-test", host, 18000, &body)?;
        verify_payload(&payload, "boundary-test", host)
    })();
    rows.push(match boundary {
        Ok(v) => Row { harness: "byte-boundary".into(), status: "ok", detail: format!("verified fixed-width metadata at {} bytes", v.bytes) },
        Err(e) => Row { harness: "byte-boundary".into(), status: "FAIL", detail: format!("{e:#}") },
    });

    rows.push(verify_startup_surface("codex", &home.join(".codex/AGENTS.md"), "ai-brief.nu render --harness codex"));
    rows.push(verify_startup_surface(
        "claude-code",
        &home.join(".claude/settings.json"),
        "ai-brief.nu render --harness claude-code --format claude-hook",
    ));
    rows.push(verify_startup_surface("grok-build", &home.join(".grok/AGENTS.md"), "ai-brief.nu render --harness grok-build"));
    rows.push(verify_startup_surface("api", &home.join("Assistants/context/briefings/api.md"), "ai-brief.nu render --harness api"));

    println!("{:<14} {:<6} detail", "harness", "status");
    for r in &rows {
        println!("{:<14} {:<6} {}", r.harness, r.status, r.detail);
    }
    if rows.iter().any(|r| r.status == "FAIL") {
        bail!("orientation doctor found failures");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ai-brief-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn cap_component_keeps_whole_lines_within_budget() {
        // 6 × 20 bytes + 5 newlines = 125 bytes, comfortably over the 90-byte budget below
        let text = "line-one-is-twenty-c\nline-two-is-twenty-c\nline-three-twenty-cc\nline-four-is-twenty-\nline-five-is-twenty-\nline-six-is-twenty-c";
        assert_eq!(text.len(), 125);
        assert_eq!(cap_component(text, 200, "x"), text);
        let capped = cap_component(text, 90, "x");
        assert!(capped.len() <= 90, "{} bytes", capped.len());
        assert!(capped.starts_with("line-one-is-twenty-c\n… ["), "{capped:?}");
        assert!(capped.ends_with("[x truncated at startup; load the source on demand]"));
        assert!(!capped.contains("line-two"));
    }

    #[test]
    fn messageboard_head_takes_first_entry_and_strips_rule() {
        let d = temp("mb");
        let p = d.join("MESSAGEBOARD.md");
        fs::write(&p, "# Messageboard\n\nintro\n\n### 2026-09-01 — Mac\n\nfirst body\n\n---\n\n### 2026-08-31 — nimbini\n\nsecond\n").unwrap();
        assert_eq!(messageboard_head(&p).unwrap(), "### 2026-09-01 — Mac\n\nfirst body\n");
        fs::write(&p, "# Messageboard\n\nnothing yet\n").unwrap();
        assert_eq!(messageboard_head(&p).unwrap(), "No current Messageboard entries.");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn forum_open_summary_collects_only_open_rows() {
        let d = temp("forum");
        let p = d.join("INDEX.md");
        fs::write(&p, "# Index\n\n## Open\n\n| id | x |\n| `a` | open-a |\n\n| `b` | open-b |\n\n## Decided\n\n| `c` | decided |\n").unwrap();
        assert_eq!(forum_open_summary(&p).unwrap(), "| `a` | open-a |\n| `b` | open-b |");
        fs::write(&p, "## Open\n\n## Decided\n| `c` | d |\n").unwrap();
        assert_eq!(forum_open_summary(&p).unwrap(), "No open forum threads.");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn assemble_then_verify_roundtrips_and_detects_tampering() {
        let body = format!("{MARKER}\n\nhello kernel\n\n## Machine layer: macos\n\nstuff");
        let payload = assemble_payload("codex", "macos", 18000, &body).unwrap();
        assert!(payload.contains(&format!("payload-bytes: {:08}", payload.len())));
        let v = verify_payload(&payload, "codex", "macos").unwrap();
        assert_eq!(v.bytes, payload.len());
        assert_eq!(v.hash, sha256_hex(&body));
        assert!(verify_payload(&payload, "codex", "nimbini").is_err());
        let tampered = payload.replace("hello kernel", "hello kernal");
        assert!(verify_payload(&tampered, "codex", "macos").unwrap_err().to_string().contains("content hash mismatch"));
        assert!(assemble_payload("codex", "macos", 10, &body).unwrap_err().to_string().contains("exceeding the hard budget"));
    }

    #[test]
    fn boundary_payload_uses_fixed_width_byte_field() {
        let body = format!("{MARKER}\n\n{}", "x".repeat(9760));
        let payload = assemble_payload("boundary-test", "macos", 18000, &body).unwrap();
        let v = verify_payload(&payload, "boundary-test", "macos").unwrap();
        assert_eq!(v.bytes, payload.len());
        assert!(!payload.contains(PLACEHOLDER));
    }

    #[test]
    fn metadata_value_reads_first_matching_line() {
        assert_eq!(metadata_value("a: 1\nhost: macos\nhost: other", "host").unwrap(), "macos");
        assert!(metadata_value("a: 1", "host").is_err());
    }

    #[test]
    fn host_health_renders_fresh_stale_unreadable_and_missing() {
        let d = temp("health");
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-01T23:00:00+01:00").unwrap();
        assert!(host_health_summary_at(&d.join("nope"), now).starts_with("WARNING: no host health status"));
        fs::write(d.join("macos.json"), r#"{"host":"macos","checked_at":"2026-09-01T22:50:00+01:00","count":2,"problems":["p1","p2"]}"#).unwrap();
        fs::write(d.join("nimbini.json"), r#"{"host":"nimbini","checked_at":"2026-08-25T08:00:00+01:00","count":0,"problems":[]}"#).unwrap();
        fs::write(d.join("zz.json"), "not json").unwrap();
        let out = host_health_summary_at(&d, now);
        assert_eq!(
            out,
            "macos: 🚨 2 problems (checked 10m ago)\n  - p1\n  - p2\nnimbini: ✅ clean — STALE (last check 183h ago; the health check itself may be dead)\nzz.json: unreadable status file"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn resolve_host_accepts_known_layers_only() {
        assert_eq!(resolve_host("MacOS").unwrap(), "macos");
        assert_eq!(resolve_host("nimbini").unwrap(), "nimbini");
        assert!(resolve_host("toaster").unwrap_err().to_string().contains("unknown host layer"));
    }

    #[test]
    fn kernel_ints_parse_frontmatter() {
        let k = "---\norientation_schema: 1\nrender_budget_bytes: 18000\n---\n";
        assert_eq!(kernel_int(k, "orientation_schema:").unwrap(), 1);
        assert_eq!(kernel_int(k, "render_budget_bytes:").unwrap(), 18000);
        assert!(kernel_int(k, "nope:").is_err());
    }
}
