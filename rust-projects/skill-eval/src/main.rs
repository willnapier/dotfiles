use anyhow::{Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

mod config;
mod evaluate;
mod invoke;
mod log_parser;
mod propose;
mod report;

#[derive(Parser)]
#[command(name = "skill-eval", about = "Vendor-neutral skill evaluation runner")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run eval assertions against a live AI CLI session
    Run {
        /// AI CLI to use: claude, gemini, codex, goose
        #[arg(long, default_value = "claude")]
        cli: String,

        /// Skill name (directory under ~/.claude/skills/)
        #[arg(long)]
        skill: String,

        /// Run only a specific scenario by ID
        #[arg(long)]
        scenario: Option<String>,

        /// Number of test runs per scenario (default 1)
        #[arg(long, default_value = "1")]
        runs: usize,

        /// Include scenarios with side effects (SSH, skill edits)
        #[arg(long)]
        include_unsafe: bool,
    },

    /// Score assertions against an existing conversation log
    Score {
        /// Path to conversation JSONL log file
        #[arg(long)]
        log: PathBuf,

        /// Skill name (to find assertion files)
        #[arg(long)]
        skill: String,
    },

    /// List assertions for a skill
    List {
        /// Skill name
        #[arg(long)]
        skill: String,
    },

    /// Mine structural patterns from LLM evaluations to propose mechanical checks
    ProposeChecks {
        /// AI CLI to use
        #[arg(long, default_value = "claude")]
        cli: String,

        /// Skill name
        #[arg(long)]
        skill: String,

        /// Assertion ID to analyze (e.g. CQ2)
        #[arg(long)]
        assertion: String,

        /// Number of samples to collect
        #[arg(long, default_value = "10")]
        samples: usize,

        /// Include scenarios with side effects (SSH, skill edits)
        #[arg(long)]
        include_unsafe: bool,
    },

    /// Run the self-improvement loop (Karpathy autoresearch)
    Improve {
        /// AI CLI to use
        #[arg(long, default_value = "claude")]
        cli: String,

        /// Skill name
        #[arg(long)]
        skill: String,

        /// Maximum improvement rounds
        #[arg(long, default_value = "5")]
        rounds: usize,

        /// Include scenarios with side effects (SSH, skill edits)
        #[arg(long)]
        include_unsafe: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            cli: cli_name,
            skill,
            scenario,
            runs,
            include_unsafe,
        } => cmd_run(&cli_name, &skill, scenario.as_deref(), runs, include_unsafe),
        Commands::Score { log, skill } => cmd_score(&log, &skill),
        Commands::List { skill } => cmd_list(&skill),
        Commands::ProposeChecks {
            cli: cli_name,
            skill,
            assertion,
            samples,
            include_unsafe,
        } => cmd_propose_checks(&cli_name, &skill, &assertion, samples, include_unsafe),
        Commands::Improve {
            cli: cli_name,
            skill,
            rounds,
            include_unsafe,
        } => cmd_improve(&cli_name, &skill, rounds, include_unsafe),
    }
}

/// Run scenarios, score assertions, collect results. Core logic shared by cmd_run and cmd_improve.
fn run_scenarios(
    cli_name: &str,
    skill: &str,
    scenarios: &[config::Scenario],
    assertions: &[config::Assertion],
    runs: usize,
) -> Result<Vec<evaluate::EvalResult>> {
    let mut all_results = Vec::new();

    for scenario in scenarios {
        for run_num in 1..=runs {
            if runs > 1 {
                println!("--- {} (run {}/{}) ---", scenario.id, run_num, runs);
            } else {
                println!("--- {} ---", scenario.id);
            }

            let log_entries = invoke::run_scenario(cli_name, skill, scenario)?;

            let relevant: Vec<_> = assertions
                .iter()
                .filter(|a| scenario.exercises.contains(&a.id))
                .collect();

            let results = evaluate::score(&log_entries, &relevant)?;
            report::print_scenario_results(&scenario.id, &results);
            all_results.extend(results);
        }
    }

    Ok(all_results)
}

fn cmd_run(cli_name: &str, skill: &str, scenario_filter: Option<&str>, runs: usize, include_unsafe: bool) -> Result<()> {
    let skill_dir = config::skill_dir(skill)?;
    let assertions = config::load_all_assertions(&skill_dir)?;
    let scenarios = config::load_scenarios(&skill_dir)?;

    let scenarios: Vec<_> = if let Some(filter) = scenario_filter {
        scenarios.into_iter().filter(|s| s.id == filter).collect()
    } else {
        filter_scenarios(scenarios, include_unsafe)
    };

    if scenarios.is_empty() {
        anyhow::bail!("No matching scenarios found");
    }

    println!(
        "Running {} scenario(s) x {} run(s) against {}",
        scenarios.len(),
        runs,
        cli_name
    );
    println!();

    let all_results = run_scenarios(cli_name, skill, &scenarios, &assertions, runs)?;

    println!();
    report::print_totals(&all_results);

    Ok(())
}

fn cmd_score(log_path: &Path, skill: &str) -> Result<()> {
    let skill_dir = config::skill_dir(skill)?;
    let assertions = config::load_all_assertions(&skill_dir)?;

    let log_entries = log_parser::parse_log(log_path, "claude")
        .context("Failed to parse conversation log")?;

    let results = evaluate::score(
        &log_entries,
        &assertions.iter().collect::<Vec<_>>(),
    )?;

    report::print_scenario_results("full-session", &results);
    println!();
    report::print_totals(&results);

    Ok(())
}

fn cmd_list(skill: &str) -> Result<()> {
    let skill_dir = config::skill_dir(skill)?;
    let assertions = config::load_all_assertions(&skill_dir)?;

    println!("Assertions for skill '{}':", skill);
    println!();

    let mut current_layer = None;
    for a in &assertions {
        let layer_label = match a.id.chars().next() {
            Some('U') => "Universal",
            _ => match a.layer {
                Some(2) => "Layer 2 (Quality)",
                _ => "Layer 1 (Compliance)",
            },
        };

        if current_layer != Some(layer_label) {
            println!("  ## {}", layer_label);
            current_layer = Some(layer_label);
        }

        println!("  {:>4}  [{}] {}", a.id, a.category, a.assert_text);
    }

    let scenarios = config::load_scenarios(&skill_dir)?;
    println!();
    println!("Scenarios ({}):", scenarios.len());
    for s in &scenarios {
        println!("  {:>20}  exercises: {:?}", s.id, s.exercises);
    }

    Ok(())
}

fn cmd_propose_checks(
    cli_name: &str,
    skill: &str,
    assertion_id: &str,
    samples: usize,
    include_unsafe: bool,
) -> Result<()> {
    let skill_dir = config::skill_dir(skill)?;
    let assertions = config::load_all_assertions(&skill_dir)?;
    let scenarios = filter_scenarios(config::load_scenarios(&skill_dir)?, include_unsafe);

    // Find the target assertion
    let target = assertions
        .iter()
        .find(|a| a.id == assertion_id)
        .with_context(|| format!("Assertion '{}' not found", assertion_id))?;

    // Check if already mechanicalized
    let empty_log: Vec<log_parser::LogEntry> = vec![];
    if evaluate::try_mechanical_check(&empty_log, target).is_some() {
        println!(
            "{} is already mechanicalized — nothing to propose.",
            assertion_id
        );
        return Ok(());
    }

    // Find scenarios that exercise this assertion
    let relevant_scenarios: Vec<_> = scenarios
        .iter()
        .filter(|s| s.exercises.contains(&assertion_id.to_string()))
        .collect();

    if relevant_scenarios.is_empty() {
        anyhow::bail!("No scenarios exercise assertion '{}'", assertion_id);
    }

    println!("=== PROPOSE CHECKS: {} ===", assertion_id);
    println!("  Assertion: {}", target.assert_text);
    println!(
        "  Scenarios: {:?}",
        relevant_scenarios
            .iter()
            .map(|s| &s.id)
            .collect::<Vec<_>>()
    );
    println!("  Collecting {} samples...\n", samples);

    // Collect corpus: run scenarios, LLM-evaluate target assertion each time
    let mut corpus: Vec<propose::CorpusSample> = Vec::new();
    let target_refs = vec![target];

    for i in 0..samples {
        let scenario = relevant_scenarios[i % relevant_scenarios.len()];
        println!(
            "  Sample {}/{} (scenario: {})...",
            i + 1,
            samples,
            scenario.id
        );

        let log_entries = invoke::run_scenario(cli_name, skill, scenario)?;
        let results = evaluate::score(&log_entries, &target_refs)?;

        if let Some(result) = results.first() {
            let label = match result.outcome {
                evaluate::EvalOutcome::Pass => "PASS",
                evaluate::EvalOutcome::Fail => "FAIL",
                evaluate::EvalOutcome::NotApplicable => "N/A",
            };
            println!("    {} — {}", label, result.reason);

            corpus.push(propose::CorpusSample {
                entries: log_entries,
                verdict: result.outcome.clone(),
                reason: result.reason.clone(),
            });
        }
    }

    // Filter to applicable samples only
    let applicable: Vec<&propose::CorpusSample> = corpus
        .iter()
        .filter(|s| s.verdict != evaluate::EvalOutcome::NotApplicable)
        .collect();

    let pass_count = applicable
        .iter()
        .filter(|s| s.verdict == evaluate::EvalOutcome::Pass)
        .count();
    let fail_count = applicable
        .iter()
        .filter(|s| s.verdict == evaluate::EvalOutcome::Fail)
        .count();

    println!(
        "\n  Corpus: {} PASS, {} FAIL (out of {} applicable)",
        pass_count,
        fail_count,
        applicable.len()
    );

    if pass_count == 0 || fail_count == 0 {
        println!("  Need both PASS and FAIL examples to find a discriminating pattern.");
        println!(
            "  All applicable samples were {}. Try more samples or different scenarios.",
            if pass_count == 0 { "FAIL" } else { "PASS" }
        );
        return Ok(());
    }

    // Build summaries for the analyzer (cap at 5 per class to fit context)
    let pass_summaries: Vec<String> = corpus
        .iter()
        .filter(|s| s.verdict == evaluate::EvalOutcome::Pass)
        .take(5)
        .map(|s| evaluate::build_log_summary(&s.entries))
        .collect();
    let fail_summaries: Vec<String> = corpus
        .iter()
        .filter(|s| s.verdict == evaluate::EvalOutcome::Fail)
        .take(5)
        .map(|s| evaluate::build_log_summary(&s.entries))
        .collect();

    println!("\n  Analyzing patterns...");
    let prompt = propose::build_analysis_prompt(target, &pass_summaries, &fail_summaries);

    let output = std::process::Command::new("claude")
        .env_remove("ANTHROPIC_API_KEY")
        .arg("-p")
        .arg(&prompt)
        .arg("--dangerously-skip-permissions")
        .arg("--no-session-persistence")
        .output()
        .context("Failed to invoke analyzer LLM")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Analyzer LLM failed: {}", stderr);
    }

    let response = String::from_utf8_lossy(&output.stdout);
    let proposal = propose::parse_proposal(&response, assertion_id)?;

    println!("\n  Proposed check:");
    println!("    Description: {}", proposal.description);
    println!("    Rationale: {}", proposal.rationale);
    println!(
        "    Spec: {}",
        serde_json::to_string(&proposal.spec).unwrap_or_default()
    );

    if matches!(proposal.spec, propose::CheckSpec::Custom { .. }) {
        println!("\n  Result: No mechanical check possible (subjective/quality judgment).");
        println!("  Assertion stays with LLM evaluator.");
        save_proposal(&skill_dir, &proposal, None)?;
    } else {
        println!("\n  Validating against {} samples...", applicable.len());
        let validation = propose::validate(&proposal, &corpus);

        println!(
            "  Agreement: {}/{} ({:.0}%)",
            validation.agreements,
            validation.agreements + validation.disagreements,
            validation.agreement_rate * 100.0
        );

        for detail in &validation.details {
            if !detail.agrees {
                println!(
                    "    Sample {}: mechanical={} llm={}",
                    detail.sample_index + 1,
                    detail.mechanical,
                    detail.llm
                );
            }
        }

        if validation.agreement_rate >= 0.9 {
            println!("\n  Agreement >= 90% — this check is a viable replacement.");
            println!(
                "  Add to evaluate.rs try_mechanical_check() for assertion {}.",
                assertion_id
            );
        } else {
            println!("\n  Agreement < 90% — check is not reliable enough.");
            println!("  Assertion stays with LLM evaluator.");
        }

        save_proposal(&skill_dir, &proposal, Some(&validation))?;
    }

    Ok(())
}

fn save_proposal(
    skill_dir: &Path,
    proposal: &propose::ProposedCheck,
    validation: Option<&propose::ValidationResult>,
) -> Result<()> {
    let path = skill_dir.join("eval").join("proposed_checks.json");

    let mut proposals: Vec<serde_json::Value> = if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        Vec::new()
    };

    let mut entry = serde_json::json!({
        "assertion_id": proposal.assertion_id,
        "spec": proposal.spec,
        "description": proposal.description,
        "rationale": proposal.rationale,
        "timestamp": Utc::now().to_rfc3339(),
    });

    if let Some(v) = validation {
        entry["agreement_rate"] =
            serde_json::json!(format!("{:.0}%", v.agreement_rate * 100.0));
        entry["agreements"] = serde_json::json!(v.agreements);
        entry["disagreements"] = serde_json::json!(v.disagreements);
    }

    // Replace existing proposal for same assertion
    proposals.retain(|p| {
        p.get("assertion_id").and_then(|a| a.as_str()) != Some(&proposal.assertion_id)
    });
    proposals.push(entry);

    let json = serde_json::to_string_pretty(&proposals)?;
    std::fs::write(&path, json).context("Failed to write proposed_checks.json")?;
    println!("  Saved to eval/proposed_checks.json");

    Ok(())
}

fn cmd_improve(cli_name: &str, skill: &str, rounds: usize, include_unsafe: bool) -> Result<()> {
    let skill_dir = config::skill_dir(skill)?;
    let skill_md_path = skill_dir.join("SKILL.md");

    if !skill_md_path.exists() {
        anyhow::bail!("SKILL.md not found at {}", skill_md_path.display());
    }

    let assertions = config::load_all_assertions(&skill_dir)?;
    let scenarios = filter_scenarios(config::load_scenarios(&skill_dir)?, include_unsafe);

    println!("=== IMPROVEMENT LOOP: {} rounds against {} ===\n", rounds, cli_name);

    // Run baseline
    println!("--- Baseline ---");
    let baseline_results = run_scenarios(cli_name, skill, &scenarios, &assertions, 1)?;
    let baseline_score = tally(&baseline_results);
    println!("  Baseline score: {}\n", format_score(&baseline_score));

    let baseline_failures = failures_from(&baseline_results);
    if baseline_failures.is_empty() {
        println!("No failures to improve. Score is already perfect.");
        return Ok(());
    }

    let mut current_score = baseline_score;
    let mut current_failures = baseline_failures;

    // Check for round memory — previously tried edits
    let prior_edit_count = load_tried_edits(&skill_dir).len();
    if prior_edit_count > 0 {
        println!("  Loaded {} previously tried edits from history\n", prior_edit_count);
    }

    for round in 1..=rounds {
        println!("--- Round {}/{} ---", round, rounds);

        let skill_md = std::fs::read_to_string(&skill_md_path)
            .context("Failed to read SKILL.md")?;

        let failure_summary: Vec<String> = current_failures
            .iter()
            .map(|f| format!("  {} [{}]: {}", f.assertion_id, f.assertion_text, f.reason))
            .collect();

        // Include history of previously tried edits
        let history = load_tried_edits(&skill_dir);
        let proposal = propose_edit(&skill_md, &failure_summary, &history)?;

        if proposal.trim().is_empty() || proposal.contains("NO_CHANGE") {
            println!("  LLM proposed no changes. Stopping.\n");
            break;
        }

        let backup = skill_md.clone();
        println!("  Applying proposed edit...");
        std::fs::write(&skill_md_path, &proposal)
            .context("Failed to write SKILL.md")?;

        // Generate a diff summary for the edit record
        let edit_summary = generate_edit_summary(&backup, &proposal);

        let new_results = run_scenarios(cli_name, skill, &scenarios, &assertions, 1)?;
        let new_score = tally(&new_results);
        println!("  New score: {} (was {})", format_score(&new_score), format_score(&current_score));

        let new_pct = pct(&new_score);
        let cur_pct = pct(&current_score);
        let (result_label, kept) = if new_pct > cur_pct || (new_pct == cur_pct && new_score.0 > current_score.0) {
            ("kept", true)
        } else {
            ("reverted", false)
        };

        // Record this edit in history
        save_tried_edit(&skill_dir, TriedEdit {
            round,
            timestamp: Utc::now().to_rfc3339(),
            summary: edit_summary,
            result: result_label.to_string(),
            score_before: format_score(&current_score),
            score_after: format_score(&new_score),
        })?;

        if kept {
            println!("  KEPT — score improved.\n");
            current_score = new_score;
            current_failures = failures_from(&new_results);
        } else {
            println!("  REVERTED — no improvement.\n");
            std::fs::write(&skill_md_path, &backup)
                .context("Failed to revert SKILL.md")?;
        }

        if current_failures.is_empty() {
            println!("All assertions passing. Stopping early.\n");
            break;
        }
    }

    println!("=== FINAL ===");
    println!("  Score: {} (started at {})", format_score(&current_score), format_score(&baseline_score));

    Ok(())
}

/// Filter out scenarios with side effects unless --include-unsafe is set
fn filter_scenarios(scenarios: Vec<config::Scenario>, include_unsafe: bool) -> Vec<config::Scenario> {
    if include_unsafe {
        return scenarios;
    }
    let (safe, skipped): (Vec<_>, Vec<_>) = scenarios
        .into_iter()
        .partition(|s| s.side_effects.is_empty());
    for s in &skipped {
        eprintln!(
            "  Skipping {} (side effects: {:?}) — use --include-unsafe to run",
            s.id, s.side_effects
        );
    }
    safe
}

/// A record of an edit tried during an improve round
#[derive(Debug, Serialize, Deserialize)]
struct TriedEdit {
    round: usize,
    timestamp: String,
    summary: String,
    result: String,
    score_before: String,
    score_after: String,
}

/// Load tried edits history from the skill's eval directory
fn load_tried_edits(skill_dir: &Path) -> Vec<TriedEdit> {
    let path = skill_dir.join("eval").join("tried_edits.json");
    if !path.exists() {
        return Vec::new();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Append a tried edit to the history file
fn save_tried_edit(skill_dir: &Path, edit: TriedEdit) -> Result<()> {
    let path = skill_dir.join("eval").join("tried_edits.json");
    let mut edits = load_tried_edits(skill_dir);
    edits.push(edit);
    let json = serde_json::to_string_pretty(&edits)?;
    std::fs::write(&path, json).context("Failed to write tried_edits.json")?;
    Ok(())
}

/// Format tried edits as context for the proposer prompt
fn format_tried_edits_context(edits: &[TriedEdit]) -> String {
    if edits.is_empty() {
        return String::new();
    }

    let mut ctx = String::from("\n## Previously Tried Edits\nDo NOT repeat these changes:\n");
    for edit in edits {
        ctx.push_str(&format!(
            "- Round {} [{}]: {} (score: {} → {})\n",
            edit.round, edit.result, edit.summary, edit.score_before, edit.score_after
        ));
    }
    ctx
}

/// Tally (passed, failed, total_applicable) from results
fn tally(results: &[evaluate::EvalResult]) -> (usize, usize, usize) {
    let applicable = results.iter().filter(|r| r.is_applicable()).count();
    let passed = results.iter().filter(|r| r.is_applicable() && r.passed()).count();
    (passed, applicable - passed, applicable)
}

fn failures_from(results: &[evaluate::EvalResult]) -> Vec<evaluate::EvalResult> {
    results
        .iter()
        .filter(|r| r.outcome == evaluate::EvalOutcome::Fail)
        .cloned()
        .collect()
}

fn pct(score: &(usize, usize, usize)) -> f64 {
    if score.2 > 0 {
        (score.0 as f64 / score.2 as f64) * 100.0
    } else {
        0.0
    }
}

fn format_score(score: &(usize, usize, usize)) -> String {
    let pct = if score.2 > 0 {
        (score.0 as f64 / score.2 as f64) * 100.0
    } else {
        0.0
    };
    format!("{}/{} ({:.0}%)", score.0, score.2, pct)
}

/// Extract SKILL.md content from LLM output that may contain preamble text,
/// markdown fences, or both. Handles cases like:
///   "Here is the updated SKILL.md:\n\n```markdown\n---\nname: ...\n```"
///   "```\n---\nname: ...\n```"
///   "---\nname: ..."
fn extract_skill_md(raw: &str) -> String {
    let trimmed = raw.trim();

    // Try strip_fences first (handles case where output starts with ```)
    let defenced = log_parser::strip_fences(trimmed);

    // If strip_fences worked and result starts with ---, we're done
    if defenced.starts_with("---") {
        return defenced.to_string();
    }

    // Preamble text exists. Find the frontmatter start.
    // Look for ``` fence containing ---, or bare --- on its own line.
    // Handle: "preamble\n\n```markdown\n---\nname:...\n```"
    // Handle: "preamble\n\n---\nname:..."

    // First try to find a fenced block containing frontmatter
    for fence_marker in ["```markdown\n", "```md\n", "```\n"] {
        if let Some(fence_start) = defenced.find(fence_marker) {
            let content_start = fence_start + fence_marker.len();
            let after_fence = &defenced[content_start..];
            // Strip trailing fence
            let content = if let Some(end) = after_fence.rfind("\n```") {
                &after_fence[..end]
            } else {
                after_fence.trim_end_matches("```")
            };
            let content = content.trim();
            if content.starts_with("---") {
                return content.to_string();
            }
        }
    }

    // No fences — look for bare --- on its own line after preamble
    if let Some(pos) = defenced.find("\n---\n") {
        return defenced[pos + 1..].to_string();
    }

    // Last resort: return as-is
    defenced.to_string()
}

/// Generate a short summary of the diff between two versions of SKILL.md
fn generate_edit_summary(before: &str, after: &str) -> String {
    // Find first differing line and summarize
    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();

    let mut added = 0;
    let mut removed = 0;
    let mut first_change = String::new();

    let max_len = before_lines.len().max(after_lines.len());
    for i in 0..max_len {
        let b = before_lines.get(i).copied().unwrap_or("");
        let a = after_lines.get(i).copied().unwrap_or("");
        if b != a {
            if first_change.is_empty() {
                let snippet = if !a.is_empty() { a } else { b };
                first_change = snippet.chars().take(120).collect();
            }
            if b.is_empty() {
                added += 1;
            } else if a.is_empty() {
                removed += 1;
            } else {
                added += 1;
                removed += 1;
            }
        }
    }

    if added + removed > after_lines.len().abs_diff(before_lines.len()) {
        // Simple line count diff as fallback
        let len_diff = after_lines.len() as isize - before_lines.len() as isize;
        let direction = if len_diff > 0 { "added" } else { "removed" };
        format!("{} lines {} near: {}", len_diff.unsigned_abs(), direction, first_change)
    } else {
        format!("+{}/-{} lines near: {}", added, removed, first_change)
    }
}

/// Ask the LLM to propose a single edit to SKILL.md to fix the worst failures
fn propose_edit(current_skill_md: &str, failures: &[String], history: &[TriedEdit]) -> Result<String> {
    let history_context = format_tried_edits_context(history);

    let prompt = format!(
        r#"You are improving an AI assistant's skill file (SKILL.md). Below is the current file and a list of assertion failures from automated testing.

Your job: make ONE targeted edit to the SKILL.md that would fix the most impactful failure pattern. The edit should be minimal — change or add the fewest lines possible while being clear and specific enough that the AI will follow the instruction.

Rules:
- Output the COMPLETE updated SKILL.md (not a diff)
- Make exactly ONE change (a single addition, modification, or rewrite of a section)
- Do not remove existing instructions that are passing — only add or refine
- Do not add meta-commentary or explanations — just output the file
- The file MUST start with the YAML frontmatter (--- / name / description / ---) exactly as in the original
- NEVER omit the name: or description: fields from the frontmatter
- If you believe no change would help, output exactly: NO_CHANGE
{}
## Current SKILL.md
{}

## Assertion Failures
{}

Output the updated SKILL.md:"#,
        history_context,
        current_skill_md,
        failures.join("\n")
    );

    let output = std::process::Command::new("claude")
        .env_remove("ANTHROPIC_API_KEY")
        .arg("-p")
        .arg(&prompt)
        .arg("--dangerously-skip-permissions")
        .arg("--no-session-persistence")
        .output()
        .context("Failed to invoke proposer LLM")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Proposer LLM failed (exit {}): {}", output.status, stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let result = extract_skill_md(&stdout);

    // Validate: must contain frontmatter with name and description
    if !result.contains("name:") || !result.contains("description:") {
        eprintln!("  Warning: proposer output missing frontmatter fields, treating as NO_CHANGE");
        return Ok("NO_CHANGE".to_string());
    }

    Ok(result)
}
