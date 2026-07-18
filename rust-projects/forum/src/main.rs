use anyhow::{anyhow, bail, Context, Result};
use chrono::Local;
use clap::{Args, Parser, Subcommand, ValueEnum};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

const DEFAULT_ROOT_SUFFIX: &str = "Assistants/shared/design-forum";

#[derive(Parser)]
#[command(
    name = "forum",
    version,
    about = "Orchestrate the shared multi-assistant design forum"
)]
struct Cli {
    /// Forum directory (defaults to ~/Assistants/shared/design-forum)
    #[arg(long, global = true, env = "DESIGN_FORUM_ROOT")]
    root: Option<PathBuf>,

    /// Optional harness configuration TOML
    #[arg(long, global = true, env = "DESIGN_FORUM_CONFIG")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a forum thread and add it to INDEX.md
    Open(OpenArgs),
    /// Append a human or assistant contribution safely
    Post(PostArgs),
    /// Cold-start a panel of headless assistants for one numbered round
    Convene(ConveneArgs),
    /// Run the durable background queue worker
    Worker {
        /// Process available jobs once, then exit
        #[arg(long)]
        once: bool,
        /// Seconds between queue scans in daemon mode
        #[arg(long, default_value_t = 10)]
        poll_seconds: u64,
    },
    /// List queued, running, completed, and failed jobs
    Jobs {
        /// Include completed jobs
        #[arg(long)]
        all: bool,
    },
    /// Cancel a job that has not yet been claimed
    Cancel { job_id: String },
    /// Show one thread's status, participants, and orchestrated rounds
    Status { id: String },
    /// Turn an accepted decision into one bounded implementation work order
    Dispatch(DispatchArgs),
    /// List forum threads from INDEX.md
    List,
    /// Validate paths, harness commands, and the forum index
    Doctor,
}

#[derive(Args)]
struct OpenArgs {
    #[arg(long)]
    id: String,
    #[arg(long)]
    system: String,
    /// Directory under the forum root; defaults to --system (for example: meta)
    #[arg(long)]
    area: Option<String>,
    #[arg(long, value_enum)]
    level: Level,
    #[arg(long)]
    title: String,
    /// Short INDEX.md description; defaults to the title
    #[arg(long)]
    topic: Option<String>,
    /// Initial context text
    #[arg(long, conflicts_with = "context_file")]
    context: Option<String>,
    /// Read initial context from a file
    #[arg(long)]
    context_file: Option<PathBuf>,
    #[arg(long, default_value = "will")]
    opened_by: String,
}

#[derive(Args)]
struct PostArgs {
    id: String,
    #[arg(long)]
    author: String,
    /// Display name in the contribution heading
    #[arg(long)]
    name: Option<String>,
    #[arg(long, value_enum, default_value_t = ContributionKind::Position)]
    kind: ContributionKind,
    /// Markdown contribution body
    #[arg(long, conflicts_with = "body_file")]
    body: Option<String>,
    /// Read contribution body from a file; stdin is used if neither is given
    #[arg(long)]
    body_file: Option<PathBuf>,
    /// Optional person/harness being answered
    #[arg(long)]
    reply_to: Option<String>,
}

#[derive(Args)]
struct ConveneArgs {
    id: String,
    /// Harness initiating the round; excluded by --panel others
    #[arg(long)]
    caller: String,
    /// core, others, all, or a comma-separated harness list
    #[arg(long, default_value = "others")]
    panel: String,
    /// Use a specific round number
    #[arg(long, conflicts_with = "new_round")]
    round: Option<u32>,
    /// Start after the highest recorded round
    #[arg(long)]
    new_round: bool,
    #[arg(long, value_enum)]
    kind: Option<ContributionKind>,
    /// Print planned invocations without calling models or writing the thread
    #[arg(long, conflicts_with = "background")]
    dry_run: bool,
    /// Queue the round for the background worker and return immediately
    #[arg(long)]
    background: bool,
    /// Total attempts before a background job is archived as failed
    #[arg(long, default_value_t = 3)]
    max_attempts: u32,
}

#[derive(Args)]
struct DispatchArgs {
    id: String,
    /// Single implementation owner
    #[arg(long)]
    assignee: String,
    /// One bounded file, system, or responsibility in scope; repeat as needed
    #[arg(long = "scope", required = true)]
    scope: Vec<String>,
    /// One observable completion condition; repeat as needed
    #[arg(long = "acceptance", required = true)]
    acceptance: Vec<String>,
    /// Independent reviewer harness; repeat as needed
    #[arg(long = "reviewer", required = true)]
    reviewers: Vec<String>,
    /// Human or harness authorizing the dispatch
    #[arg(long, default_value = "will")]
    requested_by: String,
    /// Print the exact work order without changing the thread or Messageboard
    #[arg(long)]
    dry_run: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Level {
    Architecture,
    Module,
    Implementation,
    Ops,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Self::Architecture => "architecture",
            Self::Module => "module",
            Self::Implementation => "implementation",
            Self::Ops => "ops",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ContributionKind {
    Position,
    Reply,
}

impl ContributionKind {
    fn heading(self) -> &'static str {
        match self {
            Self::Position => "Position",
            Self::Reply => "Reply",
        }
    }
}

#[derive(Clone, Debug)]
struct Harness {
    id: String,
    display_name: String,
    command: String,
    args: Vec<String>,
    prompt_mode: PromptMode,
    enabled: bool,
}

#[derive(Clone, Copy, Debug)]
enum PromptMode {
    Stdin,
    Argument,
}

#[derive(Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    harnesses: BTreeMap<String, HarnessFile>,
    #[serde(default)]
    panels: BTreeMap<String, Vec<String>>,
}

#[derive(Default, Deserialize)]
struct HarnessFile {
    display_name: Option<String>,
    command: Option<String>,
    args: Option<Vec<String>>,
    prompt_mode: Option<String>,
    enabled: Option<bool>,
}

#[derive(Clone)]
struct Config {
    harnesses: BTreeMap<String, Harness>,
    panels: BTreeMap<String, Vec<String>>,
}

#[derive(Debug)]
struct InvocationResult {
    harness: Harness,
    body: Option<String>,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct QueueJob {
    version: u32,
    job_id: String,
    thread_id: String,
    caller: String,
    panel: String,
    round: u32,
    kind: ContributionKind,
    attempts: u32,
    max_attempts: u32,
    created_at: String,
    next_attempt_at: i64,
    completed_at: Option<String>,
    last_error: Option<String>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let root = cli.root.unwrap_or_else(default_root);
    let config = load_config(cli.config.as_deref())?;

    match cli.command {
        Commands::Open(args) => cmd_open(&root, args),
        Commands::Post(args) => cmd_post(&root, args),
        Commands::Convene(args) => cmd_convene(&root, &config, args),
        Commands::Worker { once, poll_seconds } => cmd_worker(&root, &config, once, poll_seconds),
        Commands::Jobs { all } => cmd_jobs(&root, all),
        Commands::Cancel { job_id } => cmd_cancel(&root, &job_id),
        Commands::Status { id } => cmd_status(&root, &id),
        Commands::Dispatch(args) => cmd_dispatch(&root, args),
        Commands::List => cmd_list(&root),
        Commands::Doctor => cmd_doctor(&root, &config),
    }
}

fn default_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(DEFAULT_ROOT_SUFFIX)
}

fn default_state_root() -> PathBuf {
    dirs::state_dir()
        .or_else(|| dirs::home_dir().map(|p| p.join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".forum-state"))
        .join("forum")
}

fn default_config() -> Config {
    let mut harnesses = BTreeMap::new();
    harnesses.insert(
        "codex".into(),
        Harness {
            id: "codex".into(),
            display_name: "Codex".into(),
            command: "codex".into(),
            args: vec![
                "exec".into(),
                "--ephemeral".into(),
                "--sandbox".into(),
                "read-only".into(),
                "--skip-git-repo-check".into(),
                "-C".into(),
                "{forum_root}".into(),
                "-".into(),
            ],
            prompt_mode: PromptMode::Stdin,
            enabled: true,
        },
    );
    harnesses.insert(
        "claude-code".into(),
        Harness {
            id: "claude-code".into(),
            display_name: "Claude Code".into(),
            command: "claude".into(),
            args: vec![
                "-p".into(),
                "--no-session-persistence".into(),
                "--permission-mode".into(),
                "plan".into(),
            ],
            prompt_mode: PromptMode::Stdin,
            enabled: true,
        },
    );
    harnesses.insert(
        "grok-build".into(),
        Harness {
            id: "grok-build".into(),
            display_name: "Grok Build".into(),
            command: "grok".into(),
            args: vec![
                "--permission-mode".into(),
                "plan".into(),
                "--no-subagents".into(),
                "--disable-web-search".into(),
                "--single".into(),
            ],
            prompt_mode: PromptMode::Argument,
            enabled: true,
        },
    );

    let core = vec!["codex".into(), "claude-code".into(), "grok-build".into()];
    let mut panels = BTreeMap::new();
    panels.insert("core".into(), core.clone());
    panels.insert("all".into(), core);
    Config { harnesses, panels }
}

fn load_config(path: Option<&Path>) -> Result<Config> {
    let mut config = default_config();
    let path = path
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|dir| dir.join("forum/config.toml")));
    let Some(path) = path else { return Ok(config) };
    if !path.exists() {
        return Ok(config);
    }

    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    let file: ConfigFile =
        toml::from_str(&raw).with_context(|| format!("invalid config {}", path.display()))?;
    for (id, overlay) in file.harnesses {
        let existing = config.harnesses.get(&id).cloned();
        let prompt_mode = match overlay.prompt_mode.as_deref() {
            Some("stdin") => PromptMode::Stdin,
            Some("argument") => PromptMode::Argument,
            Some(other) => bail!("invalid prompt_mode {other:?} for harness {id}"),
            None => existing
                .as_ref()
                .map(|h| h.prompt_mode)
                .unwrap_or(PromptMode::Stdin),
        };
        config.harnesses.insert(
            id.clone(),
            Harness {
                id: id.clone(),
                display_name: overlay
                    .display_name
                    .or_else(|| existing.as_ref().map(|h| h.display_name.clone()))
                    .unwrap_or_else(|| id.clone()),
                command: overlay
                    .command
                    .or_else(|| existing.as_ref().map(|h| h.command.clone()))
                    .ok_or_else(|| anyhow!("harness {id} needs a command"))?,
                args: overlay
                    .args
                    .or_else(|| existing.as_ref().map(|h| h.args.clone()))
                    .unwrap_or_default(),
                prompt_mode,
                enabled: overlay
                    .enabled
                    .or_else(|| existing.as_ref().map(|h| h.enabled))
                    .unwrap_or(true),
            },
        );
    }
    config.panels.extend(file.panels);
    Ok(config)
}

fn cmd_open(root: &Path, args: OpenArgs) -> Result<()> {
    validate_id(&args.id)?;
    validate_id(&args.system)?;
    validate_id(&args.opened_by)?;
    validate_single_line("title", &args.title)?;
    if let Some(topic) = &args.topic {
        validate_single_line("topic", topic)?;
    }
    let area = args.area.as_deref().unwrap_or(&args.system);
    validate_id(area)?;
    let _lock = ForumLock::acquire(root)?;
    let index_path = root.join("INDEX.md");
    if !index_path.exists() {
        bail!("forum index not found: {}", index_path.display());
    }
    if resolve_thread(root, &args.id)?.is_some() {
        bail!("thread id already exists: {}", args.id);
    }

    let context = read_text_arg(args.context, args.context_file, false)?
        .unwrap_or_else(|| "Describe the problem, constraints, and relevant evidence here.".into());
    let date = Local::now().format("%Y-%m-%d").to_string();
    let filename = format!("{}-{}.md", date, slugify(&args.title));
    let system_dir = root.join(area);
    fs::create_dir_all(&system_dir)?;
    let thread_path = system_dir.join(filename);
    if thread_path.exists() {
        bail!("thread path already exists: {}", thread_path.display());
    }

    let body = format!(
        "---\nid: {id}\nsystem: {system}\nlevel: {level}\nstatus: open\nopened: {date}\nopened_by: {opened_by}\nparticipants: [{opened_by}]\ndecision: null\nrelated_code: []\nrelated_docs: []\n---\n\n# {title}\n\n## Context\n\n{context}\n\n## Positions\n\n_(awaiting positions)_\n\n## Open questions\n\n- What should change, and what evidence would decide it?\n\n## Decision\n\n_(none yet — awaiting positions/replies and William)_\n\n## Consequences / follow-ups\n\n_(after decision only)_\n",
        id = args.id,
        system = args.system,
        level = args.level.as_str(),
        date = date,
        opened_by = args.opened_by,
        title = args.title,
        context = context.trim(),
    );
    atomic_write(&thread_path, &body)?;

    let relative = thread_path.strip_prefix(root).unwrap_or(&thread_path);
    let topic = args
        .topic
        .unwrap_or_else(|| args.title.clone())
        .replace('|', "\\|");
    let row = format!(
        "| `{}` | {} | {} | [{}]({}) | {} | {} | {} |",
        args.id,
        args.system,
        args.level.as_str(),
        relative.display(),
        relative.display(),
        date,
        args.opened_by,
        topic
    );
    insert_open_index_row(&index_path, &row)?;
    println!("Created {}", thread_path.display());
    println!("Thread id: {}", args.id);
    Ok(())
}

fn cmd_post(root: &Path, args: PostArgs) -> Result<()> {
    validate_id(&args.author)?;
    if let Some(name) = &args.name {
        validate_single_line("name", name)?;
    }
    if let Some(reply_to) = &args.reply_to {
        validate_single_line("reply-to", reply_to)?;
    }
    let body = read_text_arg(args.body, args.body_file, true)?
        .ok_or_else(|| anyhow!("contribution body is empty"))?;
    validate_contribution(&body)?;
    let path = require_thread(root, &args.id)?;
    let _lock = ForumLock::acquire(root)?;
    let mut thread = fs::read_to_string(&path)?;
    let name = args.name.unwrap_or_else(|| display_name_for(&args.author));
    append_contribution(
        &mut thread,
        &args.author,
        &name,
        args.kind,
        args.reply_to.as_deref(),
        &body,
        None,
    )?;
    atomic_write(&path, &thread)?;
    println!(
        "Posted {} by {} to {}",
        args.kind.heading(),
        args.author,
        args.id
    );
    Ok(())
}

fn cmd_convene(root: &Path, config: &Config, args: ConveneArgs) -> Result<()> {
    validate_id(&args.caller)?;
    if args.max_attempts == 0 {
        bail!("max-attempts must be at least 1");
    }
    if args.background {
        return enqueue_convene(root, config, args);
    }
    let path = require_thread(root, &args.id)?;
    let snapshot = fs::read_to_string(&path)?;
    ensure_open(&snapshot)?;
    let round = choose_round(&snapshot, args.round, args.new_round);
    let kind = args.kind.unwrap_or(if round == 1 {
        ContributionKind::Position
    } else {
        ContributionKind::Reply
    });
    let requested = resolve_panel(config, &args.panel, &args.caller)?;
    let pending: Vec<Harness> = requested
        .into_iter()
        .filter(|h| !has_round_contribution(&snapshot, round, &h.id))
        .collect();

    if pending.is_empty() {
        println!("Round {round} already contains every requested harness; nothing to do.");
        return Ok(());
    }
    println!(
        "Thread {} round {}: {}",
        args.id,
        round,
        pending
            .iter()
            .map(|h| h.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if args.dry_run {
        for harness in pending {
            println!("dry-run: {} {}", harness.command, harness.args.join(" "));
        }
        return Ok(());
    }

    let job_dir = create_job_dir(root, &args.id, round)?;
    atomic_write(&job_dir.join("snapshot.md"), &snapshot)?;
    let mut handles = Vec::new();
    for harness in pending {
        let prompt = build_prompt(&args.id, round, kind, &harness, &snapshot);
        atomic_write(&job_dir.join(format!("{}-prompt.md", harness.id)), &prompt)?;
        let root = root.to_path_buf();
        handles.push(thread::spawn(move || {
            invoke_harness(harness, &root, &prompt)
        }));
    }

    let mut results = Vec::new();
    for handle in handles {
        results.push(
            handle
                .join()
                .map_err(|_| anyhow!("harness worker panicked"))?,
        );
    }
    for result in &results {
        let suffix = if result.error.is_some() {
            "error.txt"
        } else {
            "output.md"
        };
        let content = result
            .body
            .as_deref()
            .or(result.error.as_deref())
            .unwrap_or("");
        atomic_write(
            &job_dir.join(format!("{}-{suffix}", result.harness.id)),
            content,
        )?;
    }

    let successes: Vec<&InvocationResult> = results.iter().filter(|r| r.body.is_some()).collect();
    if !successes.is_empty() {
        let _lock = ForumLock::acquire(root)?;
        let mut current = fs::read_to_string(&path)?;
        ensure_open(&current)?;
        for result in successes {
            if has_round_contribution(&current, round, &result.harness.id) {
                continue;
            }
            append_contribution(
                &mut current,
                &result.harness.id,
                &result.harness.display_name,
                kind,
                None,
                result.body.as_deref().unwrap_or(""),
                Some(round),
            )?;
        }
        atomic_write(&path, &current)?;
    }

    let failures: Vec<String> = results
        .iter()
        .filter_map(|r| r.error.as_ref().map(|e| format!("{}: {e}", r.harness.id)))
        .collect();
    println!("Job record: {}", job_dir.display());
    println!(
        "Appended {} contribution(s)",
        results.len() - failures.len()
    );
    if !failures.is_empty() {
        bail!(
            "round partially failed; retry is safe:\n{}",
            failures.join("\n")
        );
    }
    Ok(())
}

fn enqueue_convene(root: &Path, config: &Config, args: ConveneArgs) -> Result<()> {
    let _lock = ForumLock::acquire(root)?;
    let path = require_thread(root, &args.id)?;
    let snapshot = fs::read_to_string(&path)?;
    ensure_open(&snapshot)?;
    let round = choose_round(&snapshot, args.round, args.new_round);
    let kind = args.kind.unwrap_or(if round == 1 {
        ContributionKind::Position
    } else {
        ContributionKind::Reply
    });
    let pending: Vec<Harness> = resolve_panel(config, &args.panel, &args.caller)?
        .into_iter()
        .filter(|h| !has_round_contribution(&snapshot, round, &h.id))
        .collect();
    if pending.is_empty() {
        println!("Round {round} already contains every requested harness; nothing queued.");
        return Ok(());
    }

    ensure_queue_dirs(root)?;
    let now = Local::now();
    let nonce = now
        .timestamp_nanos_opt()
        .unwrap_or_else(|| now.timestamp_micros() * 1_000);
    let job_id = format!("{}-r{}-{}-{}", args.id, round, nonce, std::process::id());
    let job = QueueJob {
        version: 1,
        job_id: job_id.clone(),
        thread_id: args.id,
        caller: args.caller,
        panel: args.panel,
        round,
        kind,
        attempts: 0,
        max_attempts: args.max_attempts,
        created_at: now.to_rfc3339(),
        next_attempt_at: now.timestamp(),
        completed_at: None,
        last_error: None,
    };
    let raw = toml::to_string_pretty(&job)?;
    atomic_write(&queue_dir(root).join(format!("{job_id}.toml")), &raw)?;
    println!("Queued forum job: {job_id}");
    println!(
        "Thread {} round {}: {}",
        job.thread_id,
        round,
        pending
            .iter()
            .map(|h| h.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}

fn cmd_worker(root: &Path, config: &Config, once: bool, poll_seconds: u64) -> Result<()> {
    if poll_seconds == 0 {
        bail!("poll-seconds must be at least 1");
    }
    ensure_queue_dirs(root)?;
    let _worker_lock = WorkerLock::acquire(root)?;
    recover_running_jobs(root)?;
    println!("forum worker active for {}", root.display());
    loop {
        let processed = process_next_job(root, config)?;
        if once {
            if !processed {
                return Ok(());
            }
            continue;
        }
        if !processed {
            thread::sleep(Duration::from_secs(poll_seconds));
        }
    }
}

fn process_next_job(root: &Path, config: &Config) -> Result<bool> {
    let now = Local::now().timestamp();
    let mut candidates = job_files(&queue_dir(root))?;
    candidates.sort();
    let Some((queued_path, mut job)) = candidates.into_iter().find_map(|path| {
        let raw = fs::read_to_string(&path).ok()?;
        let job: QueueJob = toml::from_str(&raw).ok()?;
        (job.next_attempt_at <= now).then_some((path, job))
    }) else {
        return Ok(false);
    };

    let running_path = running_dir(root).join(queued_path.file_name().unwrap_or_default());
    match fs::rename(&queued_path, &running_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    }
    println!("processing forum job {}", job.job_id);

    let result = cmd_convene(
        root,
        config,
        ConveneArgs {
            id: job.thread_id.clone(),
            caller: job.caller.clone(),
            panel: job.panel.clone(),
            round: Some(job.round),
            new_round: false,
            kind: Some(job.kind),
            dry_run: false,
            background: false,
            max_attempts: job.max_attempts,
        },
    );

    match result {
        Ok(()) => {
            job.completed_at = Some(Local::now().to_rfc3339());
            job.last_error = None;
            archive_job(&running_path, &completed_dir(root), &job)?;
            println!("completed forum job {}", job.job_id);
        }
        Err(error) => {
            job.attempts += 1;
            job.last_error = Some(format!("{error:#}"));
            if job.attempts < job.max_attempts {
                let exponent = job.attempts.saturating_sub(1).min(6);
                let delay = 60_i64 * (1_i64 << exponent);
                job.next_attempt_at = Local::now().timestamp() + delay;
                archive_job(&running_path, &queue_dir(root), &job)?;
                eprintln!(
                    "forum job {} failed attempt {}/{}; retry in {}s: {}",
                    job.job_id,
                    job.attempts,
                    job.max_attempts,
                    delay,
                    job.last_error.as_deref().unwrap_or("unknown error")
                );
            } else {
                job.completed_at = Some(Local::now().to_rfc3339());
                archive_job(&running_path, &failed_dir(root), &job)?;
                eprintln!(
                    "forum job {} failed permanently after {} attempt(s): {}",
                    job.job_id,
                    job.attempts,
                    job.last_error.as_deref().unwrap_or("unknown error")
                );
            }
        }
    }
    Ok(true)
}

fn cmd_jobs(root: &Path, all: bool) -> Result<()> {
    ensure_queue_dirs(root)?;
    let mut found = false;
    let states: Vec<(&str, PathBuf)> = vec![
        ("queued", queue_dir(root)),
        ("running", running_dir(root)),
        ("failed", failed_dir(root)),
        ("cancelled", cancelled_dir(root)),
        ("completed", completed_dir(root)),
    ];
    for (state, dir) in states {
        if !all && state == "completed" {
            continue;
        }
        for path in job_files(&dir)? {
            let raw = fs::read_to_string(&path)?;
            let job: QueueJob = toml::from_str(&raw)?;
            println!(
                "{}\t{}\tthread={} round={} attempts={}/{}{}",
                state,
                job.job_id,
                job.thread_id,
                job.round,
                job.attempts,
                job.max_attempts,
                job.last_error
                    .as_deref()
                    .map(|error| format!(" error={}", error.replace('\n', " ")))
                    .unwrap_or_default()
            );
            found = true;
        }
    }
    if !found {
        println!("No forum jobs.");
    }
    Ok(())
}

fn cmd_cancel(root: &Path, job_id: &str) -> Result<()> {
    validate_single_line("job-id", job_id)?;
    ensure_queue_dirs(root)?;
    let source = queue_dir(root).join(format!("{job_id}.toml"));
    if !source.exists() {
        bail!("queued job not found (running jobs cannot be cancelled): {job_id}");
    }
    let destination = cancelled_dir(root).join(format!("{job_id}.toml"));
    fs::rename(source, destination)?;
    println!("Cancelled forum job: {job_id}");
    Ok(())
}

fn archive_job(running_path: &Path, destination_dir: &Path, job: &QueueJob) -> Result<()> {
    let raw = toml::to_string_pretty(job)?;
    atomic_write(running_path, &raw)?;
    let destination = destination_dir.join(running_path.file_name().unwrap_or_default());
    fs::rename(running_path, destination)?;
    Ok(())
}

fn recover_running_jobs(root: &Path) -> Result<()> {
    for path in job_files(&running_dir(root))? {
        let destination = queue_dir(root).join(path.file_name().unwrap_or_default());
        if !destination.exists() {
            fs::rename(&path, destination)?;
        }
    }
    Ok(())
}

fn ensure_queue_dirs(root: &Path) -> Result<()> {
    for dir in [
        queue_dir(root),
        running_dir(root),
        completed_dir(root),
        failed_dir(root),
        cancelled_dir(root),
    ] {
        fs::create_dir_all(dir)?;
    }
    Ok(())
}

fn orchestrator_dir(root: &Path) -> PathBuf {
    root.join(".orchestrator")
}

fn queue_dir(root: &Path) -> PathBuf {
    orchestrator_dir(root).join("queue")
}

fn running_dir(root: &Path) -> PathBuf {
    orchestrator_dir(root).join("running")
}

fn completed_dir(root: &Path) -> PathBuf {
    orchestrator_dir(root).join("completed")
}

fn failed_dir(root: &Path) -> PathBuf {
    orchestrator_dir(root).join("failed")
}

fn cancelled_dir(root: &Path) -> PathBuf {
    orchestrator_dir(root).join("cancelled")
}

fn job_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !dir.exists() {
        return Ok(files);
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) == Some("toml") {
            files.push(path);
        }
    }
    Ok(files)
}

fn cmd_status(root: &Path, id: &str) -> Result<()> {
    let path = require_thread(root, id)?;
    let raw = fs::read_to_string(&path)?;
    println!("Thread: {}", path.display());
    for key in [
        "id",
        "system",
        "level",
        "status",
        "opened",
        "opened_by",
        "participants",
        "decision",
    ] {
        if let Some(value) = frontmatter_value(&raw, key) {
            println!("{key}: {value}");
        }
    }
    let markers = round_markers(&raw);
    if markers.is_empty() {
        println!("orchestrated rounds: none");
    } else {
        for (round, harnesses) in markers {
            println!(
                "round {round}: {}",
                harnesses.into_iter().collect::<Vec<_>>().join(", ")
            );
        }
    }
    Ok(())
}

fn cmd_dispatch(root: &Path, args: DispatchArgs) -> Result<()> {
    validate_id(&args.id)?;
    validate_id(&args.assignee)?;
    validate_id(&args.requested_by)?;
    validate_bounded_items("scope", &args.scope)?;
    validate_bounded_items("acceptance", &args.acceptance)?;

    let mut reviewer_set = BTreeSet::new();
    for reviewer in &args.reviewers {
        validate_id(reviewer)?;
        if reviewer == &args.assignee {
            bail!("reviewer {reviewer} is also the assignee; review must be independent");
        }
        if !reviewer_set.insert(reviewer.clone()) {
            bail!("duplicate reviewer: {reviewer}");
        }
    }

    let _lock = ForumLock::acquire(root)?;
    let path = require_thread(root, &args.id)?;
    let mut thread = fs::read_to_string(&path)?;
    let status = frontmatter_value(&thread, "status")
        .ok_or_else(|| anyhow!("thread lacks status frontmatter"))?;
    if status != "decided" {
        bail!("thread is {status}, not decided; only accepted decisions can be dispatched");
    }
    let decision = frontmatter_value(&thread, "decision")
        .map(|value| unquote_frontmatter(&value))
        .filter(|value| !value.is_empty() && value != "null")
        .ok_or_else(|| anyhow!("decided thread lacks a non-null decision summary"))?;
    let dispatch_marker = format!("<!-- forum-dispatch:{} -->", args.id);
    if thread.contains(&dispatch_marker) {
        bail!("thread {} has already been dispatched", args.id);
    }

    let relative = path.strip_prefix(root).unwrap_or(&path);
    let work_order = build_work_order(
        &args.id,
        &decision,
        &args.assignee,
        &args.requested_by,
        &args.scope,
        &args.acceptance,
        &args.reviewers,
        relative,
    );
    if args.dry_run {
        println!("{work_order}");
        return Ok(());
    }

    let messageboard_marker = format!("<!-- forum-work-order:{} -->", args.id);
    let messageboard_path = root
        .parent()
        .ok_or_else(|| anyhow!("forum root has no shared-directory parent"))?
        .join("MESSAGEBOARD.md");
    let already_posted = messageboard_path.is_file()
        && fs::read_to_string(&messageboard_path)?.contains(&messageboard_marker);
    if !already_posted {
        post_messageboard_work_order(&work_order)?;
    }

    let receipt = build_dispatch_receipt(
        &args.id,
        &args.assignee,
        &args.requested_by,
        &args.scope,
        &args.acceptance,
        &args.reviewers,
    );
    if !thread.ends_with('\n') {
        thread.push('\n');
    }
    thread.push_str(&receipt);
    atomic_write(&path, &thread)?;

    if already_posted {
        println!("Recovered existing Messageboard work order for {}", args.id);
    } else {
        println!("Posted bounded Messageboard work order for {}", args.id);
    }
    println!("Assigned to: {}", args.assignee);
    println!("Dispatch receipt: {}", path.display());
    Ok(())
}

fn validate_bounded_items(label: &str, items: &[String]) -> Result<()> {
    if items.is_empty() {
        bail!("at least one {label} item is required");
    }
    for item in items {
        validate_single_line(label, item)?;
        if item.len() > 500 {
            bail!("{label} item exceeds 500 characters");
        }
    }
    Ok(())
}

fn unquote_frontmatter(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
        .replace("\\\"", "\"")
        .trim()
        .to_string()
}

fn build_work_order(
    id: &str,
    decision: &str,
    assignee: &str,
    requested_by: &str,
    scope: &[String],
    acceptance: &[String],
    reviewers: &[String],
    relative_thread: &Path,
) -> String {
    let scope = markdown_list(scope);
    let acceptance = markdown_list(acceptance);
    let reviewers = reviewers
        .iter()
        .map(|reviewer| format!("`{reviewer}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "FORUM WORK ORDER — `{id}`\n\n\
**Accepted decision:** {decision}\n\n\
**Implementation owner:** `{assignee}`  \n\
**Requested by:** `{requested_by}`\n\n\
**Bounded scope:**\n{scope}\n\n\
**Acceptance criteria:**\n{acceptance}\n\n\
**Independent reviewers:** {reviewers}\n\n\
**Decision record:** `design-forum/{}`\n\n\
Implement only the accepted decision and bounded scope above. Debate does not reopen during implementation; material ambiguity or scope expansion returns to the forum or William. Record shipped work in ASSISTANT-HANDOFF and obtain independent review before treating the work order as complete.\n\n\
<!-- forum-work-order:{id} -->",
        relative_thread.display()
    )
}

fn build_dispatch_receipt(
    id: &str,
    assignee: &str,
    requested_by: &str,
    scope: &[String],
    acceptance: &[String],
    reviewers: &[String],
) -> String {
    let date = Local::now().format("%Y-%m-%d");
    let reviewers = reviewers
        .iter()
        .map(|reviewer| format!("`{reviewer}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "\n### Implementation dispatch — {date}\n\n\
- **Owner:** `{assignee}`\n\
- **Requested by:** `{requested_by}`\n\
- **Scope:** {}\n\
- **Acceptance:** {}\n\
- **Independent reviewers:** {reviewers}\n\n\
<!-- forum-dispatch:{id} -->\n",
        scope.join("; "),
        acceptance.join("; ")
    )
}

fn markdown_list(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn post_messageboard_work_order(work_order: &str) -> Result<()> {
    if !command_exists("messageboard-edit") {
        bail!("messageboard-edit is not available on PATH");
    }
    let output = Command::new("messageboard-edit")
        .arg("insert")
        .arg(work_order)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("failed to run messageboard-edit")?;
    if !output.status.success() {
        bail!(
            "messageboard-edit failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn cmd_list(root: &Path) -> Result<()> {
    let index = fs::read_to_string(root.join("INDEX.md"))?;
    let mut in_open = false;
    for line in index.lines() {
        if line == "## Open" {
            in_open = true;
            continue;
        }
        if in_open && line.starts_with("## ") {
            break;
        }
        if in_open && line.starts_with("| `") {
            println!("{line}");
        }
    }
    Ok(())
}

fn cmd_doctor(root: &Path, config: &Config) -> Result<()> {
    let mut failed = false;
    for required in ["INDEX.md", "PROTOCOL.md"] {
        let path = root.join(required);
        let ok = path.is_file();
        println!("{} {}", if ok { "ok" } else { "MISSING" }, path.display());
        failed |= !ok;
    }
    for harness in config.harnesses.values().filter(|h| h.enabled) {
        let found = command_exists(&harness.command);
        println!(
            "{} harness {} -> {}",
            if found { "ok" } else { "MISSING" },
            harness.id,
            harness.command
        );
        failed |= !found;
    }
    if failed {
        bail!("doctor found missing requirements");
    }
    Ok(())
}

fn invoke_harness(harness: Harness, root: &Path, prompt: &str) -> InvocationResult {
    let args: Vec<String> = harness
        .args
        .iter()
        .map(|arg| arg.replace("{forum_root}", &root.to_string_lossy()))
        .collect();
    let mut command = Command::new(&harness.command);
    command
        .args(args)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match harness.prompt_mode {
        PromptMode::Stdin => {
            command.stdin(Stdio::piped());
        }
        PromptMode::Argument => {
            command.arg(prompt);
        }
    }

    let output = if matches!(harness.prompt_mode, PromptMode::Stdin) {
        command.spawn().and_then(|mut child| {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(prompt.as_bytes())?;
            }
            child.wait_with_output()
        })
    } else {
        command.output()
    };

    match output {
        Ok(output) if output.status.success() => {
            let raw = String::from_utf8_lossy(&output.stdout);
            match clean_model_output(&raw) {
                Ok(body) => InvocationResult {
                    harness,
                    body: Some(body),
                    error: None,
                },
                Err(error) => InvocationResult {
                    harness,
                    body: None,
                    error: Some(error.to_string()),
                },
            }
        }
        Ok(output) => InvocationResult {
            harness,
            body: None,
            error: Some(format!(
                "exit {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )),
        },
        Err(error) => InvocationResult {
            harness,
            body: None,
            error: Some(error.to_string()),
        },
    }
}

fn build_prompt(
    id: &str,
    round: u32,
    kind: ContributionKind,
    harness: &Harness,
    snapshot: &str,
) -> String {
    format!(
        "You are {name} participating in William's vendor-neutral Design Forum.\n\n\
Thread: {id}\nRound: {round}\nContribution type: {kind}\n\n\
Read the complete snapshot below. Produce an independent, substantive contribution. State a clear claim, use evidence from the snapshot or named paths, identify risks and alternatives, and say what would change if accepted. For a reply round, engage the strongest existing claims rather than merely agreeing. Stay PHI-free. Debate only: do not implement, invoke tools, edit files, or start other assistants.\n\n\
Return only the Markdown body of your contribution. Do not emit YAML frontmatter, a Position/Reply heading, code fences around the whole response, or commentary about the task.\n\n\
--- THREAD SNAPSHOT ---\n{snapshot}\n--- END SNAPSHOT ---\n",
        name = harness.display_name,
        id = id,
        round = round,
        kind = kind.heading(),
        snapshot = snapshot
    )
}

fn clean_model_output(raw: &str) -> Result<String> {
    let mut body = raw.trim().to_string();
    if body.starts_with("```markdown") && body.ends_with("```") {
        body = body[11..body.len() - 3].trim().to_string();
    } else if body.starts_with("```") && body.ends_with("```") {
        body = body[3..body.len() - 3].trim().to_string();
    }
    if body.is_empty() {
        bail!("harness returned an empty contribution");
    }
    if body.len() > 120_000 {
        bail!("harness contribution exceeded 120 KB");
    }
    if body.starts_with("---\n") {
        bail!("harness returned forbidden frontmatter");
    }
    validate_contribution(&body)?;
    Ok(body)
}

fn resolve_panel(config: &Config, panel: &str, caller: &str) -> Result<Vec<Harness>> {
    let ids = if panel == "others" {
        config
            .panels
            .get("core")
            .cloned()
            .ok_or_else(|| anyhow!("core panel is not configured"))?
    } else if let Some(ids) = config.panels.get(panel) {
        ids.clone()
    } else {
        panel
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };
    let mut seen = BTreeSet::new();
    let mut harnesses = Vec::new();
    for id in ids {
        if panel == "others" && id == caller {
            continue;
        }
        if !seen.insert(id.clone()) {
            continue;
        }
        let harness = config
            .harnesses
            .get(&id)
            .ok_or_else(|| anyhow!("unknown harness: {id}"))?;
        if harness.enabled {
            harnesses.push(harness.clone());
        }
    }
    if harnesses.is_empty() {
        bail!("panel resolved to no enabled harnesses");
    }
    Ok(harnesses)
}

fn append_contribution(
    thread: &mut String,
    author: &str,
    display_name: &str,
    kind: ContributionKind,
    reply_to: Option<&str>,
    body: &str,
    round: Option<u32>,
) -> Result<()> {
    update_participants(thread, author)?;
    let placeholder = "_(awaiting positions)_";
    if thread.contains(placeholder) {
        *thread = thread.replacen(placeholder, "", 1);
    }
    let date = Local::now().format("%Y-%m-%d");
    let target = reply_to
        .map(|name| format!(" → {name}"))
        .unwrap_or_default();
    let marker = round
        .map(|round| format!("\n\n<!-- forum-round:{round} harness:{author} -->"))
        .unwrap_or_default();
    let block = format!(
        "\n### {} — {} ({}, {}){}\n\n{}{}\n",
        kind.heading(),
        display_name,
        author,
        date,
        target,
        body.trim(),
        marker
    );
    let insertion = thread
        .find("\n## Open questions")
        .ok_or_else(|| anyhow!("thread lacks an '## Open questions' section"))?;
    thread.insert_str(insertion, &block);
    Ok(())
}

fn update_participants(thread: &mut String, author: &str) -> Result<()> {
    let old = thread
        .lines()
        .find(|line| line.starts_with("participants:"))
        .ok_or_else(|| anyhow!("thread frontmatter lacks participants"))?
        .to_string();
    let start = old
        .find('[')
        .ok_or_else(|| anyhow!("invalid participants list"))?;
    let end = old
        .rfind(']')
        .ok_or_else(|| anyhow!("invalid participants list"))?;
    let mut participants: Vec<String> = old[start + 1..end]
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if !participants.iter().any(|p| p == author) {
        participants.push(author.to_string());
        let new = format!("participants: [{}]", participants.join(", "));
        *thread = thread.replacen(&old, &new, 1);
    }
    Ok(())
}

fn ensure_open(thread: &str) -> Result<()> {
    match frontmatter_value(thread, "status").as_deref() {
        Some("open") => Ok(()),
        Some(status) => bail!("thread is {status}, not open"),
        None => bail!("thread lacks status frontmatter"),
    }
}

fn frontmatter_value(thread: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    thread
        .lines()
        .skip(1)
        .take_while(|line| *line != "---")
        .find_map(|line| line.strip_prefix(&prefix).map(|v| v.trim().to_string()))
}

fn has_round_marker(thread: &str, round: u32, harness: &str) -> bool {
    thread.contains(&format!("<!-- forum-round:{round} harness:{harness} -->"))
}

fn has_round_contribution(thread: &str, round: u32, harness: &str) -> bool {
    has_round_marker(thread, round, harness)
        || (round == 1 && participants(thread).iter().any(|item| item == harness))
}

fn participants(thread: &str) -> Vec<String> {
    let Some(value) = frontmatter_value(thread, "participants") else {
        return Vec::new();
    };
    value
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn round_markers(thread: &str) -> BTreeMap<u32, BTreeSet<String>> {
    let mut result: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    for line in thread.lines() {
        let Some(rest) = line.trim().strip_prefix("<!-- forum-round:") else {
            continue;
        };
        let Some((round, rest)) = rest.split_once(" harness:") else {
            continue;
        };
        let Ok(round) = round.parse::<u32>() else {
            continue;
        };
        let harness = rest.trim_end_matches(" -->").trim();
        if !harness.is_empty() {
            result.entry(round).or_default().insert(harness.to_string());
        }
    }
    result
}

fn choose_round(thread: &str, requested: Option<u32>, new_round: bool) -> u32 {
    if let Some(round) = requested {
        return round.max(1);
    }
    let mut highest = round_markers(thread)
        .keys()
        .next_back()
        .copied()
        .unwrap_or(0);
    if highest == 0 && (thread.contains("### Position —") || thread.contains("### Reply —")) {
        highest = 1;
    }
    if new_round {
        highest + 1
    } else {
        highest.max(1)
    }
}

fn require_thread(root: &Path, id: &str) -> Result<PathBuf> {
    resolve_thread(root, id)?.ok_or_else(|| anyhow!("forum thread not found: {id}"))
}

fn resolve_thread(root: &Path, id: &str) -> Result<Option<PathBuf>> {
    let candidate = PathBuf::from(id);
    if candidate.is_file() {
        return Ok(Some(candidate));
    }
    let joined = root.join(id);
    if joined.is_file() {
        return Ok(Some(joined));
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in
            fs::read_dir(&dir).with_context(|| format!("failed to scan {}", dir.display()))?
        {
            let path = entry?.path();
            if path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with('.'))
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("md") {
                let raw = fs::read_to_string(&path)?;
                if frontmatter_value(&raw, "id").as_deref() == Some(id) {
                    return Ok(Some(path));
                }
            }
        }
    }
    Ok(None)
}

fn insert_open_index_row(index_path: &Path, row: &str) -> Result<()> {
    let mut index = fs::read_to_string(index_path)?;
    let marker = "\n---\n\n## Proposed";
    let insertion = index
        .find(marker)
        .ok_or_else(|| anyhow!("INDEX.md lacks the Open/Proposed boundary"))?;
    index.insert_str(insertion, &format!("\n{row}\n"));
    atomic_write(index_path, &index)
}

fn read_text_arg(
    inline: Option<String>,
    file: Option<PathBuf>,
    stdin_fallback: bool,
) -> Result<Option<String>> {
    if let Some(text) = inline {
        return nonempty(text);
    }
    if let Some(path) = file {
        return nonempty(
            fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?,
        );
    }
    if stdin_fallback && !std::io::stdin().is_terminal() {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        return nonempty(text);
    }
    Ok(None)
}

fn nonempty(text: String) -> Result<Option<String>> {
    if text.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(text))
    }
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        bail!("{id:?} must contain only lowercase ASCII letters, digits, and hyphens");
    }
    Ok(())
}

fn validate_single_line(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() || value.contains(['\n', '\r']) {
        bail!("{label} must be a non-empty single line");
    }
    Ok(())
}

fn validate_contribution(body: &str) -> Result<()> {
    const RESERVED: [&str; 5] = [
        "## Positions",
        "## Open questions",
        "## Decision",
        "## Consequences / follow-ups",
        "<!-- forum-round:",
    ];
    for line in body.lines().map(str::trim) {
        if RESERVED.iter().any(|reserved| line.starts_with(reserved)) {
            bail!("contribution contains reserved forum structure: {line}");
        }
    }
    Ok(())
}

fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn display_name_for(id: &str) -> String {
    match id {
        "codex" => "Codex".into(),
        "claude-code" => "Claude Code".into(),
        "grok-build" => "Grok Build".into(),
        "will" => "William".into(),
        other => other
            .split('-')
            .map(capitalize)
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

fn create_job_dir(root: &Path, id: &str, round: u32) -> Result<PathBuf> {
    let timestamp = Local::now().format("%Y%m%d-%H%M%S");
    let state_root = if root == default_root() {
        default_state_root()
    } else {
        orchestrator_dir(root).join("local-state")
    };
    let path = state_root
        .join("jobs")
        .join(format!("{id}-r{round}-{timestamp}"));
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn command_exists(command: &str) -> bool {
    let path = Path::new(command);
    if path.components().count() > 1 {
        return path.is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(command).is_file()))
}

fn atomic_write(path: &Path, body: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.forum-tmp-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    {
        let mut file = File::create(&temp)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&temp, path).with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

struct ForumLock(File);

impl ForumLock {
    fn acquire(root: &Path) -> Result<Self> {
        fs::create_dir_all(root)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(".forum.lock"))?;
        file.lock_exclusive()?;
        Ok(Self(file))
    }
}

impl Drop for ForumLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

struct WorkerLock(File);

impl WorkerLock {
    fn acquire(root: &Path) -> Result<Self> {
        let path = orchestrator_dir(root).join("worker.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        file.try_lock_exclusive()
            .with_context(|| format!("another forum worker already owns {}", path.display()))?;
        Ok(Self(file))
    }
}

impl Drop for WorkerLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_thread() -> String {
        "---\nid: test-thread\nsystem: meta\nlevel: architecture\nstatus: open\nopened: 2026-07-17\nopened_by: will\nparticipants: [will]\ndecision: null\n---\n\n# Test\n\n## Context\n\nContext.\n\n## Positions\n\n_(awaiting positions)_\n\n## Open questions\n\n- Question?\n\n## Decision\n\n_(none)_\n".into()
    }

    fn write_temp_forum(temp: &TempDir) {
        fs::write(temp.path().join("INDEX.md"), "# Index\n").unwrap();
        fs::write(temp.path().join("PROTOCOL.md"), "# Protocol\n").unwrap();
        let dir = temp.path().join("meta");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("thread.md"), sample_thread()).unwrap();
    }

    #[test]
    fn contribution_updates_participants_and_adds_marker() {
        let mut thread = sample_thread();
        append_contribution(
            &mut thread,
            "codex",
            "Codex",
            ContributionKind::Position,
            None,
            "A substantive claim.",
            Some(1),
        )
        .unwrap();
        assert!(thread.contains("participants: [will, codex]"));
        assert!(thread.contains("### Position — Codex (codex,"));
        assert!(thread.contains("<!-- forum-round:1 harness:codex -->"));
        assert!(!thread.contains("_(awaiting positions)_"));
        assert!(thread.find("### Position").unwrap() < thread.find("## Open questions").unwrap());
    }

    #[test]
    fn markers_are_idempotency_keys() {
        let mut thread = sample_thread();
        append_contribution(
            &mut thread,
            "codex",
            "Codex",
            ContributionKind::Position,
            None,
            "One",
            Some(1),
        )
        .unwrap();
        append_contribution(
            &mut thread,
            "claude-code",
            "Claude",
            ContributionKind::Position,
            None,
            "Two",
            Some(1),
        )
        .unwrap();
        assert!(has_round_marker(&thread, 1, "codex"));
        assert_eq!(round_markers(&thread).get(&1).unwrap().len(), 2);
        assert_eq!(choose_round(&thread, None, false), 1);
        assert_eq!(choose_round(&thread, None, true), 2);
    }

    #[test]
    fn legacy_participants_count_as_round_one_only() {
        let mut thread = sample_thread();
        append_contribution(
            &mut thread,
            "grok-build",
            "Grok Build",
            ContributionKind::Position,
            None,
            "Manual legacy contribution.",
            None,
        )
        .unwrap();
        assert!(has_round_contribution(&thread, 1, "grok-build"));
        assert!(!has_round_contribution(&thread, 2, "grok-build"));
        assert_eq!(choose_round(&thread, None, true), 2);
    }

    #[test]
    fn panel_others_excludes_caller() {
        let config = default_config();
        let panel = resolve_panel(&config, "others", "codex").unwrap();
        assert_eq!(
            panel.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            vec!["claude-code", "grok-build"]
        );
    }

    #[test]
    fn background_convene_creates_a_durable_exact_round_job() {
        let temp = TempDir::new().unwrap();
        write_temp_forum(&temp);
        let args = ConveneArgs {
            id: "test-thread".into(),
            caller: "codex".into(),
            panel: "others".into(),
            round: None,
            new_round: false,
            kind: None,
            dry_run: false,
            background: true,
            max_attempts: 3,
        };
        enqueue_convene(temp.path(), &default_config(), args).unwrap();
        let files = job_files(&queue_dir(temp.path())).unwrap();
        assert_eq!(files.len(), 1);
        let job: QueueJob = toml::from_str(&fs::read_to_string(&files[0]).unwrap()).unwrap();
        assert_eq!(job.thread_id, "test-thread");
        assert_eq!(job.round, 1);
        assert_eq!(job.attempts, 0);
        assert_eq!(job.max_attempts, 3);
        assert!(matches!(job.kind, ContributionKind::Position));
    }

    #[cfg(unix)]
    #[test]
    fn queued_job_runs_end_to_end_with_a_fake_harness() {
        let temp = TempDir::new().unwrap();
        write_temp_forum(&temp);
        let fake = Harness {
            id: "fake".into(),
            display_name: "Fake Harness".into(),
            command: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf '**Claim:** queued worker ran\\n'".into(),
            ],
            prompt_mode: PromptMode::Argument,
            enabled: true,
        };
        let mut harnesses = BTreeMap::new();
        harnesses.insert("fake".into(), fake);
        let config = Config {
            harnesses,
            panels: BTreeMap::new(),
        };
        enqueue_convene(
            temp.path(),
            &config,
            ConveneArgs {
                id: "test-thread".into(),
                caller: "will".into(),
                panel: "fake".into(),
                round: None,
                new_round: false,
                kind: None,
                dry_run: false,
                background: true,
                max_attempts: 2,
            },
        )
        .unwrap();
        assert!(process_next_job(temp.path(), &config).unwrap());
        assert!(job_files(&queue_dir(temp.path())).unwrap().is_empty());
        assert_eq!(job_files(&completed_dir(temp.path())).unwrap().len(), 1);
        let thread = fs::read_to_string(temp.path().join("meta/thread.md")).unwrap();
        assert!(thread.contains("**Claim:** queued worker ran"));
        assert!(thread.contains("<!-- forum-round:1 harness:fake -->"));
    }

    #[test]
    fn resolves_thread_by_frontmatter_id() {
        let temp = TempDir::new().unwrap();
        write_temp_forum(&temp);
        assert!(resolve_thread(temp.path(), "test-thread")
            .unwrap()
            .is_some());
    }

    #[test]
    fn index_row_goes_inside_open_section() {
        let temp = TempDir::new().unwrap();
        let index = temp.path().join("INDEX.md");
        fs::write(
            &index,
            "# Index\n\n## Open\n\n| h |\n|---|\n\n---\n\n## Proposed\n",
        )
        .unwrap();
        insert_open_index_row(&index, "| row |").unwrap();
        let raw = fs::read_to_string(index).unwrap();
        assert!(raw.contains("|---|\n\n| row |\n\n---\n\n## Proposed"));
    }

    #[test]
    fn rejects_frontmatter_from_model() {
        assert!(clean_model_output("---\nid: bad\n---").is_err());
        assert_eq!(
            clean_model_output("```markdown\nClaim.\n```").unwrap(),
            "Claim."
        );
        assert!(clean_model_output("Claim.\n\n## Decision\nNo.").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn cold_starts_a_configured_headless_process() {
        let temp = TempDir::new().unwrap();
        let harness = Harness {
            id: "fake".into(),
            display_name: "Fake Harness".into(),
            command: "/bin/sh".into(),
            args: vec!["-c".into(), "printf '**Claim:** cold-started\\n'".into()],
            prompt_mode: PromptMode::Argument,
            enabled: true,
        };
        let result = invoke_harness(harness, temp.path(), "ignored prompt");
        assert_eq!(result.body.as_deref(), Some("**Claim:** cold-started"));
        assert!(result.error.is_none());
    }
}
