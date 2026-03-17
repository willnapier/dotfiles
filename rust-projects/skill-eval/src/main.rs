use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

mod config;
mod evaluate;
mod invoke;
mod log_parser;
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

    for round in 1..=rounds {
        println!("--- Round {}/{} ---", round, rounds);

        let skill_md = std::fs::read_to_string(&skill_md_path)
            .context("Failed to read SKILL.md")?;

        let failure_summary: Vec<String> = current_failures
            .iter()
            .map(|f| format!("  {} [{}]: {}", f.assertion_id, f.assertion_text, f.reason))
            .collect();

        let proposal = propose_edit(&skill_md, &failure_summary)?;

        if proposal.trim().is_empty() || proposal.contains("NO_CHANGE") {
            println!("  LLM proposed no changes. Stopping.\n");
            break;
        }

        let backup = skill_md.clone();
        println!("  Applying proposed edit...");
        std::fs::write(&skill_md_path, &proposal)
            .context("Failed to write SKILL.md")?;

        let new_results = run_scenarios(cli_name, skill, &scenarios, &assertions, 1)?;
        let new_score = tally(&new_results);
        println!("  New score: {} (was {})", format_score(&new_score), format_score(&current_score));

        let new_pct = pct(&new_score);
        let cur_pct = pct(&current_score);
        if new_pct > cur_pct || (new_pct == cur_pct && new_score.0 > current_score.0) {
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

/// Ask the LLM to propose a single edit to SKILL.md to fix the worst failures
fn propose_edit(current_skill_md: &str, failures: &[String]) -> Result<String> {
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

## Current SKILL.md
{}

## Assertion Failures
{}

Output the updated SKILL.md:"#,
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
