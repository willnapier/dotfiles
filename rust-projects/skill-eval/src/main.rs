use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
        } => cmd_run(&cli_name, &skill, scenario.as_deref(), runs),
        Commands::Score { log, skill } => cmd_score(&log, &skill),
        Commands::List { skill } => cmd_list(&skill),
        Commands::Improve {
            cli: cli_name,
            skill,
            rounds,
        } => cmd_improve(&cli_name, &skill, rounds),
    }
}

fn cmd_run(cli_name: &str, skill: &str, scenario_filter: Option<&str>, runs: usize) -> Result<()> {
    let skill_dir = config::skill_dir(skill)?;
    let assertions = config::load_all_assertions(&skill_dir)?;
    let scenarios = config::load_scenarios(&skill_dir)?;

    let scenarios: Vec<_> = if let Some(filter) = scenario_filter {
        scenarios.into_iter().filter(|s| s.id == filter).collect()
    } else {
        scenarios
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

    let mut all_results = Vec::new();

    for scenario in &scenarios {
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

            let results = evaluate::score(&log_entries, &relevant, &scenario.prompt)?;
            all_results.extend(results.clone());

            report::print_scenario_results(&scenario.id, &results);
        }
    }

    println!();
    report::print_summary(&all_results);

    Ok(())
}

fn cmd_score(log_path: &PathBuf, skill: &str) -> Result<()> {
    let skill_dir = config::skill_dir(skill)?;
    let assertions = config::load_all_assertions(&skill_dir)?;

    let log_entries = log_parser::parse_log(log_path, "claude")
        .context("Failed to parse conversation log")?;

    let results = evaluate::score(
        &log_entries,
        &assertions.iter().collect::<Vec<_>>(),
        "(full session)",
    )?;

    report::print_scenario_results("full-session", &results);
    println!();
    report::print_summary(&results);

    Ok(())
}

fn cmd_list(skill: &str) -> Result<()> {
    let skill_dir = config::skill_dir(skill)?;
    let assertions = config::load_all_assertions(&skill_dir)?;

    println!("Assertions for skill '{}':", skill);
    println!();

    let mut current_layer = None;
    for a in &assertions {
        let layer_label = if a.id.starts_with('U') {
            "Universal"
        } else if a.id.starts_with('Q') {
            "Layer 2 (Quality)"
        } else {
            "Layer 1 (Compliance)"
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

fn cmd_improve(_cli_name: &str, _skill: &str, rounds: usize) -> Result<()> {
    println!("Improvement loop: {} rounds", rounds);
    println!("(Not yet implemented -- requires SKILL.md edit + re-eval cycle)");
    Ok(())
}
