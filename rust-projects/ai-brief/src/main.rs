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
/// Maxima for the four live surfaces. They are ceilings, not entitlements:
/// the surfaces share whatever the fixed parts (kernel, machine layer,
/// adapter, frame) leave of the budget, scaled down in these proportions when
/// that residual is smaller than their sum. See `plan_live_budgets`.
const MESSAGEBOARD_BUDGET: usize = 4500;
const FORUM_INDEX_BUDGET: usize = 3500;
const FORUM_INBOX_BUDGET: usize = 2000;
const HEALTH_BUDGET: usize = 1200;
/// The minimum a live surface is ever given: room for its own "truncated"
/// notice and one line, so a starved surface still says it was starved.
const LIVE_FLOOR: usize = 120;
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

/// The merged Messageboard (every host's file plus the legacy file, tombstones
/// applied) comes from `messageboard-edit render` since 2026-09-23; the legacy
/// file alone is the fallback when the binary is absent or fails.
fn messageboard_head_merged(legacy_path: &Path, budget: usize) -> Result<String> {
    if on_path("messageboard-edit") {
        let root = legacy_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        if let Ok(o) = Command::new("messageboard-edit").args(["--root".as_ref(), root.as_os_str(), "render".as_ref()]).output() {
            if o.status.success() {
                return Ok(messageboard_head_text(&String::from_utf8_lossy(&o.stdout), budget));
            }
        }
    }
    messageboard_head(legacy_path, budget)
}

fn messageboard_head(path: &Path, budget: usize) -> Result<String> {
    let raw = required_text(path)?;
    Ok(messageboard_head_text(&raw, budget))
}

fn messageboard_head_text(raw: &str, budget: usize) -> String {
    let mut sections = raw.split("\n### ");
    sections.next(); // everything before the first entry
    let Some(first) = sections.next() else {
        return "No current Messageboard entries.".to_string();
    };
    let trimmed = first.trim();
    let body = trimmed.strip_suffix("\n---").unwrap_or(trimmed);
    cap_component(&format!("### {body}"), budget, "Messageboard head")
}

fn forum_open_summary(path: &Path, budget: usize) -> Result<String> {
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
        cap_component(&rows.join("\n"), budget, "forum index summary")
    })
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

fn forum_inbox_summary(budget: usize) -> String {
    if !on_path("forum") {
        return "WARNING: forum CLI unavailable; unread completion state could not be checked.".to_string();
    }
    match Command::new("forum").args(["inbox", "--format", "brief"]).output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let code = out.status.code().unwrap_or(-1);
            if code == 0 && !stdout.trim().is_empty() {
                cap_component(stdout.trim(), budget, "forum inbox")
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
fn host_health_summary(home: &Path, budget: usize) -> String {
    host_health_summary_at(&home.join("Assistants/health"), chrono::Local::now().into(), budget)
}

fn host_health_summary_at(dir: &Path, now: chrono::DateTime<chrono::FixedOffset>, budget: usize) -> String {
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
            // Parked problems (system-health-check ≥ 0.5.0): a recorded
            // decision with an end date — shown, counted apart, never red.
            let parked = s.get("parked").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let head = match (count, parked.len()) {
                (0, 0) => format!("{host}: ✅ clean"),
                (0, p) => format!("{host}: ✅ clean ({p} parked)"),
                (n, 0) => format!("{host}: 🚨 {n} problems"),
                (n, p) => format!("{host}: 🚨 {n} problems, {p} parked"),
            };
            let when = if stale {
                format!(" — STALE (last check {age_text}; the health check itself may be dead)")
            } else {
                format!(" (checked {age_text})")
            };
            // Age per problem (system-health-check ≥ 0.4.0 writes
            // `problem_history`): "[since 15 Sep, 8×]" is the cue that a red
            // has been ignored; "[new]" may be a wake-time artefact.
            let history = s.get("problem_history").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let age_tag = |text: &str| -> String {
                let Some(rec) = history.iter().find(|r| r.get("text").and_then(|t| t.as_str()) == Some(text)) else {
                    return String::new();
                };
                let runs = rec.get("runs").and_then(|r| r.as_u64()).unwrap_or(1);
                if runs <= 1 {
                    return " [new]".to_string();
                }
                let since = rec
                    .get("first_seen")
                    .and_then(|f| f.as_str())
                    .and_then(|f| chrono::DateTime::parse_from_rfc3339(f).ok())
                    .map(|f| f.format("%-d %b").to_string())
                    .unwrap_or_else(|| "?".to_string());
                format!(" [since {since}, {runs}×]")
            };
            let mut lines: Vec<String> = s
                .get("problems")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|p| p.as_str()).map(|p| format!("  - {p}{}", age_tag(p))).collect())
                .unwrap_or_default();
            for park in &parked {
                let text = park.get("text").and_then(|t| t.as_str()).unwrap_or("?");
                let until = park
                    .get("until")
                    .and_then(|u| u.as_str())
                    .and_then(|u| chrono::NaiveDate::parse_from_str(u, "%Y-%m-%d").ok())
                    .map(|u| u.format("%-d %b").to_string())
                    .unwrap_or_else(|| "?".to_string());
                let reason = park.get("reason").and_then(|r| r.as_str()).unwrap_or("");
                lines.push(format!("  ⏸ {text} — parked until {until}: {reason}{}", age_tag(text)));
            }
            if lines.is_empty() {
                format!("{head}{when}")
            } else {
                format!("{head}{when}\n{}", lines.join("\n"))
            }
        })
        .collect();
    cap_component(&rows.join("\n"), budget, "host health")
}

// ---------------------------------------------------------------------------
// Payload assembly and verification
// ---------------------------------------------------------------------------

fn sha256_hex(s: &str) -> String {
    let digest = Sha256::digest(s.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The header as rendered, with the byte field still a placeholder of the
/// same width as the final number, so its length is the header's length.
fn payload_header(harness: &str, host: &str, budget: usize, content_hash: &str) -> String {
    format!(
        "# Effective Assistant Startup Contract\n\norientation-schema: {ORIENTATION_SCHEMA}\nharness: {harness}\nhost: {host}\ncontent-sha256: {content_hash}\npayload-bytes: {PLACEHOLDER}\nbudget-bytes: {budget}\n"
    )
}

fn assemble_payload(harness: &str, host: &str, budget: usize, body: &str) -> Result<String> {
    let content_hash = sha256_hex(body);
    let header = payload_header(harness, host, budget, &content_hash);
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

/// Byte budgets for the four live surfaces, in section order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LiveBudgets {
    messageboard: usize,
    health: usize,
    forum_index: usize,
    forum_inbox: usize,
}

impl LiveBudgets {
    const MAXIMA: LiveBudgets = LiveBudgets {
        messageboard: MESSAGEBOARD_BUDGET,
        health: HEALTH_BUDGET,
        forum_index: FORUM_INDEX_BUDGET,
        forum_inbox: FORUM_INBOX_BUDGET,
    };

    fn total(&self) -> usize {
        self.messageboard + self.health + self.forum_index + self.forum_inbox
    }
}

/// Give the live surfaces what the fixed parts leave. Each surface gets its
/// maximum when the residual covers all four; otherwise all four scale down
/// in proportion, none below `LIVE_FLOOR`. Fails only when the fixed parts
/// alone (plus the floors) do not fit — that is a kernel problem, not a busy
/// week, and deserves the hard failure.
fn plan_live_budgets(fixed_bytes: usize, budget: usize) -> Result<LiveBudgets> {
    let max = LiveBudgets::MAXIMA;
    let floors = 4 * LIVE_FLOOR;
    if fixed_bytes + floors > budget {
        bail!(
            "the fixed parts of the startup contract (kernel, machine layer, adapter, frame) are {fixed_bytes} bytes, leaving no room under the hard budget of {budget} for the live surfaces; shorten ORIENTATION.md or the machine/harness layers"
        );
    }
    let residual = budget - fixed_bytes;
    if residual >= max.total() {
        return Ok(max);
    }
    // Water-fill: a surface whose proportional share would fall below the
    // floor is pinned at the floor and the rest of the residual is shared, in
    // proportion, among the others — repeated until nothing new pins. Plain
    // "scale then max(floor)" could overshoot the residual (found 2026-09-22:
    // the 0.3.0 debug_assert below failed at residual 500).
    let maxima = [max.messageboard, max.health, max.forum_index, max.forum_inbox];
    let mut planned = [0usize; 4];
    let mut pinned = [false; 4];
    loop {
        let free_max: usize = (0..4).filter(|&i| !pinned[i]).map(|i| maxima[i]).sum();
        let free_residual = residual.saturating_sub(pinned.iter().filter(|&&p| p).count() * LIVE_FLOOR);
        let mut newly_pinned = false;
        for i in 0..4 {
            if pinned[i] {
                planned[i] = LIVE_FLOOR;
                continue;
            }
            let share = if free_max == 0 { 0 } else { ((maxima[i] as u128 * free_residual as u128) / free_max as u128) as usize };
            if share < LIVE_FLOOR {
                pinned[i] = true;
                newly_pinned = true;
            } else {
                planned[i] = share;
            }
        }
        if !newly_pinned {
            break;
        }
    }
    let planned = LiveBudgets { messageboard: planned[0], health: planned[1], forum_index: planned[2], forum_inbox: planned[3] };
    debug_assert!(planned.total() <= residual, "{planned:?} exceeds residual {residual}");
    Ok(planned)
}

/// Where the bytes went, for `doctor`.
#[derive(Debug)]
struct Breakdown {
    budget: usize,
    kernel: usize,
    machine: usize,
    adapter: usize,
    frame: usize,
    fixed: usize,
    residual: usize,
    planned: LiveBudgets,
    actual: LiveBudgets,
    total: usize,
}

impl Breakdown {
    fn describe(&self) -> String {
        format!(
            "fixed {} = kernel {} + machine {} + adapter {} + frame {}; residual {} of budget {}; live (used/cap): messageboard {}/{}, health {}/{}, forum index {}/{}, forum inbox {}/{}; total {}",
            self.fixed, self.kernel, self.machine, self.adapter, self.frame, self.residual, self.budget,
            self.actual.messageboard, self.planned.messageboard, self.actual.health, self.planned.health,
            self.actual.forum_index, self.planned.forum_index, self.actual.forum_inbox, self.planned.forum_inbox, self.total
        )
    }
}

fn render_contract(home: &Path, harness: &str, host: &str, budget: usize) -> Result<String> {
    render_with_breakdown(home, harness, host, budget).map(|(payload, _)| payload)
}

fn render_with_breakdown(home: &Path, harness: &str, host: &str, budget: usize) -> Result<(String, Breakdown)> {
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

    // Fixed parts first: everything whose size the live surfaces cannot change.
    let kernel_body = kernel.trim().to_string();
    let machine_body = required_text(&machine_path)?.trim().to_string();
    let adapter_body = required_text(&adapter_path)?.trim().to_string();
    let headings = [
        MARKER.to_string(),
        format!("## Machine layer: {host}"),
        format!("## Harness adapter: {harness}"),
        "## Messageboard head (transient)".to_string(),
        "## Host health (last system-health-check per machine)".to_string(),
        "## Open forum summary (discovery only)".to_string(),
        "## Forum inbox".to_string(),
    ];
    // Frame = header (with its fixed-width byte field), the blank line after
    // it, the seven headings, and the "\n\n" between each of the 14 sections.
    let header_len = payload_header(harness, host, budget, &sha256_hex("")).len() + 1;
    let frame = header_len + headings.iter().map(|h| h.len()).sum::<usize>() + 13 * 2;
    let fixed = kernel_body.len() + machine_body.len() + adapter_body.len() + frame;
    let planned = plan_live_budgets(fixed, budget)?;

    let messageboard = messageboard_head_merged(&messageboard_path, planned.messageboard)?;
    let health = host_health_summary(home, planned.health);
    let forum_index = forum_open_summary(&index_path, planned.forum_index)?;
    let forum_inbox = forum_inbox_summary(planned.forum_inbox);
    let actual = LiveBudgets {
        messageboard: messageboard.len(),
        health: health.len(),
        forum_index: forum_index.len(),
        forum_inbox: forum_inbox.len(),
    };

    let sections = [
        headings[0].clone(),
        kernel_body.clone(),
        headings[1].clone(),
        machine_body.clone(),
        headings[2].clone(),
        adapter_body.clone(),
        headings[3].clone(),
        messageboard,
        headings[4].clone(),
        health,
        headings[5].clone(),
        forum_index,
        headings[6].clone(),
        forum_inbox,
    ];
    let payload = assemble_payload(harness, host, budget, &sections.join("\n\n"))?;
    let breakdown = Breakdown {
        budget,
        kernel: kernel_body.len(),
        machine: machine_body.len(),
        adapter: adapter_body.len(),
        frame,
        fixed,
        residual: budget - fixed,
        planned,
        actual,
        total: payload.len(),
    };
    Ok((payload, breakdown))
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
    // Surfaces were renamed to the bare binary name on 2026-09-01; the
    // historical `ai-brief.nu` spelling is still accepted.
    let legacy = needle.replace("ai-brief render", "ai-brief.nu render");
    if raw.contains(needle) || raw.contains(&legacy) {
        Row { harness: label.into(), status: "ok", detail: format!("startup surface {}", path.display()) }
    } else {
        Row { harness: label.into(), status: "FAIL", detail: format!("startup surface does not reference renderer: {}", path.display()) }
    }
}

fn doctor(home: &Path, host: &str, budget: usize) -> Result<()> {
    let mut rows: Vec<Row> = vec![];
    let mut breakdowns: Vec<(String, String)> = vec![];
    for harness in HARNESSES {
        let row = match render_with_breakdown(home, harness, host, budget).and_then(|(p, b)| verify_payload(&p, harness, host).map(|v| (v, b))) {
            Ok((v, b)) => {
                breakdowns.push((harness.to_string(), b.describe()));
                Row { harness: harness.into(), status: "ok", detail: format!("verified {} bytes; sha256: {}", v.bytes, v.hash) }
            }
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

    rows.push(verify_startup_surface("codex", &home.join(".codex/AGENTS.md"), "ai-brief render --harness codex"));
    rows.push(verify_startup_surface(
        "claude-code",
        &home.join(".claude/settings.json"),
        "ai-brief render --harness claude-code --format claude-hook",
    ));
    rows.push(verify_startup_surface("grok-build", &home.join(".grok/AGENTS.md"), "ai-brief render --harness grok-build"));
    rows.push(verify_startup_surface("api", &home.join("Assistants/context/briefings/api.md"), "ai-brief render --harness api"));

    println!("{:<14} {:<6} detail", "harness", "status");
    for r in &rows {
        println!("{:<14} {:<6} {}", r.harness, r.status, r.detail);
    }
    if !breakdowns.is_empty() {
        println!("\nbytes per section (the live surfaces share the residual; a cap below its maximum means the fixed parts have grown):");
        for (harness, text) in &breakdowns {
            println!("{:<14} {text}", harness);
        }
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
        assert_eq!(messageboard_head(&p, MESSAGEBOARD_BUDGET).unwrap(), "### 2026-09-01 — Mac\n\nfirst body\n");
        fs::write(&p, "# Messageboard\n\nnothing yet\n").unwrap();
        assert_eq!(messageboard_head(&p, MESSAGEBOARD_BUDGET).unwrap(), "No current Messageboard entries.");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn forum_open_summary_collects_only_open_rows() {
        let d = temp("forum");
        let p = d.join("INDEX.md");
        fs::write(&p, "# Index\n\n## Open\n\n| id | x |\n| `a` | open-a |\n\n| `b` | open-b |\n\n## Decided\n\n| `c` | decided |\n").unwrap();
        assert_eq!(forum_open_summary(&p, FORUM_INDEX_BUDGET).unwrap(), "| `a` | open-a |\n| `b` | open-b |");
        fs::write(&p, "## Open\n\n## Decided\n| `c` | d |\n").unwrap();
        assert_eq!(forum_open_summary(&p, FORUM_INDEX_BUDGET).unwrap(), "No open forum threads.");
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
    fn host_health_renders_parked_problems_apart_and_never_red() {
        let dir = temp("health-park");
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-23T09:00:00+01:00").unwrap();
        fs::write(
            dir.join("macos.json"),
            r#"{"schema":1,"host":"macos","hostname":"mac","checked_at":"2026-09-23T08:05:00+01:00","count":2,
                "problems":["Agent errored: a [exit 1]","Watcher g: last error: y"],
                "problem_history":[
                  {"text":"Agent errored: a [exit 1]","key":"k1","first_seen":"2026-09-22T08:00:00+01:00","runs":2},
                  {"text":"Watcher g: last error: y","key":"k2","first_seen":"2026-09-23T08:05:00+01:00","runs":1},
                  {"text":"Service failed: mailcurator-drift","key":"k3","first_seen":"2026-09-20T09:00:00+01:00","runs":4}
                ],
                "parked":[{"key":"k3","text":"Service failed: mailcurator-drift","until":"2026-09-30","reason":"coverage work queued","parked_at":"2026-09-23T01:40:00+01:00"}]}"#,
        )
        .unwrap();
        fs::write(
            dir.join("nimbini.json"),
            r#"{"schema":1,"host":"nimbini","hostname":"nimbini","checked_at":"2026-09-23T08:00:00+01:00","count":0,
                "problems":[],"problem_history":[{"text":"Service failed: x","key":"k9","first_seen":"2026-09-21T08:00:00+01:00","runs":3}],
                "parked":[{"key":"k9","text":"Service failed: x","until":"2026-10-01","reason":"awaiting vendor","parked_at":"2026-09-23T01:40:00+01:00"}]}"#,
        )
        .unwrap();
        let out = host_health_summary_at(&dir, now, HEALTH_BUDGET);
        assert!(out.contains("macos: 🚨 2 problems, 1 parked"), "{out}");
        assert!(out.contains("  ⏸ Service failed: mailcurator-drift — parked until 30 Sep: coverage work queued [since 20 Sep, 4×]"), "{out}");
        assert!(out.contains("nimbini: ✅ clean (1 parked)"), "{out}");
        assert!(out.contains("  ⏸ Service failed: x — parked until 1 Oct: awaiting vendor [since 21 Sep, 3×]"), "{out}");
        // The parked line comes after the active ones.
        assert!(out.find("Watcher g").unwrap() < out.find("⏸ Service failed: mailcurator").unwrap());
    }

    #[test]
    fn host_health_renders_problem_age_when_history_present() {
        let dir = temp("health-age");
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-22T09:00:00+01:00").unwrap();
        fs::write(
            dir.join("macos.json"),
            r#"{"schema":1,"host":"macos","hostname":"mac","checked_at":"2026-09-22T08:03:00+01:00","count":2,
                "problems":["Watcher f: last error: x","Watcher g: last cycle 16 min ago — dead or hung"],
                "problem_history":[
                  {"text":"Watcher f: last error: x","key":"k1","first_seen":"2026-09-15T08:03:00+01:00","runs":8},
                  {"text":"Watcher g: last cycle 16 min ago — dead or hung","key":"k2","first_seen":"2026-09-22T08:03:00+01:00","runs":1}
                ]}"#,
        )
        .unwrap();
        let out = host_health_summary_at(&dir, now, HEALTH_BUDGET);
        assert!(out.contains("  - Watcher f: last error: x [since 15 Sep, 8×]"), "{out}");
        assert!(out.contains("dead or hung [new]"), "{out}");
    }

    #[test]
    fn host_health_renders_fresh_stale_unreadable_and_missing() {
        let d = temp("health");
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-01T23:00:00+01:00").unwrap();
        assert!(host_health_summary_at(&d.join("nope"), now, HEALTH_BUDGET).starts_with("WARNING: no host health status"));
        fs::write(d.join("macos.json"), r#"{"host":"macos","checked_at":"2026-09-01T22:50:00+01:00","count":2,"problems":["p1","p2"]}"#).unwrap();
        fs::write(d.join("nimbini.json"), r#"{"host":"nimbini","checked_at":"2026-08-25T08:00:00+01:00","count":0,"problems":[]}"#).unwrap();
        fs::write(d.join("zz.json"), "not json").unwrap();
        let out = host_health_summary_at(&d, now, HEALTH_BUDGET);
        assert_eq!(
            out,
            "macos: 🚨 2 problems (checked 10m ago)\n  - p1\n  - p2\nnimbini: ✅ clean — STALE (last check 183h ago; the health check itself may be dead)\nzz.json: unreadable status file"
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn live_budgets_take_their_maxima_when_the_residual_allows_and_scale_when_not() {
        let max = LiveBudgets::MAXIMA;
        assert_eq!(plan_live_budgets(5_000, 18_000).unwrap(), max);
        // 9,000 fixed leaves 9,000 for 11,200 of maxima: everything scales down, sum stays inside.
        let p = plan_live_budgets(9_000, 18_000).unwrap();
        assert!(p.total() <= 9_000, "{p:?}");
        assert!(p.messageboard < max.messageboard && p.forum_index < max.forum_index && p.forum_inbox < max.forum_inbox && p.health < max.health);
        assert!(p.messageboard > p.forum_index && p.forum_index > p.forum_inbox && p.forum_inbox > p.health, "proportions kept: {p:?}");
        // A tiny residual still leaves every surface its floor.
        let p = plan_live_budgets(17_500, 18_000).unwrap();
        assert!(p.messageboard >= LIVE_FLOOR && p.health >= LIVE_FLOOR);
        // Fixed parts alone over the budget: the genuine failure.
        let e = plan_live_budgets(18_001, 18_000).unwrap_err().to_string();
        assert!(e.contains("fixed parts"), "{e}");
    }

    /// A synthetic home where every live surface is far bigger than its cap:
    /// the render must still fit the budget, and a kernel bigger than the
    /// budget must still fail.
    fn synthetic_home(name: &str, kernel_filler: usize) -> PathBuf {
        let d = temp(name);
        fs::create_dir_all(d.join("Assistants/shared/design-forum")).unwrap();
        fs::create_dir_all(d.join("Assistants/context/machines")).unwrap();
        fs::create_dir_all(d.join("Assistants/context/briefings")).unwrap();
        fs::write(
            d.join("Assistants/shared/ORIENTATION.md"),
            format!("---\norientation_schema: 1\nrender_budget_bytes: 18000\n---\n\n# Kernel\n\n{}\n", "k".repeat(kernel_filler)),
        )
        .unwrap();
        fs::write(d.join("Assistants/context/machines/macos.md"), format!("# macos\n\n{}\n", "m".repeat(1_100))).unwrap();
        fs::write(d.join("Assistants/context/briefings/codex.md"), format!("# codex\n\n{}\n", "a".repeat(1_100))).unwrap();
        let entries: String = (0..40).map(|i| format!("\n### 2026-09-{:02} — Mac\n\n{}\n\n---\n", 1 + i % 28, "b".repeat(if i == 0 { 6_000 } else { 400 }))).collect();
        fs::write(d.join("Assistants/shared/MESSAGEBOARD.md"), format!("# Messageboard\n\nintro\n{entries}")).unwrap();
        let rows: String = (0..60).map(|i| format!("| `t{i}` | {} |\n", "o".repeat(120))).collect();
        fs::write(d.join("Assistants/shared/design-forum/INDEX.md"), format!("# Index\n\n## Open\n\n{rows}\n## Decided\n")).unwrap();
        d
    }

    #[test]
    fn render_fits_the_budget_when_every_live_surface_is_full() {
        // 6.5 KB kernel like the real one: fixed ≈ 9 KB, so the live surfaces must share ≈ 9 KB.
        let d = synthetic_home("full", 6_500);
        let (payload, b) = render_with_breakdown(&d, "codex", "macos", 18_000).unwrap();
        assert!(payload.len() <= 18_000, "{} bytes", payload.len());
        assert_eq!(b.total, payload.len());
        assert!(b.planned.total() <= b.residual, "{b:?}");
        assert!(b.actual.messageboard <= b.planned.messageboard && b.actual.forum_index <= b.planned.forum_index);
        assert!(payload.contains("[Messageboard head truncated at startup"));
        assert!(payload.contains("[forum index summary truncated at startup"));
        verify_payload(&payload, "codex", "macos").unwrap();
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn render_still_fails_when_the_kernel_alone_exceeds_the_budget() {
        let d = synthetic_home("bigkernel", 18_500);
        let e = render_contract(&d, "codex", "macos", 18_000).unwrap_err().to_string();
        assert!(e.contains("fixed parts"), "{e}");
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
