use anyhow::{anyhow, bail, Context, Result};
use chrono::Local;
use clap::{Args, Parser, Subcommand, ValueEnum};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
    /// List completed background rounds awaiting William's attention
    Inbox {
        /// Include acknowledged completion receipts
        #[arg(long)]
        all: bool,
        /// Per-thread view (default), raw per-round table, or the compact startup summary
        #[arg(long, value_enum, default_value_t = InboxFormat::Threads)]
        format: InboxFormat,
    },
    /// Mark unread completion receipts for a thread or job as seen
    Acknowledge { id: String },
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
    /// Round 1 only: the caller's own Position, staged in a file, hash-locked, and revealed together with the panel's (blind round)
    #[arg(long)]
    with_position: Option<PathBuf>,
    /// Reply rounds: harness that performs the falsification pass, or "auto" to rotate by thread
    #[arg(long)]
    critic: Option<String>,
    /// Shadow baseline "<harness>:<n>": n independent samples of one harness against the same snapshot, written to <area>/.shadow/<thread-id>/ and never into the thread. Use the strongest single model available (currently claude-code:5; see PROTOCOL.md)
    #[arg(long)]
    shadow: Option<String>,
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

#[derive(Clone, Copy, Debug, ValueEnum)]
enum InboxFormat {
    /// One line per thread: status, latest round, what it needs, verdict or decision gist
    Threads,
    /// One line per completed round (the raw receipts)
    Table,
    /// Compact startup summary used by ai-brief
    Brief,
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
    /// Blind round 1: the caller's staged Position travels with the job and is hash-locked
    #[serde(default)]
    staged_position: Option<String>,
    #[serde(default)]
    staged_position_sha256: Option<String>,
    /// Resolved critic harness for a reply round
    #[serde(default)]
    critic: Option<String>,
    /// Shadow baseline spec "<harness>:<n>"
    #[serde(default)]
    shadow: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct InboxReceipt {
    version: u32,
    job_id: String,
    thread_id: String,
    round: u32,
    completed_at: String,
    participants: Vec<String>,
    /// The thread's `### Residual dissent` section, quoted verbatim, when one exists
    #[serde(default)]
    residual_dissent: Option<String>,
}

/// A caller's round-1 Position staged before the panel is convened.
#[derive(Clone, Debug)]
struct StagedPosition {
    content: String,
    sha256: String,
    /// Foreground only: the file it was read from, re-read at write time to refuse substitution
    source: Option<PathBuf>,
}

impl StagedPosition {
    fn from_file(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read staged position {}", path.display()))?;
        if content.trim().is_empty() {
            bail!("staged position {} is empty", path.display());
        }
        validate_contribution(&content)?;
        Ok(Self {
            sha256: sha256_hex(&content),
            content,
            source: Some(path.to_path_buf()),
        })
    }

    fn from_job(content: &str, sha256: &str) -> Self {
        Self {
            content: content.to_string(),
            sha256: sha256.to_string(),
            source: None,
        }
    }

    /// Refuse substitution: the content must still hash to what was recorded when the
    /// round started, and a foreground source file must not have been edited meanwhile.
    fn verify(&self) -> Result<()> {
        if sha256_hex(&self.content) != self.sha256 {
            bail!("staged position hash mismatch: content no longer matches the hash recorded at convene time");
        }
        if let Some(source) = &self.source {
            let now = fs::read_to_string(source)
                .with_context(|| format!("staged position {} disappeared during the round", source.display()))?;
            if sha256_hex(&now) != self.sha256 {
                bail!(
                    "staged position {} was edited after the round started (hash {} != {}); refusing to write it",
                    source.display(),
                    &sha256_hex(&now)[..12],
                    &self.sha256[..12]
                );
            }
        }
        Ok(())
    }
}

fn sha256_hex(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// "<harness>:<n>" → (harness, n), n in 1..=10.
fn parse_shadow_spec(spec: &str, config: &Config) -> Result<(Harness, u32)> {
    let (id, count) = spec
        .split_once(':')
        .ok_or_else(|| anyhow!("shadow spec must be <harness>:<n>, got {spec:?}"))?;
    validate_id(id)?;
    let count: u32 = count
        .trim()
        .parse()
        .map_err(|_| anyhow!("shadow sample count must be a number, got {count:?}"))?;
    if !(1..=10).contains(&count) {
        bail!("shadow sample count must be between 1 and 10");
    }
    let harness = config
        .harnesses
        .get(id)
        .ok_or_else(|| anyhow!("unknown shadow harness: {id}"))?;
    if !harness.enabled {
        bail!("shadow harness {id} is disabled");
    }
    Ok((harness.clone(), count))
}

/// None → no critic; "auto" → rotate by thread id over the resolved panel; otherwise a panel member.
fn choose_critic(spec: Option<&str>, thread_id: &str, panel: &[Harness]) -> Result<Option<String>> {
    let Some(spec) = spec else { return Ok(None) };
    if panel.is_empty() {
        bail!("cannot assign a critic to an empty panel");
    }
    if spec == "auto" {
        let index = (fnv1a(thread_id) % panel.len() as u64) as usize;
        return Ok(Some(panel[index].id.clone()));
    }
    validate_id(spec)?;
    if !panel.iter().any(|h| h.id == spec) {
        bail!("critic {spec} is not on the resolved panel");
    }
    Ok(Some(spec.to_string()))
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
        Commands::Inbox { all, format } => cmd_inbox(&root, all, format),
        Commands::Acknowledge { id } => cmd_acknowledge(&root, &id),
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
    let staged = args
        .with_position
        .as_deref()
        .map(StagedPosition::from_file)
        .transpose()?;
    run_convene(root, config, args, staged)
}

/// One round's plan, resolved identically for foreground, queued, and worker paths.
struct RoundPlan {
    path: PathBuf,
    snapshot: String,
    round: u32,
    kind: ContributionKind,
    pending: Vec<Harness>,
    sequential: bool,
    critic: Option<String>,
    shadow: Option<(Harness, u32)>,
}

fn plan_round(
    root: &Path,
    config: &Config,
    args: &ConveneArgs,
    staged: Option<&StagedPosition>,
) -> Result<RoundPlan> {
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
        .iter()
        .filter(|h| !has_round_contribution(&snapshot, round, &h.id))
        .cloned()
        .collect();

    let caller_posted = has_contribution_heading(&snapshot, &args.caller);
    if staged.is_some() {
        if round != 1 || !matches!(kind, ContributionKind::Position) {
            bail!("--with-position applies to round 1 Positions only");
        }
        if caller_posted {
            bail!(
                "caller {} has already posted to {}; the round is sequential and --with-position cannot make it blind",
                args.caller,
                args.id
            );
        }
    }
    // A round-1 Position round whose caller already posted was not blind: the
    // panel reads the caller's framing. Record that on every marker of the round.
    let sequential = round == 1 && matches!(kind, ContributionKind::Position) && caller_posted;

    let critic = match kind {
        ContributionKind::Reply => choose_critic(args.critic.as_deref(), &args.id, &requested)?,
        ContributionKind::Position => {
            if args.critic.is_some() {
                bail!("--critic applies to reply rounds; round {round} is a Position round");
            }
            None
        }
    };
    let shadow = args
        .shadow
        .as_deref()
        .map(|spec| parse_shadow_spec(spec, config))
        .transpose()?;

    Ok(RoundPlan {
        path,
        snapshot,
        round,
        kind,
        pending,
        sequential,
        critic,
        shadow,
    })
}

fn describe_plan(id: &str, plan: &RoundPlan, staged: Option<&StagedPosition>) {
    println!(
        "Thread {} round {}{}: {}",
        id,
        plan.round,
        if plan.sequential { " (sequential: caller posted before convening)" } else { "" },
        if plan.pending.is_empty() {
            "no panel invocations".to_string()
        } else {
            plan.pending
                .iter()
                .map(|h| h.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    if let Some(staged) = staged {
        println!(
            "blind round: caller Position staged (sha256 {}), revealed together with the panel",
            &staged.sha256[..12]
        );
    }
    if let Some(critic) = &plan.critic {
        println!("critic (falsification pass): {critic}");
    }
    if let Some((harness, count)) = &plan.shadow {
        println!(
            "shadow baseline: {} x{} → {}/ (never written into the thread)",
            harness.id,
            count,
            shadow_dir(&plan.path).join(id).display()
        );
    }
}

fn run_convene(
    root: &Path,
    config: &Config,
    args: ConveneArgs,
    staged: Option<StagedPosition>,
) -> Result<()> {
    let plan = plan_round(root, config, &args, staged.as_ref())?;
    // A retry after a completed reveal (worker crashed between the thread write and the
    // queue acknowledgement) must recognise the revealed round and only finish bookkeeping:
    // nothing is re-invoked and no adapted Position can be appended.
    let already_revealed = staged.is_some() && has_contribution_heading(&plan.snapshot, &args.caller);
    if plan.pending.is_empty() && (staged.is_none() || already_revealed) && plan.shadow.is_none() {
        println!(
            "Round {} already contains every requested harness{}; nothing to do.",
            plan.round,
            if already_revealed { " and the staged Position is revealed" } else { "" }
        );
        return Ok(());
    }
    let staged = if already_revealed { None } else { staged };
    describe_plan(&args.id, &plan, staged.as_ref());
    if args.dry_run {
        for harness in &plan.pending {
            println!("dry-run: {} {}", harness.command, harness.args.join(" "));
        }
        if let Some((harness, count)) = &plan.shadow {
            for k in 1..=*count {
                println!(
                    "dry-run shadow {k}/{count}: {} {}",
                    harness.command,
                    harness.args.join(" ")
                );
            }
        }
        return Ok(());
    }

    // Pre-flight: the staged Position must verify and its hash must be committed in the
    // thread before any model is invoked. The panel's snapshot is taken after that
    // commitment, so it carries the hash but never the text.
    let snapshot = if let Some(staged) = &staged {
        staged.verify()?;
        let _lock = ForumLock::acquire(root)?;
        let mut current = fs::read_to_string(&plan.path)?;
        ensure_open(&current)?;
        commit_staged_hash(&mut current, plan.round, &args.caller, &staged.sha256)?;
        atomic_write(&plan.path, &current)?;
        current
    } else {
        plan.snapshot.clone()
    };
    let plan = RoundPlan { snapshot, ..plan };

    let job_dir = create_job_dir(root, &args.id, plan.round)?;
    atomic_write(&job_dir.join("snapshot.md"), &plan.snapshot)?;
    if let Some(staged) = &staged {
        atomic_write(&job_dir.join("staged-position.md"), &staged.content)?;
        atomic_write(&job_dir.join("staged-position.sha256"), &staged.sha256)?;
    }

    let mut handles = Vec::new();
    for harness in plan.pending.clone() {
        let is_critic = plan.critic.as_deref() == Some(harness.id.as_str());
        let prompt = build_prompt(&args.id, plan.round, plan.kind, &harness, &plan.snapshot, is_critic);
        atomic_write(&job_dir.join(format!("{}-prompt.md", harness.id)), &prompt)?;
        let root = root.to_path_buf();
        handles.push(thread::spawn(move || invoke_harness(harness, &root, &prompt)));
    }
    let mut shadow_handles = Vec::new();
    if let Some((harness, count)) = &plan.shadow {
        let prompt = build_prompt(
            &args.id,
            plan.round,
            ContributionKind::Position,
            harness,
            &plan.snapshot,
            false,
        );
        atomic_write(&job_dir.join(format!("shadow-{}-prompt.md", harness.id)), &prompt)?;
        for _ in 0..*count {
            let harness = harness.clone();
            let root = root.to_path_buf();
            let prompt = prompt.clone();
            shadow_handles.push(thread::spawn(move || invoke_harness(harness, &root, &prompt)));
        }
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
        let suffix = if result.error.is_some() { "error.txt" } else { "output.md" };
        let content = result
            .body
            .as_deref()
            .or(result.error.as_deref())
            .unwrap_or("");
        atomic_write(&job_dir.join(format!("{}-{suffix}", result.harness.id)), content)?;
    }

    // Shadow samples land beside the thread, never in it. `.shadow/` is dot-prefixed so
    // thread resolution skips it, and snapshots are built from the thread file alone.
    let mut shadow_written = 0u32;
    let mut shadow_failures = Vec::new();
    if let Some((harness, _)) = &plan.shadow {
        let snapshot_hash = sha256_hex(&plan.snapshot);
        let dir = shadow_dir(&plan.path).join(&args.id);
        fs::create_dir_all(&dir)?;
        for (index, handle) in shadow_handles.into_iter().enumerate() {
            let result = handle
                .join()
                .map_err(|_| anyhow!("shadow worker panicked"))?;
            let k = index as u32 + 1;
            match result.body {
                Some(body) => {
                    let header = format!(
                        "<!-- forum-shadow thread:{} round:{} harness:{} sample:{} snapshot-sha256:{} written:{} -->\n\n",
                        args.id,
                        plan.round,
                        harness.id,
                        k,
                        snapshot_hash,
                        Local::now().to_rfc3339()
                    );
                    atomic_write(
                        &dir.join(format!("r{}-{}-{}.md", plan.round, harness.id, k)),
                        &format!("{header}{body}\n"),
                    )?;
                    shadow_written += 1;
                }
                None => shadow_failures.push(format!(
                    "shadow {}#{k}: {}",
                    harness.id,
                    result.error.unwrap_or_default()
                )),
            }
        }
    }

    // Reveal: the staged Position and every successful panel contribution are written
    // in one serialised edit, so no participant's text reaches the thread before the rest.
    // Fail closed: if the staged Position no longer verifies, or no longer matches the
    // hash committed in the thread, nothing is written (panel outputs stay in the job dir).
    let successes: Vec<&InvocationResult> = results.iter().filter(|r| r.body.is_some()).collect();
    let invocation_failures: Vec<String> = results
        .iter()
        .filter_map(|r| r.error.as_ref().map(|e| format!("{}: {e}", r.harness.id)))
        .collect();
    // A blind round is atomic over the planned round, not over whichever bodies happened
    // to succeed: if any requested harness failed, nothing is revealed, so a retry can
    // never extend an already-revealed round against post-reveal state.
    if staged.is_some() && !invocation_failures.is_empty() {
        bail!(
            "blind round not revealed: {} of {} requested invocation(s) failed; nothing written; outputs are in {}; retry is safe:\n{}",
            invocation_failures.len(),
            results.len(),
            job_dir.display(),
            invocation_failures.join("\n")
        );
    }
    if !successes.is_empty() || staged.is_some() {
        let _lock = ForumLock::acquire(root)?;
        let mut current = fs::read_to_string(&plan.path)?;
        ensure_open(&current)?;
        if let Some(staged) = &staged {
            staged
                .verify()
                .with_context(|| format!("nothing written; panel outputs are in {}", job_dir.display()))?;
            match committed_staged_hash(&current, plan.round, &args.caller) {
                Some(committed) if committed == staged.sha256 => {}
                Some(committed) => bail!(
                    "staged position does not match the commitment recorded in the thread ({}… committed, {}… offered); nothing written; panel outputs are in {}",
                    &committed[..12.min(committed.len())],
                    &staged.sha256[..12],
                    job_dir.display()
                ),
                None => bail!(
                    "thread carries no staged commitment for round {} by {}; nothing written; panel outputs are in {}",
                    plan.round,
                    args.caller,
                    job_dir.display()
                ),
            }
            // Round-scoped idempotency: `--with-position` is round 1 only and plan_round
            // rejects a caller who already has any heading (that round is sequential), so
            // an existing round-1 marker is the only legitimate "already revealed" state.
            if !has_round_marker(&current, plan.round, &args.caller)
                && !has_contribution_heading(&current, &args.caller)
            {
                append_contribution_marked(
                    &mut current,
                    &args.caller,
                    &display_name_for(&args.caller),
                    ContributionKind::Position,
                    None,
                    &staged.content,
                    Some(plan.round),
                    "",
                )?;
            }
        }
        let extra_mode = if plan.sequential { " mode:sequential" } else { "" };
        for result in successes {
            if has_round_contribution(&current, plan.round, &result.harness.id) {
                continue;
            }
            let role = if plan.critic.as_deref() == Some(result.harness.id.as_str()) {
                " role:critic"
            } else {
                ""
            };
            append_contribution_marked(
                &mut current,
                &result.harness.id,
                &result.harness.display_name,
                plan.kind,
                None,
                result.body.as_deref().unwrap_or(""),
                Some(plan.round),
                &format!("{extra_mode}{role}"),
            )?;
        }
        atomic_write(&plan.path, &current)?;
    }

    let failures = invocation_failures;
    println!("Job record: {}", job_dir.display());
    println!("Appended {} contribution(s)", results.len() - failures.len());
    if plan.shadow.is_some() {
        println!("Shadow samples written: {shadow_written}");
    }
    if !failures.is_empty() || !shadow_failures.is_empty() {
        let mut lines = failures;
        lines.extend(shadow_failures);
        bail!("round partially failed; retry is safe:\n{}", lines.join("\n"));
    }
    Ok(())
}

fn enqueue_convene(root: &Path, config: &Config, args: ConveneArgs) -> Result<()> {
    let _lock = ForumLock::acquire(root)?;
    let staged = args
        .with_position
        .as_deref()
        .map(StagedPosition::from_file)
        .transpose()?;
    let plan = plan_round(root, config, &args, staged.as_ref())?;
    if plan.pending.is_empty() && staged.is_none() && plan.shadow.is_none() {
        println!(
            "Round {} already contains every requested harness; nothing queued.",
            plan.round
        );
        return Ok(());
    }

    ensure_queue_dirs(root)?;
    let now = Local::now();
    let nonce = now
        .timestamp_nanos_opt()
        .unwrap_or_else(|| now.timestamp_micros() * 1_000);
    let job_id = format!("{}-r{}-{}-{}", args.id, plan.round, nonce, std::process::id());
    let job = QueueJob {
        version: 1,
        job_id: job_id.clone(),
        thread_id: args.id.clone(),
        caller: args.caller.clone(),
        panel: args.panel.clone(),
        round: plan.round,
        kind: plan.kind,
        attempts: 0,
        max_attempts: args.max_attempts,
        created_at: now.to_rfc3339(),
        next_attempt_at: now.timestamp(),
        completed_at: None,
        last_error: None,
        staged_position: staged.as_ref().map(|s| s.content.clone()),
        staged_position_sha256: staged.as_ref().map(|s| s.sha256.clone()),
        critic: plan.critic.clone(),
        shadow: args.shadow.clone(),
    };
    if let Some(staged) = &staged {
        let mut current = fs::read_to_string(&plan.path)?;
        commit_staged_hash(&mut current, plan.round, &args.caller, &staged.sha256)?;
        atomic_write(&plan.path, &current)?;
    }
    let raw = toml::to_string_pretty(&job)?;
    atomic_write(&queue_dir(root).join(format!("{job_id}.toml")), &raw)?;
    println!("Queued forum job: {job_id}");
    describe_plan(&args.id, &plan, staged.as_ref());
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

    let staged = match (&job.staged_position, &job.staged_position_sha256) {
        (Some(content), Some(sha256)) => Some(StagedPosition::from_job(content, sha256)),
        (Some(_), None) => {
            job.attempts = job.max_attempts;
            job.completed_at = Some(Local::now().to_rfc3339());
            job.last_error = Some("staged position has no recorded hash; refusing".into());
            archive_job(&running_path, &failed_dir(root), &job)?;
            return Ok(true);
        }
        _ => None,
    };
    let result = run_convene(
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
            with_position: None,
            critic: job.critic.clone(),
            shadow: job.shadow.clone(),
        },
        staged,
    );

    match result {
        Ok(()) => {
            job.completed_at = Some(Local::now().to_rfc3339());
            job.last_error = None;
            publish_completion(root, &job)?;
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

fn publish_completion(root: &Path, job: &QueueJob) -> Result<()> {
    let _lock = ForumLock::acquire(root)?;
    let path = require_thread(root, &job.thread_id)?;
    let thread = fs::read_to_string(path)?;
    let receipt = InboxReceipt {
        version: 1,
        job_id: job.job_id.clone(),
        thread_id: job.thread_id.clone(),
        round: job.round,
        completed_at: job
            .completed_at
            .clone()
            .unwrap_or_else(|| Local::now().to_rfc3339()),
        participants: participants(&thread),
        residual_dissent: residual_dissent_section(&thread),
    };
    let receipt_path = unread_inbox_dir(root).join(format!("{}.toml", job.job_id));
    if !receipt_path.exists()
        && !acknowledged_inbox_dir(root)
            .join(format!("{}.toml", job.job_id))
            .exists()
    {
        atomic_write(&receipt_path, &toml::to_string_pretty(&receipt)?)?;
    }

    if root == default_root() {
        let marker = format!("<!-- forum-complete:{} -->", job.job_id);
        let messageboard = root
            .parent()
            .ok_or_else(|| anyhow!("forum root has no shared-directory parent"))?
            .join("MESSAGEBOARD.md");
        let already_notified =
            messageboard.is_file() && fs::read_to_string(&messageboard)?.contains(&marker);
        if !already_notified {
            let dissent = receipt
                .residual_dissent
                .as_deref()
                .map(|text| format!("\n\n**Residual dissent (quoted from the thread):**\n{}", blockquote(text)))
                .unwrap_or_default();
            let message = format!(
                "FORUM COMPLETE — `{}` round {}\n\nThe background panel has finished. Durable results are in the forum thread. Review with `forum inbox` or `forum status {}`; acknowledge after reading with `forum acknowledge {}`.{}\n\n{}",
                job.thread_id, job.round, job.thread_id, job.thread_id, dissent, marker
            );
            if let Err(error) = post_messageboard_message(&message) {
                eprintln!(
                    "warning: completion receipt is durable but Messageboard notification failed: {error:#}"
                );
            }
        }
        notify_completion_best_effort(&job.thread_id, job.round);
    }
    Ok(())
}

fn notify_completion_best_effort(thread_id: &str, round: u32) {
    if !command_exists("notify-send")
        || (std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none())
    {
        return;
    }
    let _ = Command::new("notify-send")
        .arg("Forum round complete")
        .arg(format!("{thread_id} round {round} is ready for review"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn cmd_inbox(root: &Path, all: bool, format: InboxFormat) -> Result<()> {
    ensure_queue_dirs(root)?;
    let mut receipts = read_receipts(&unread_inbox_dir(root), false)?;
    if all {
        receipts.extend(read_receipts(&acknowledged_inbox_dir(root), true)?);
    }
    receipts.sort_by(|a, b| b.1.completed_at.cmp(&a.1.completed_at));
    if receipts.is_empty() {
        println!("No unread forum completions.");
        return Ok(());
    }
    match format {
        InboxFormat::Table => {
            println!("state\tthread\tround\tcompleted\tparticipants\tjob");
            for (acknowledged, receipt) in receipts {
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}",
                    if acknowledged {
                        "acknowledged"
                    } else {
                        "unread"
                    },
                    receipt.thread_id,
                    receipt.round,
                    receipt.completed_at,
                    receipt.participants.join(","),
                    receipt.job_id
                );
                if let Some(text) = &receipt.residual_dissent {
                    println!("\tresidual dissent:\n{}", blockquote(text));
                }
            }
        }
        InboxFormat::Brief => {
            // Receipt lines first, dissent verbatim after: ai-brief caps this output at a
            // fixed byte budget, so a long dissent section can only ever cost the tail.
            let mut dissent = Vec::new();
            for (acknowledged, receipt) in receipts {
                println!(
                    "{}: `{}` round {} completed {}; participants: {}",
                    if acknowledged {
                        "acknowledged"
                    } else {
                        "UNREAD"
                    },
                    receipt.thread_id,
                    receipt.round,
                    receipt.completed_at,
                    receipt.participants.join(", ")
                );
                if let Some(text) = &receipt.residual_dissent {
                    dissent.push((receipt.thread_id.clone(), receipt.round, text.clone()));
                }
            }
            for (thread_id, round, text) in dissent {
                println!("\nResidual dissent — `{thread_id}` round {round} (verbatim):\n{}", blockquote(&text));
            }
        }
        InboxFormat::Threads => print_thread_inbox(root, receipts)?,
    }
    Ok(())
}

/// What a human needs from an inbox: not which rounds finished, but which threads are
/// waiting on someone, and on whom.
fn print_thread_inbox(root: &Path, receipts: Vec<(bool, InboxReceipt)>) -> Result<()> {
    let mut by_thread: BTreeMap<String, Vec<InboxReceipt>> = BTreeMap::new();
    for (_, receipt) in receipts {
        by_thread.entry(receipt.thread_id.clone()).or_default().push(receipt);
    }
    let mut open = Vec::new();
    let mut closed = Vec::new();
    for (id, mut rs) in by_thread {
        rs.sort_by(|a, b| b.round.cmp(&a.round));
        let latest = &rs[0];
        let raw = require_thread(root, &id)
            .and_then(|p| fs::read_to_string(p).map_err(Into::into))
            .unwrap_or_default();
        let status = frontmatter_value(&raw, "status").unwrap_or_else(|| "?".into());
        let when = latest.completed_at.get(..16).unwrap_or(&latest.completed_at).replace('T', " ");
        let line = format!(
            "  {:<44} r{} {}  {}",
            id,
            latest.round,
            when,
            thread_need(&raw, &status)
        );
        if status == "decided" || status == "rejected" || status == "parked" {
            closed.push((line, rs.len()));
        } else {
            let dissent = residual_dissent_section(&raw);
            open.push((line, dissent));
        }
    }
    if open.is_empty() {
        println!("Open threads with unread rounds: none.");
    } else {
        println!("OPEN — waiting on someone ({} thread(s))", open.len());
        for (line, dissent) in open {
            println!("{line}");
            if let Some(text) = dissent {
                println!("{}", blockquote(&text).lines().map(|l| format!("      {l}")).collect::<Vec<_>>().join("\n"));
            }
        }
    }
    if !closed.is_empty() {
        let receipts: usize = closed.iter().map(|(_, n)| n).sum();
        println!(
            "\nCLOSED — nothing owed ({} thread(s), {} unread receipt(s); `forum acknowledge <thread-id>` clears them)",
            closed.len(),
            receipts
        );
        for (line, _) in closed {
            println!("{line}");
        }
    }
    Ok(())
}

/// One phrase saying whom the thread waits on, derived from the thread text.
fn thread_need(raw: &str, status: &str) -> String {
    if status == "decided" {
        let dispatched = raw.contains("<!-- forum-dispatch:");
        let receipt = raw.contains("### Implementation receipt");
        return match (dispatched, receipt) {
            (true, true) => "decided; implemented and receipted".into(),
            (true, false) => "decided; work order dispatched, receipt pending".into(),
            _ => "decided".into(),
        };
    }
    if status != "open" {
        return status.to_string();
    }
    // A heading line, not a mention of the heading inside Context or a quote.
    if raw.lines().any(|l| l.starts_with("## Proposed decision")) {
        return "NEEDS YOUR RATIFICATION (proposed decision drafted)".into();
    }
    if let Some((who, verdict)) = latest_verdict(raw) {
        return format!("verdict {verdict} ({who}) — implementer to act or close");
    }
    "needs a decision or another round".into()
}

/// The last `**PASS…**` / `**FAIL…**` token in the thread, with the harness that wrote it.
fn latest_verdict(raw: &str) -> Option<(String, String)> {
    let mut author = String::new();
    let mut found = None;
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("### Position — ").or_else(|| line.strip_prefix("### Reply — ")) {
            if let Some(start) = rest.find('(') {
                author = rest[start + 1..].split(',').next().unwrap_or("").to_string();
            }
        }
        let mut search = line;
        while let Some(idx) = search.find("**") {
            let after = &search[idx + 2..];
            if after.starts_with("PASS") || after.starts_with("FAIL") {
                if let Some(end) = after.find("**") {
                    found = Some((author.clone(), after[..end].trim().to_string()));
                }
            }
            search = after;
        }
    }
    found
}

fn cmd_acknowledge(root: &Path, id: &str) -> Result<()> {
    validate_single_line("thread-or-job-id", id)?;
    ensure_queue_dirs(root)?;
    let _lock = ForumLock::acquire(root)?;
    let mut moved = 0;
    let mut moved_job_ids = Vec::new();
    for path in receipt_files(&unread_inbox_dir(root))? {
        let raw = fs::read_to_string(&path)?;
        let receipt: InboxReceipt = toml::from_str(&raw)?;
        if receipt.job_id == id || receipt.thread_id == id {
            let destination =
                acknowledged_inbox_dir(root).join(path.file_name().unwrap_or_default());
            fs::rename(path, destination)?;
            moved_job_ids.push(receipt.job_id);
            moved += 1;
        }
    }
    if moved == 0 {
        bail!("no unread completion receipt matches: {id}");
    }
    println!("Acknowledged {moved} forum completion(s) for {id}");
    if root == default_root() {
        for job_id in moved_job_ids {
            let marker = format!("<!-- forum-complete:{job_id} -->");
            if let Err(error) = archive_messageboard_containing(&marker) {
                eprintln!(
                    "warning: completion acknowledged but Messageboard archive failed: {error:#}"
                );
            }
        }
    }
    Ok(())
}

fn read_receipts(dir: &Path, acknowledged: bool) -> Result<Vec<(bool, InboxReceipt)>> {
    let mut receipts = Vec::new();
    for path in receipt_files(dir)? {
        let raw = fs::read_to_string(path)?;
        receipts.push((acknowledged, toml::from_str(&raw)?));
    }
    Ok(receipts)
}

fn receipt_files(dir: &Path) -> Result<Vec<PathBuf>> {
    job_files(dir)
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
        unread_inbox_dir(root),
        acknowledged_inbox_dir(root),
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

fn unread_inbox_dir(root: &Path) -> PathBuf {
    orchestrator_dir(root).join("inbox/unread")
}

fn acknowledged_inbox_dir(root: &Path) -> PathBuf {
    orchestrator_dir(root).join("inbox/acknowledged")
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
        let attrs = round_marker_attrs(&raw);
        for (round, harnesses) in markers {
            let sequential = round_is_sequential(&raw, round);
            let names = harnesses
                .into_iter()
                .map(|h| {
                    let is_critic = attrs
                        .iter()
                        .any(|(r, id, tokens)| *r == round && *id == h && tokens.iter().any(|t| t == "role:critic"));
                    if is_critic { format!("{h} (critic)") } else { h }
                })
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "round {round}{}: {names}",
                if sequential { " (sequential)" } else { "" }
            );
        }
    }
    for line in raw.lines() {
        if let Some(rest) = line.trim().strip_prefix("<!-- forum-staged:") {
            println!("staged commitment: {}", rest.trim_end_matches("-->").trim());
        }
    }
    let shadows = shadow_sample_count(&path, id)?;
    if shadows > 0 {
        println!("shadow samples: {shadows} in {}", shadow_dir(&path).join(id).display());
    }
    println!(
        "residual dissent: {}",
        if residual_dissent_section(&raw).is_some() { "recorded" } else { "none recorded" }
    );
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
    let decision_text = decision_section(&thread)
        .ok_or_else(|| anyhow!("decided thread lacks a '## Decision' section"))?;
    let residual_dissent = residual_dissent_section(&decision_text).ok_or_else(|| {
        anyhow!(
            "Decision lacks a '### Residual dissent' section (strongest rejected alternative and holder; live dissent with holder and disposition; unresolved assumptions; revisit trigger); refusing to dispatch"
        )
    })?;
    let level = frontmatter_value(&thread, "level").unwrap_or_default();
    let ratified = has_william_ratification(&decision_text);
    if level == "architecture" && !ratified {
        bail!(
            "level: architecture thread {} is decided without William's ratification recorded in the Decision; a delegated architecture close ends at '## Proposed decision (awaiting Will)' and cannot become a work order",
            args.id
        );
    }
    if round_is_sequential(&thread, 1) && !ratified {
        bail!(
            "thread {} was convened sequentially (caller posted before round 1) and its Decision does not record William's ratification; sequential threads cannot close under delegation",
            args.id
        );
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
        &residual_dissent,
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
        post_messageboard_message(&work_order)?;
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

#[allow(clippy::too_many_arguments)]
fn build_work_order(
    id: &str,
    decision: &str,
    assignee: &str,
    requested_by: &str,
    scope: &[String],
    acceptance: &[String],
    reviewers: &[String],
    relative_thread: &Path,
    residual_dissent: &str,
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
**Residual dissent (quoted from the Decision):**\n{dissent}\n\n\
**Decision record:** `design-forum/{}`\n\n\
Implement only the accepted decision and bounded scope above. Debate does not reopen during implementation; material ambiguity or scope expansion returns to the forum or William. Record shipped work in ASSISTANT-HANDOFF and obtain independent review before treating the work order as complete.\n\n\
<!-- forum-work-order:{id} -->",
        relative_thread.display(),
        dissent = blockquote(residual_dissent)
    )
}

fn blockquote(text: &str) -> String {
    text.trim()
        .lines()
        .map(|line| if line.is_empty() { ">".to_string() } else { format!("> {line}") })
        .collect::<Vec<_>>()
        .join("\n")
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

fn post_messageboard_message(message: &str) -> Result<()> {
    run_messageboard_edit(&["insert", message])
}

fn archive_messageboard_containing(needle: &str) -> Result<()> {
    run_messageboard_edit(&["archive-containing", needle])
}

fn run_messageboard_edit(args: &[&str]) -> Result<()> {
    if !command_exists("messageboard-edit") {
        bail!("messageboard-edit is not available on PATH");
    }
    let output = Command::new("messageboard-edit")
        .args(args)
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
    for line in doctor_lints(root)? {
        println!("{line}");
    }
    if failed {
        bail!("doctor found missing requirements");
    }
    Ok(())
}

/// Lints, not gates: architecture threads decided without William's ratification in
/// the Decision, and decided threads with no `### Residual dissent` section.
fn doctor_lints(root: &Path) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    let mut unratified = 0;
    let mut without_dissent = 0;
    let mut decided = 0;
    for path in thread_files(root)? {
        let raw = fs::read_to_string(&path)?;
        if frontmatter_value(&raw, "status").as_deref() != Some("decided") {
            continue;
        }
        decided += 1;
        let id = frontmatter_value(&raw, "id").unwrap_or_else(|| path.display().to_string());
        let relative = path.strip_prefix(root).unwrap_or(&path).display().to_string();
        let decision = decision_section(&raw).unwrap_or_default();
        if frontmatter_value(&raw, "level").as_deref() == Some("architecture")
            && !has_william_ratification(&decision)
        {
            unratified += 1;
            lines.push(format!(
                "LINT architecture decided without William's ratification in the Decision: {id} ({relative})"
            ));
        }
        let opened = frontmatter_value(&raw, "opened").unwrap_or_default();
        if opened.as_str() >= "2026-09-09" && residual_dissent_section(&decision).is_none() {
            without_dissent += 1;
            lines.push(format!(
                "LINT decided without a Residual dissent section (required for threads opened since 2026-09-09): {id} ({relative})"
            ));
        }
    }
    lines.push(format!(
        "lint summary: {decided} decided thread(s); {unratified} architecture close(s) lack William's ratification; {without_dissent} Decision(s) opened since 2026-09-09 lack a Residual dissent section (earlier threads exempt)"
    ));
    Ok(lines)
}

fn thread_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).with_context(|| format!("failed to scan {}", dir.display()))? {
            let path = entry?.path();
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or_default();
            if name.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("md")
                && name != "INDEX.md"
                && name != "PROTOCOL.md"
                && frontmatter_value(&fs::read_to_string(&path)?, "id").is_some()
            {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn shadow_dir(thread_path: &Path) -> PathBuf {
    thread_path
        .parent()
        .map(|p| p.join(".shadow"))
        .unwrap_or_else(|| PathBuf::from(".shadow"))
}

fn shadow_sample_count(thread_path: &Path, id: &str) -> Result<usize> {
    let dir = shadow_dir(thread_path).join(id);
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut count = 0;
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        if name.starts_with('r') && name.ends_with(".md") {
            count += 1;
        }
    }
    Ok(count)
}

/// The body of `## Decision` when it holds more than the template placeholder;
/// otherwise the body of `## Proposed decision …`. Each runs to the next `## ` heading.
fn decision_section(thread: &str) -> Option<String> {
    let section = |heading: &str| -> Option<String> {
        let mut out = String::new();
        let mut inside = false;
        for line in thread.lines() {
            if line.starts_with("## ") {
                if inside {
                    break;
                }
                inside = line.starts_with(heading);
                continue;
            }
            if inside {
                out.push_str(line);
                out.push('\n');
            }
        }
        let trimmed = out.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    };
    let is_placeholder = |text: &str| {
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .all(|l| l.starts_with("_(") && l.ends_with(")_"))
    };
    match section("## Decision") {
        Some(text) if !is_placeholder(&text) => Some(text),
        _ => section("## Proposed decision"),
    }
}

/// `### Residual dissent` body up to the next heading of level three or shallower.
fn residual_dissent_section(text: &str) -> Option<String> {
    let mut out = String::new();
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("### ") || trimmed.starts_with("## ") || trimmed.starts_with("# ") {
            if inside {
                break;
            }
            inside = trimmed.starts_with("### Residual dissent");
            continue;
        }
        if inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    let body = out.trim();
    (!body.is_empty()).then(|| body.to_string())
}

/// William's ratification, as distinct from his delegation. "under William's
/// delegation" does not count; "William ratified", "accepted by Will", etc. do.
/// Authorisation boundary for `forum dispatch`: true only if the Decision (with its
/// `### Residual dissent` subsection removed) contains a **line-anchored attestation**.
/// PROTOCOL.md prescribes the forward forms; a short enumerated legacy set covers the
/// Decisions William wrote before 2026-09-10. Anything else — including true sentences
/// that merely mention William and a verb — is a false negative, which blocks dispatch
/// safely, rather than a false positive, which would bypass the delegation ceiling.
fn has_william_ratification(text: &str) -> bool {
    without_residual_dissent(text)
        .lines()
        .any(line_records_william_ratification)
}

fn without_residual_dissent(text: &str) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("### ") || trimmed.starts_with("## ") || trimmed.starts_with("# ") {
            skipping = trimmed.starts_with("### Residual dissent");
            if skipping {
                continue;
            }
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// One line attests only if, after stripping heading marks and emphasis, it *starts*
/// with a prescribed form, and the rest of that sentence carries no deferral or reversal.
fn line_records_william_ratification(line: &str) -> bool {
    // Normalise emphasis and dash variants so "Decision - William" and "Decision – William"
    // read as the em-dash form.
    let cleaned = line
        .to_lowercase()
        .replace('*', "")
        .replace(" – ", " — ")
        .replace(" - ", " — ");
    // Strip heading marks only. A blockquote (`>`) or list item (`-`) is somebody
    // quoting or citing a ruling, not making one, and must not anchor.
    let stripped = cleaned.trim().trim_start_matches(['#', ' ']).trim();
    // Forward forms (PROTOCOL.md, "Closing without unhelpful consensus").
    const PRESCRIBED: [&str; 6] = [
        "ratified by william",
        "decided by william",
        "william ratified",
        "william ratifies",
        "decided — william",
        "decided - william",
    ];
    // Legacy forms, enumerated from the Decisions William wrote before 2026-09-10.
    const LEGACY: [&str; 5] = [
        "accepted by william on ",
        "decision — william, ",
        "decision — william ",
        "william decided on ",
        "william accepted the panel",
    ];
    let matched = PRESCRIBED
        .iter()
        .chain(LEGACY.iter())
        .find(|form| stripped.starts_with(*form))
        .map(|form| form.len())
        .or_else(|| dated_signature_len(stripped));
    let Some(prefix_len) = matched else { return false };
    // Reversals are tested on the rest of the *sentence*: a semicolon does not end it
    // ("Decided by William; awaiting monitoring." must not attest), while a full stop
    // followed by whitespace does, so a long Decision paragraph that ratifies in its
    // first sentence and says "not" three sentences later still attests.
    let sentence = first_sentence(&stripped[prefix_len..]);
    const REVERSALS: [&str; 16] = [
        "not ", "n't", "against", "defer", "until", "once ", "will be", "to be", "unless",
        "if ", "pending", "awaiting", "outstanding", "later", "should", "delegat",
    ];
    !REVERSALS.iter().any(|r| sentence.contains(r))
}

/// Text up to and including the first `.` that is followed (after any closing quotes or
/// brackets) by whitespace or the end of the string. Semicolons do not end a sentence.
fn first_sentence(text: &str) -> &str {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].1 == '.' {
            let mut j = i + 1;
            while j < chars.len() && matches!(chars[j].1, '"' | '\u{201d}' | '\'' | '\u{2019}' | ')' | ']') {
                j += 1;
            }
            if j >= chars.len() || chars[j].1.is_whitespace() {
                let end = if j < chars.len() { chars[j].0 } else { text.len() };
                return &text[..end];
            }
        }
        i += 1;
    }
    text
}

/// `will, 2026-08-21` / `william, 2026-08-21` (William signing his own ruling) and
/// `decided 2026-09-04 — william` / `decided 2026-09-09 12:27 — william …` (dated close).
fn dated_signature_len(stripped: &str) -> Option<usize> {
    let is_date = |s: &str| s.len() >= 10 && s.as_bytes()[..10].iter().enumerate().all(|(i, b)| {
        if i == 4 || i == 7 { *b == b'-' } else { b.is_ascii_digit() }
    });
    for name in ["william, ", "will, "] {
        if let Some(rest) = stripped.strip_prefix(name) {
            if is_date(rest) {
                return Some(name.len() + 10);
            }
        }
    }
    if let Some(rest) = stripped.strip_prefix("decided ") {
        if is_date(rest) {
            let after_date = &rest[10..];
            let after_time = after_date
                .strip_prefix(' ')
                .filter(|s| s.len() >= 5 && s.as_bytes()[2] == b':')
                .map(|s| &s[5..])
                .unwrap_or(after_date);
            for dash in [" — william"] {
                if let Some(rest) = after_time.strip_prefix(dash) {
                    let consumed = stripped.len() - rest.len();
                    return Some(consumed);
                }
            }
        }
    }
    None
}

fn has_contribution_heading(thread: &str, harness: &str) -> bool {
    let needle = format!("({harness}, ");
    thread.lines().any(|line| {
        (line.starts_with("### Position — ") || line.starts_with("### Reply — ")) && line.contains(&needle)
    })
}

/// Blind-round commitment: the staged Position's hash is written into the thread itself,
/// before any panel invocation, so the queued job (mutable plain text) is not the only
/// record of what the caller committed to. The reveal refuses a job whose content does
/// not hash to the committed value.
fn staged_marker(round: u32, caller: &str, sha256: &str) -> String {
    format!("<!-- forum-staged:{round} harness:{caller} sha256:{sha256} -->")
}

fn committed_staged_hash(thread: &str, round: u32, caller: &str) -> Option<String> {
    let prefix = format!("<!-- forum-staged:{round} harness:{caller} sha256:");
    thread.lines().find_map(|line| {
        line.trim()
            .strip_prefix(&prefix)
            .map(|rest| rest.trim_end_matches("-->").trim().to_string())
    })
}

/// Record the commitment in the thread (idempotent for an identical hash; refuses a
/// second, different commitment for the same round and caller).
fn commit_staged_hash(thread: &mut String, round: u32, caller: &str, sha256: &str) -> Result<()> {
    match committed_staged_hash(thread, round, caller) {
        Some(existing) if existing == sha256 => return Ok(()),
        Some(existing) => bail!(
            "round {round} already carries a different staged commitment for {caller} ({}… vs {}…); refusing",
            &existing[..12.min(existing.len())],
            &sha256[..12]
        ),
        None => {}
    }
    let insertion = thread
        .find("\n## Open questions")
        .ok_or_else(|| anyhow!("thread lacks an '## Open questions' section"))?;
    thread.insert_str(insertion, &format!("\n{}\n", staged_marker(round, caller, sha256)));
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
    critic: bool,
) -> String {
    let round_duties = match kind {
        ContributionKind::Position => {
            "If the Context omits a premise you dispute or a material option it should have named, say so explicitly: omission is a form of framing.".to_string()
        }
        ContributionKind::Reply => {
            let mut text = String::from(
                "End with a line beginning **Dispositions:** that states, for each central claim you made in earlier rounds, whether you retain, revise, or withdraw it; if you made none, write **Dispositions:** no earlier claims.",
            );
            if critic {
                text.push_str(
                    "\n\nYou are this round's designated critic. Before your dispositions, include a section headed **Falsification pass** stating: the strongest assumption the existing Positions share; one concrete way it fails, with the evidence or path that would show it; and what evidence would change the recommendation. Be evidence-bound, not contrarian: if the shared assumption survives your best attempt, say so.",
                );
            }
            text
        }
    };
    format!(
        "You are {name} participating in William's vendor-neutral Design Forum.\n\n\
Thread: {id}\nRound: {round}\nContribution type: {kind}\n\n\
Read the complete snapshot below. Produce an independent, substantive contribution. State a clear claim, use evidence from the snapshot or named paths, identify risks and alternatives, and say what would change if accepted. For a reply round, engage the strongest existing claims rather than merely agreeing. Stay PHI-free. Debate only: do not implement, invoke tools, edit files, or start other assistants.\n\n\
{duties}\n\n\
Return only the Markdown body of your contribution. Do not emit YAML frontmatter, a Position/Reply heading, code fences around the whole response, or commentary about the task.\n\n\
--- THREAD SNAPSHOT ---\n{snapshot}\n--- END SNAPSHOT ---\n",
        name = harness.display_name,
        id = id,
        round = round,
        kind = kind.heading(),
        duties = round_duties,
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
    append_contribution_marked(thread, author, display_name, kind, reply_to, body, round, "")
}

#[allow(clippy::too_many_arguments)]
fn append_contribution_marked(
    thread: &mut String,
    author: &str,
    display_name: &str,
    kind: ContributionKind,
    reply_to: Option<&str>,
    body: &str,
    round: Option<u32>,
    marker_extra: &str,
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
        .map(|round| format!("\n\n<!-- forum-round:{round} harness:{author}{marker_extra} -->"))
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
    let prefix = format!("<!-- forum-round:{round} harness:{harness}");
    thread.lines().any(|line| {
        let line = line.trim();
        line.strip_prefix(&prefix)
            .is_some_and(|rest| rest == " -->" || rest.starts_with(' '))
    })
}

/// Every marker as (round, harness, extra tokens such as `mode:sequential`, `role:critic`).
fn round_marker_attrs(thread: &str) -> Vec<(u32, String, Vec<String>)> {
    let mut out = Vec::new();
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
        let mut tokens = rest.trim_end_matches("-->").split_whitespace();
        let Some(harness) = tokens.next() else { continue };
        out.push((round, harness.to_string(), tokens.map(str::to_string).collect()));
    }
    out
}

fn round_is_sequential(thread: &str, round: u32) -> bool {
    round_marker_attrs(thread)
        .iter()
        .any(|(r, _, tokens)| *r == round && tokens.iter().any(|t| t == "mode:sequential"))
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
    for (round, harness, _) in round_marker_attrs(thread) {
        result.entry(round).or_default().insert(harness);
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
    const RESERVED: [&str; 6] = [
        "## Positions",
        "## Open questions",
        "## Decision",
        "## Consequences / follow-ups",
        "<!-- forum-round:",
        "<!-- forum-staged:",
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

    /// Decided, ratified by William, with a Residual dissent section: passes every gate.
    fn decided_thread() -> String {
        sample_thread()
            .replace("status: open", "status: decided")
            .replace(
                "decision: null",
                "decision: \"Adopt the bounded implementation.\"",
            )
            .replace(
                "## Decision\n\n_(none)_\n",
                "## Decision\n\nWilliam ratified the bounded implementation on 2026-09-09.\n\n### Residual dissent\n\n- Strongest rejected alternative: do nothing (held by grok-build, withdrawn after round 2).\n- Revisit trigger: a second incident.\n",
            )
    }

    /// Decided under delegation only, no ratification, no dissent section; opened after
    /// the dissent rule so the doctor lint counts it.
    fn delegated_thread() -> String {
        sample_thread()
            .replace("status: open", "status: decided")
            .replace("opened: 2026-07-17", "opened: 2026-09-09")
            .replace(
                "decision: null",
                "decision: \"Adopt the bounded implementation.\"",
            )
            .replace(
                "## Decision\n\n_(none)_\n",
                "## Decision\n\nDecided under William's explicit delegation after two rounds.\n",
            )
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
            with_position: None,
            critic: None,
            shadow: None,
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
                with_position: None,
                critic: None,
                shadow: None,
            },
        )
        .unwrap();
        assert!(process_next_job(temp.path(), &config).unwrap());
        assert!(job_files(&queue_dir(temp.path())).unwrap().is_empty());
        assert_eq!(job_files(&completed_dir(temp.path())).unwrap().len(), 1);
        assert_eq!(
            receipt_files(&unread_inbox_dir(temp.path())).unwrap().len(),
            1
        );
        let thread = fs::read_to_string(temp.path().join("meta/thread.md")).unwrap();
        assert!(thread.contains("**Claim:** queued worker ran"));
        assert!(thread.contains("<!-- forum-round:1 harness:fake -->"));

        let receipts = read_receipts(&unread_inbox_dir(temp.path()), false).unwrap();
        assert_eq!(receipts[0].1.thread_id, "test-thread");
        assert_eq!(receipts[0].1.round, 1);
        cmd_acknowledge(temp.path(), "test-thread").unwrap();
        assert!(receipt_files(&unread_inbox_dir(temp.path()))
            .unwrap()
            .is_empty());
        assert_eq!(
            receipt_files(&acknowledged_inbox_dir(temp.path()))
                .unwrap()
                .len(),
            1
        );
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

    #[test]
    fn work_order_contains_the_required_bounds_and_traceability() {
        let work_order = build_work_order(
            "test-thread",
            "Adopt the bounded implementation.",
            "codex",
            "will",
            &["forum dispatch command only".into()],
            &["all forum tests pass".into()],
            &["claude-code".into(), "grok-build".into()],
            Path::new("meta/thread.md"),
            "- Strongest rejected alternative: none\n- Revisit trigger: none",
        );
        assert!(work_order.contains("**Implementation owner:** `codex`"));
        assert!(work_order.contains("**Residual dissent (quoted from the Decision):**\n> - Strongest rejected alternative: none\n> - Revisit trigger: none"));
        assert!(work_order.contains("- forum dispatch command only"));
        assert!(work_order.contains("- all forum tests pass"));
        assert!(work_order.contains("`claude-code`, `grok-build`"));
        assert!(work_order.contains("`design-forum/meta/thread.md`"));
        assert!(work_order.contains("<!-- forum-work-order:test-thread -->"));
    }

    #[test]
    fn dispatch_dry_run_requires_a_decision_and_does_not_mutate() {
        let temp = TempDir::new().unwrap();
        write_temp_forum(&temp);
        let thread_path = temp.path().join("meta/thread.md");
        fs::write(&thread_path, decided_thread()).unwrap();
        let before = fs::read_to_string(&thread_path).unwrap();
        cmd_dispatch(
            temp.path(),
            DispatchArgs {
                id: "test-thread".into(),
                assignee: "codex".into(),
                scope: vec!["forum dispatch command".into()],
                acceptance: vec!["tests pass".into()],
                reviewers: vec!["claude-code".into()],
                requested_by: "will".into(),
                dry_run: true,
            },
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&thread_path).unwrap(), before);
    }

    #[test]
    fn dispatch_rejects_open_threads_and_non_independent_review() {
        let temp = TempDir::new().unwrap();
        write_temp_forum(&temp);
        let open_error = cmd_dispatch(
            temp.path(),
            DispatchArgs {
                id: "test-thread".into(),
                assignee: "codex".into(),
                scope: vec!["bounded scope".into()],
                acceptance: vec!["observable result".into()],
                reviewers: vec!["claude-code".into()],
                requested_by: "will".into(),
                dry_run: true,
            },
        )
        .unwrap_err();
        assert!(open_error.to_string().contains("not decided"));

        let reviewer_error = cmd_dispatch(
            temp.path(),
            DispatchArgs {
                id: "test-thread".into(),
                assignee: "codex".into(),
                scope: vec!["bounded scope".into()],
                acceptance: vec!["observable result".into()],
                reviewers: vec!["codex".into()],
                requested_by: "will".into(),
                dry_run: true,
            },
        )
        .unwrap_err();
        assert!(reviewer_error
            .to_string()
            .contains("review must be independent"));
    }

    fn fake_config(script: &str) -> Config {
        let fake = Harness {
            id: "fake".into(),
            display_name: "Fake Harness".into(),
            command: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            prompt_mode: PromptMode::Argument,
            enabled: true,
        };
        let mut harnesses = BTreeMap::new();
        harnesses.insert("fake".into(), fake);
        Config { harnesses, panels: BTreeMap::new() }
    }

    fn convene_args(id: &str, caller: &str, panel: &str) -> ConveneArgs {
        ConveneArgs {
            id: id.into(),
            caller: caller.into(),
            panel: panel.into(),
            round: None,
            new_round: false,
            kind: None,
            dry_run: false,
            background: false,
            max_attempts: 1,
            with_position: None,
            critic: None,
            shadow: None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn blind_round_reveals_staged_position_together_with_the_panel() {
        let temp = TempDir::new().unwrap();
        write_temp_forum(&temp);
        let staged = temp.path().join("staged.md");
        fs::write(&staged, "**Claim:** the caller's blind position.\n").unwrap();
        let config = fake_config("printf '**Claim:** panel saw context only\\n'");
        let mut args = convene_args("test-thread", "codex", "fake");
        args.with_position = Some(staged.clone());
        cmd_convene(temp.path(), &config, args).unwrap();

        let thread = fs::read_to_string(temp.path().join("meta/thread.md")).unwrap();
        assert!(thread.contains("### Position — Codex (codex,"));
        assert!(thread.contains("the caller's blind position"));
        assert!(thread.contains("<!-- forum-round:1 harness:codex -->"));
        assert!(thread.contains("<!-- forum-round:1 harness:fake -->"));
        assert!(!thread.contains("mode:sequential"));
        assert!(thread.find("harness:codex").unwrap() < thread.find("harness:fake").unwrap());

        // The panel's snapshot and prompt were built before the reveal: Context only.
        let jobs = orchestrator_dir(temp.path()).join("local-state/jobs");
        let job = fs::read_dir(&jobs).unwrap().next().unwrap().unwrap().path();
        let snapshot = fs::read_to_string(job.join("snapshot.md")).unwrap();
        let prompt = fs::read_to_string(job.join("fake-prompt.md")).unwrap();
        assert!(!snapshot.contains("blind position"));
        assert!(!prompt.contains("blind position"));
        assert!(prompt.contains("omission is a form of framing"));
        let hash = sha256_hex("**Claim:** the caller's blind position.\n");
        assert!(snapshot.contains(&staged_marker(1, "codex", &hash)), "commitment precedes the panel");
        assert_eq!(committed_staged_hash(&thread, 1, "codex").as_deref(), Some(hash.as_str()));
        assert_eq!(
            fs::read_to_string(job.join("staged-position.sha256")).unwrap(),
            sha256_hex("**Claim:** the caller's blind position.\n")
        );
    }

    #[cfg(unix)]
    #[test]
    fn staged_position_edited_during_the_round_is_refused() {
        let temp = TempDir::new().unwrap();
        write_temp_forum(&temp);
        let staged = temp.path().join("staged.md");
        fs::write(&staged, "**Claim:** original.\n").unwrap();
        // The fake harness tampers with the staged file while the round runs.
        let script = format!(
            "printf '**Claim:** panel ran\\n'; printf 'sneaky edit\\n' >> '{}'",
            staged.display()
        );
        let config = fake_config(&script);
        let mut args = convene_args("test-thread", "codex", "fake");
        args.with_position = Some(staged);
        let error = cmd_convene(temp.path(), &config, args).unwrap_err();
        assert!(format!("{error:#}").contains("edited after the round started"), "{error:#}");
        assert!(format!("{error:#}").contains("nothing written"), "{error:#}");
        // Fail closed: neither the panel's text nor the caller's reaches the thread; the
        // commitment marker written before invocation remains as the record.
        let thread = fs::read_to_string(temp.path().join("meta/thread.md")).unwrap();
        assert!(!thread.contains("panel ran"));
        assert!(!thread.contains("original."));
        assert!(!has_contribution_heading(&thread, "codex"));
        assert!(committed_staged_hash(&thread, 1, "codex").is_some());
    }

    #[test]
    fn queued_blind_round_is_hash_locked() {
        let temp = TempDir::new().unwrap();
        write_temp_forum(&temp);
        let staged = temp.path().join("staged.md");
        fs::write(&staged, "**Claim:** queued blind position.\n").unwrap();
        let config = fake_config("printf '**Claim:** worker ran\\n'");
        let mut args = convene_args("test-thread", "codex", "fake");
        args.background = true;
        args.with_position = Some(staged);
        enqueue_convene(temp.path(), &config, args).unwrap();
        let job_path = job_files(&queue_dir(temp.path())).unwrap().remove(0);
        let mut job: QueueJob = toml::from_str(&fs::read_to_string(&job_path).unwrap()).unwrap();
        assert_eq!(job.staged_position.as_deref(), Some("**Claim:** queued blind position.\n"));
        assert_eq!(job.staged_position_sha256.as_deref(), Some(sha256_hex("**Claim:** queued blind position.\n").as_str()));

        // The commitment is in the thread, not only in the mutable job.
        let thread = fs::read_to_string(temp.path().join("meta/thread.md")).unwrap();
        assert_eq!(committed_staged_hash(&thread, 1, "codex"), job.staged_position_sha256);

        // Substitute BOTH content and hash after enqueue (an internally consistent pair):
        // the worker must still refuse, because the thread's commitment disagrees.
        let substituted = "**Claim:** substituted after enqueue.\n";
        job.staged_position = Some(substituted.into());
        job.staged_position_sha256 = Some(sha256_hex(substituted));
        fs::write(&job_path, toml::to_string_pretty(&job).unwrap()).unwrap();
        assert!(process_next_job(temp.path(), &config).unwrap());
        assert_eq!(job_files(&failed_dir(temp.path())).unwrap().len(), 1);
        let failed: QueueJob =
            toml::from_str(&fs::read_to_string(&job_files(&failed_dir(temp.path())).unwrap()[0]).unwrap()).unwrap();
        // Refused at pre-flight (before any model call) because the thread already carries
        // a different commitment for this round and caller.
        assert!(failed.last_error.as_deref().unwrap_or("").contains("commitment"), "{:?}", failed.last_error);
        let thread = fs::read_to_string(temp.path().join("meta/thread.md")).unwrap();
        assert!(!thread.contains("substituted after enqueue"));
        assert!(!thread.contains("worker ran"), "fail closed: the panel's text is not published either");
    }

    #[cfg(unix)]
    #[test]
    fn retry_after_a_completed_reveal_cannot_append_an_adapted_position() {
        // Crash window: the worker wrote the revealed round, then died before archiving the
        // job. The caller, who can now read the panel, rewrites the queued content and hash.
        let temp = TempDir::new().unwrap();
        write_temp_forum(&temp);
        let staged = temp.path().join("staged.md");
        fs::write(&staged, "**Claim:** committed before the panel.\n").unwrap();
        let config = fake_config("printf '**Claim:** panel text\\n'");
        let mut args = convene_args("test-thread", "codex", "fake");
        args.background = true;
        args.with_position = Some(staged);
        enqueue_convene(temp.path(), &config, args).unwrap();
        let queued = job_files(&queue_dir(temp.path())).unwrap().remove(0);
        let original_job = fs::read_to_string(&queued).unwrap();
        assert!(process_next_job(temp.path(), &config).unwrap());
        let revealed = fs::read_to_string(temp.path().join("meta/thread.md")).unwrap();
        assert!(revealed.contains("committed before the panel"));

        // Simulate the crash: the same job is back in the queue, adapted by the caller.
        let mut job: QueueJob = toml::from_str(&original_job).unwrap();
        let adapted = "**Claim:** adapted after reading the panel.\n";
        job.staged_position = Some(adapted.into());
        job.staged_position_sha256 = Some(sha256_hex(adapted));
        fs::write(&queued, toml::to_string_pretty(&job).unwrap()).unwrap();
        assert!(process_next_job(temp.path(), &config).unwrap());

        let after = fs::read_to_string(temp.path().join("meta/thread.md")).unwrap();
        assert_eq!(after, revealed, "retry changes nothing in the thread");
        assert_eq!(after.matches("### Position — Codex").count(), 1);
        assert_eq!(after.matches("harness:fake").count(), 1);
        assert!(!after.contains("adapted after reading"));
        assert!(job_files(&queue_dir(temp.path())).unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn blind_round_with_a_failed_invocation_reveals_nothing() {
        let temp = TempDir::new().unwrap();
        write_temp_forum(&temp);
        let staged = temp.path().join("staged.md");
        fs::write(&staged, "**Claim:** staged.\n").unwrap();
        let mut config = fake_config("printf '**Claim:** good harness\\n'");
        config.harnesses.insert(
            "broken".into(),
            Harness {
                id: "broken".into(),
                display_name: "Broken".into(),
                command: "/bin/sh".into(),
                args: vec!["-c".into(), "exit 1".into()],
                prompt_mode: PromptMode::Argument,
                enabled: true,
            },
        );
        let mut args = convene_args("test-thread", "codex", "fake,broken");
        args.background = true;
        args.with_position = Some(staged);
        enqueue_convene(temp.path(), &config, args).unwrap();
        assert!(process_next_job(temp.path(), &config).unwrap());
        let thread = fs::read_to_string(temp.path().join("meta/thread.md")).unwrap();
        assert!(!thread.contains("good harness"), "partial reveal must not happen");
        assert!(!thread.contains("**Claim:** staged."));
        assert!(committed_staged_hash(&thread, 1, "codex").is_some());
        assert!(round_markers(&thread).is_empty());
        let failed: QueueJob =
            toml::from_str(&fs::read_to_string(&job_files(&failed_dir(temp.path())).unwrap()[0]).unwrap()).unwrap();
        assert!(failed.last_error.as_deref().unwrap_or("").contains("blind round not revealed"), "{:?}", failed.last_error);
    }

    #[test]
    fn completion_receipt_quotes_residual_dissent_when_present() {
        let temp = TempDir::new().unwrap();
        write_temp_forum(&temp);
        let path = temp.path().join("meta/thread.md");
        let thread = sample_thread().replace(
            "## Decision\n\n_(none)_\n",
            "## Proposed decision (awaiting Will)\n\nRuling.\n\n### Residual dissent\n\n- Codex holds that a ledger is needed.\n\n## Decision\n\n_(none)_\n",
        );
        fs::write(&path, thread).unwrap();
        ensure_queue_dirs(temp.path()).unwrap();
        let job = QueueJob {
            version: 1,
            job_id: "test-thread-r2-1-1".into(),
            thread_id: "test-thread".into(),
            caller: "will".into(),
            panel: "fake".into(),
            round: 2,
            kind: ContributionKind::Reply,
            attempts: 1,
            max_attempts: 1,
            created_at: "2026-09-09T00:00:00+01:00".into(),
            next_attempt_at: 0,
            completed_at: Some("2026-09-09T00:01:00+01:00".into()),
            last_error: None,
            staged_position: None,
            staged_position_sha256: None,
            critic: None,
            shadow: None,
        };
        publish_completion(temp.path(), &job).unwrap();
        let receipts = read_receipts(&unread_inbox_dir(temp.path()), false).unwrap();
        assert_eq!(
            receipts[0].1.residual_dissent.as_deref(),
            Some("- Codex holds that a ledger is needed.")
        );
    }

    #[test]
    fn a_second_different_commitment_for_the_same_round_is_refused() {
        let mut thread = sample_thread();
        commit_staged_hash(&mut thread, 1, "codex", &sha256_hex("a")).unwrap();
        commit_staged_hash(&mut thread, 1, "codex", &sha256_hex("a")).unwrap();
        assert_eq!(thread.matches("forum-staged:1").count(), 1);
        assert!(commit_staged_hash(&mut thread, 1, "codex", &sha256_hex("b")).is_err());
        assert!(validate_contribution("<!-- forum-staged:1 harness:codex sha256:x -->").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn caller_posting_before_convening_marks_the_round_sequential() {
        let temp = TempDir::new().unwrap();
        write_temp_forum(&temp);
        let path = temp.path().join("meta/thread.md");
        let mut thread = fs::read_to_string(&path).unwrap();
        append_contribution(&mut thread, "codex", "Codex", ContributionKind::Position, None, "**Claim:** first, framing everyone.", None).unwrap();
        fs::write(&path, &thread).unwrap();
        let config = fake_config("printf '**Claim:** anchored\\n'");
        cmd_convene(temp.path(), &config, convene_args("test-thread", "codex", "fake")).unwrap();
        let thread = fs::read_to_string(&path).unwrap();
        assert!(thread.contains("<!-- forum-round:1 harness:fake mode:sequential -->"));
        assert!(round_is_sequential(&thread, 1));
        assert!(has_round_marker(&thread, 1, "fake"));
        assert_eq!(round_markers(&thread).get(&1).unwrap().len(), 1);

        // And --with-position cannot pretend such a round was blind.
        let staged = temp.path().join("staged.md");
        fs::write(&staged, "**Claim:** too late.\n").unwrap();
        let mut args = convene_args("test-thread", "codex", "fake");
        args.round = Some(1);
        args.with_position = Some(staged);
        let error = cmd_convene(temp.path(), &config, args).unwrap_err();
        assert!(error.to_string().contains("sequential"));
    }

    #[cfg(unix)]
    #[test]
    fn shadow_samples_stay_beside_the_thread_and_out_of_later_snapshots() {
        let temp = TempDir::new().unwrap();
        write_temp_forum(&temp);
        let config = fake_config("printf '**Claim:** SHADOW-OR-PANEL\\n'");
        let mut args = convene_args("test-thread", "will", "fake");
        args.shadow = Some("fake:2".into());
        cmd_convene(temp.path(), &config, args).unwrap();
        let path = temp.path().join("meta/thread.md");
        let thread = fs::read_to_string(&path).unwrap();
        assert_eq!(thread.matches("SHADOW-OR-PANEL").count(), 1, "only the panel contribution enters the thread");
        assert!(!thread.contains("forum-shadow"));
        assert_eq!(shadow_sample_count(&path, "test-thread").unwrap(), 2);
        let sample = fs::read_to_string(shadow_dir(&path).join("test-thread/r1-fake-1.md")).unwrap();
        assert!(sample.starts_with("<!-- forum-shadow thread:test-thread round:1 harness:fake sample:1"));
        // The .shadow directory is invisible to thread resolution and to the round-2 snapshot.
        assert!(resolve_thread(temp.path(), "test-thread").unwrap().unwrap().ends_with("meta/thread.md"));
        assert_eq!(choose_round(&thread, None, true), 2);
        assert!(!fs::read_to_string(&path).unwrap().contains("forum-shadow"));
        assert!(parse_shadow_spec("fake:0", &config).is_err());
        assert!(parse_shadow_spec("fake:11", &config).is_err());
        assert!(parse_shadow_spec("nobody:2", &config).is_err());
    }

    #[test]
    fn critic_rotates_deterministically_and_gets_the_falsification_duty() {
        let config = default_config();
        let panel = resolve_panel(&config, "core", "will").unwrap();
        let first = choose_critic(Some("auto"), "some-thread", &panel).unwrap().unwrap();
        let again = choose_critic(Some("auto"), "some-thread", &panel).unwrap().unwrap();
        assert_eq!(first, again);
        assert!(panel.iter().any(|h| h.id == first));
        assert!(choose_critic(Some("nobody"), "some-thread", &panel).is_err());
        assert!(choose_critic(None, "some-thread", &panel).unwrap().is_none());
        // Different thread ids spread the duty across the panel.
        let spread: BTreeSet<String> = (0..50)
            .map(|i| choose_critic(Some("auto"), &format!("thread-{i}"), &panel).unwrap().unwrap())
            .collect();
        assert_eq!(spread.len(), panel.len());

        let harness = &panel[0];
        let critic_prompt = build_prompt("t", 2, ContributionKind::Reply, harness, "snap", true);
        let reply_prompt = build_prompt("t", 2, ContributionKind::Reply, harness, "snap", false);
        let position_prompt = build_prompt("t", 1, ContributionKind::Position, harness, "snap", false);
        assert!(critic_prompt.contains("Falsification pass"));
        assert!(critic_prompt.contains("**Dispositions:**"));
        assert!(reply_prompt.contains("**Dispositions:**"));
        assert!(!reply_prompt.contains("Falsification pass"));
        assert!(!position_prompt.contains("Dispositions"));
    }

    #[test]
    fn markers_with_extra_tokens_still_parse() {
        let thread = "<!-- forum-round:1 harness:codex -->\n<!-- forum-round:1 harness:grok-build mode:sequential -->\n<!-- forum-round:2 harness:claude-code role:critic -->\n";
        assert!(has_round_marker(thread, 1, "codex"));
        assert!(has_round_marker(thread, 1, "grok-build"));
        assert!(!has_round_marker(thread, 1, "grok"));
        assert!(has_round_marker(thread, 2, "claude-code"));
        assert!(!has_round_marker(thread, 2, "codex"));
        assert_eq!(round_markers(thread).get(&1).unwrap().len(), 2);
        assert!(round_is_sequential(thread, 1));
        assert!(!round_is_sequential(thread, 2));
        let attrs = round_marker_attrs(thread);
        assert_eq!(attrs[2], (2, "claude-code".to_string(), vec!["role:critic".to_string()]));
    }

    #[test]
    fn ratification_is_distinct_from_delegation() {
        // Prescribed and legacy attestations (line-anchored).
        for positive in [
            "**DECIDED — William ratified in conversation.**",
            "Ratified by William 2026-09-09.",
            "Ratified by William, ratifying the proposal above.",
            "**Decided by William, 2026-08-05**, ratifying the proposed decision above in full.",
            "Accepted by William on 2026-09-03 after two forum rounds.",
            "### Decision — William, 2026-07-18",
            "**DECIDED 2026-09-04 — William, after three rounds.**",
            "**DECIDED 2026-09-09 12:27 \u{2014} William ratified in conversation with claude-code:** \"yes\". Rulings: (a) both a gate and a lint, not lint alone.",
            "**Will, 2026-08-21.** Parent logs are **hubs**, not rolled-up family timelines.",
            "William ratifies the panel's proposal.",
            "## Decision - William, 2026-09-09",
            "William decided on 2026-08-25 that Continuum will own the record.",
            "William accepted the panel's convergent design and asked Codex to implement it.",
            "Decided by William, ratifying the wording above.",
        ] {
            assert!(has_william_ratification(positive), "false negative: {positive}");
        }
        // Delegation, deferral, reversal, quotation, mention — none attest.
        for negative in [
            "Decided under William's explicit delegation after two rounds.",
            "**DECIDED 2026-09-08 — William delegated; consensus after 2 rounds.**",
            "Consensus after two rounds; William delegated.",
            "This decision is not ratified by William.",
            "Ratification by William remains outstanding.",
            "The dissent argued that \u{201c}ratified by William\u{201d} should be required.",
            "Codex challenged the claim that \u{201c}this change was accepted by William\u{201d} and requested primary evidence.",
            "awaiting William's ratification",
            "William will ratify later.",
            "William decided to defer ratification until monitoring completes.",
            "This will be decided by William after the critic pass.",
            "Once William has approved the wording, dispatch may proceed.",
            "Until William has ratified, treat this as a draft.",
            "William has decided against this approach.",
            "William accepted the residual dissent as recorded.",
            "Ratified by William if the tests pass.",
            "Decided by William? Not yet.",
            "Decided by William; awaiting monitoring.",
            "> Ratified by William 2026-09-09",
            "> **DECIDED 2026-09-09 12:27 \u{2014} William ratified in conversation with claude-code:**",
            "- Decided by William in the parent; this close is delegated.",
            "see William's Decision in the parent",
            "William approved this decision.",
            "Ratifier: Will",
            "",
        ] {
            assert!(!has_william_ratification(negative), "false positive: {negative}");
        }
        // Attestation-shaped text inside Residual dissent does not count.
        let dissent_only = "Decided under delegation.\n\n### Residual dissent\n\n- Ratified by William was demanded by Codex.\n";
        assert!(!has_william_ratification(dissent_only));
        let with_both = "Ratified by William, 2026-09-10.\n\n### Residual dissent\n\n- none live\n";
        assert!(has_william_ratification(with_both));

        let thread = decided_thread();
        let decision = decision_section(&thread).unwrap();
        assert!(decision.starts_with("William ratified"));
        let dissent = residual_dissent_section(&decision).unwrap();
        assert!(dissent.starts_with("- Strongest rejected alternative"));
        assert!(dissent.contains("Revisit trigger"));
        assert!(residual_dissent_section(&delegated_thread()).is_none());

        let proposed = "## Context\n\nx\n\n## Proposed decision (awaiting Will)\n\nRuling.\n\n### Residual dissent\n\n- Codex holds X.\n\n## Decision\n\n_(none)_\n";
        assert_eq!(decision_section(proposed).unwrap(), "Ruling.\n\n### Residual dissent\n\n- Codex holds X.");
    }

    #[test]
    fn dated_signatures_anchor_only_at_line_start() {
        assert_eq!(dated_signature_len("will, 2026-08-21. parent logs"), Some(16));
        assert_eq!(dated_signature_len("william, 2026-08-21"), Some(19));
        assert_eq!(
            dated_signature_len("decided 2026-09-04 \u{2014} william, after three rounds."),
            Some("decided 2026-09-04 \u{2014} william".len())
        );
        assert_eq!(
            dated_signature_len("decided 2026-09-09 12:27 \u{2014} william ratified in conversation"),
            Some("decided 2026-09-09 12:27 \u{2014} william".len())
        );
        assert_eq!(dated_signature_len("decided 2026-09-08 under william's delegation"), None);
        assert_eq!(dated_signature_len("will, yesterday"), None);
        assert_eq!(dated_signature_len("> decided 2026-09-04 \u{2014} william"), None);
        // The parent heading, blockquoted into a child, does not attest for the child.
        assert!(!has_william_ratification("> **DECIDED 2026-09-04 \u{2014} William, after three rounds.**"));
        assert!(has_william_ratification("**DECIDED 2026-09-04 \u{2014} William, after three rounds.**"));
    }

    #[test]
    fn thread_inbox_says_what_each_thread_needs() {
        let awaiting = sample_thread().replace(
            "## Decision\n\n_(none)_\n",
            "## Proposed decision (awaiting Will)\n\nRuling.\n\n## Decision\n\n_(none)_\n",
        );
        assert!(thread_need(&awaiting, "open").starts_with("NEEDS YOUR RATIFICATION"));
        let mentions = sample_thread().replace("Context.", "Ends at `## Proposed decision (awaiting Will)` under delegation.");
        assert_eq!(thread_need(&mentions, "open"), "needs a decision or another round");
        let reviewed = sample_thread().replace(
            "_(awaiting positions)_",
            "### Position \u{2014} Codex (codex, 2026-09-09)\n\n**FAIL.** x\n\n### Reply \u{2014} Grok Build (grok-build, 2026-09-10)\n\n**PASS with defects** against `a3b9b96`.\n",
        );
        assert_eq!(latest_verdict(&reviewed), Some(("grok-build".into(), "PASS with defects".into())));
        assert!(thread_need(&reviewed, "open").starts_with("verdict PASS with defects (grok-build)"));
        assert_eq!(thread_need(&sample_thread(), "open"), "needs a decision or another round");
        let done = decided_thread() + "\n<!-- forum-dispatch:test-thread -->\n### Implementation receipt\n";
        assert_eq!(thread_need(&done, "decided"), "decided; implemented and receipted");
    }

    #[test]
    fn decision_section_prefers_a_real_decision_over_a_proposal() {
        // A ratified `## Decision` wins over an earlier `## Proposed decision`.
        let both = "## Proposed decision (awaiting Will)\n\nPanel draft.\n\n## Decision\n\n**Decided by William, 2026-08-05**, ratifying the proposal.\n\n### Residual dissent\n\n- none live\n";
        assert!(decision_section(both).unwrap().starts_with("**Decided by William"));
        // The template placeholder (exact text `forum open` emits) yields to the proposal.
        let placeholder = "## Proposed decision (awaiting Will)\n\nPanel draft.\n\n## Decision\n\n_(none yet \u{2014} awaiting positions/replies and William)_\n";
        assert_eq!(decision_section(placeholder).unwrap(), "Panel draft.");
        // An "almost placeholder" binds `## Decision`: no attestation there, so dispatch refuses (safe).
        let almost = "## Proposed decision (awaiting Will)\n\nRatified by William.\n\n## Decision\n\n_(awaiting William)_ with extra words\n";
        assert_eq!(decision_section(almost).unwrap(), "_(awaiting William)_ with extra words");
        assert!(!has_william_ratification(&decision_section(almost).unwrap()));
        // A `### Decision` heading inside another section is not a section start.
        let nested = "## Context\n\n### Decision history\n\nold\n\n## Decision\n\nRatified by William.\n";
        assert_eq!(decision_section(nested).unwrap(), "Ratified by William.");
        // The two-tier close: proposal body with future-tense prose does not attest.
        let awaiting = "## Proposed decision (awaiting Will)\n\nRuling. This will be decided by William after the critic pass.\n\n## Decision\n\n_(none yet \u{2014} awaiting positions/replies and William)_\n";
        assert!(!has_william_ratification(&decision_section(awaiting).unwrap()));
    }

    #[test]
    fn dispatch_gates_on_dissent_ratification_and_sequential_rounds() {
        let temp = TempDir::new().unwrap();
        write_temp_forum(&temp);
        let path = temp.path().join("meta/thread.md");
        let args = || DispatchArgs {
            id: "test-thread".into(),
            assignee: "codex".into(),
            scope: vec!["bounded scope".into()],
            acceptance: vec!["observable result".into()],
            reviewers: vec!["claude-code".into()],
            requested_by: "will".into(),
            dry_run: true,
        };

        // 1. Decided under delegation, no dissent section → refused on the section.
        fs::write(&path, delegated_thread()).unwrap();
        let error = cmd_dispatch(temp.path(), args()).unwrap_err();
        assert!(error.to_string().contains("Residual dissent"), "{error:#}");

        // 2. Dissent present but architecture without ratification → refused on ratification.
        let with_dissent = delegated_thread().replace(
            "Decided under William's explicit delegation after two rounds.\n",
            "Decided under William's explicit delegation after two rounds.\n\n### Residual dissent\n\n- None held after round 2 (grok-build withdrew).\n",
        );
        fs::write(&path, &with_dissent).unwrap();
        let error = cmd_dispatch(temp.path(), args()).unwrap_err();
        assert!(error.to_string().contains("William's ratification"), "{error:#}");

        // 3. Same Decision on a module-level thread passes the architecture gate...
        let module = with_dissent.replace("level: architecture", "level: module");
        fs::write(&path, &module).unwrap();
        cmd_dispatch(temp.path(), args()).unwrap();

        // 4. ...unless round 1 was sequential, which cannot close under delegation.
        let sequential = module.replace(
            "_(awaiting positions)_",
            "### Position — Codex (codex, 2026-09-09)\n\nFraming.\n\n<!-- forum-round:1 harness:fake mode:sequential -->\n",
        );
        fs::write(&path, &sequential).unwrap();
        let error = cmd_dispatch(temp.path(), args()).unwrap_err();
        assert!(error.to_string().contains("sequential"), "{error:#}");

        // 5. Ratified with dissent → dispatches (dry run) even at architecture level.
        fs::write(&path, decided_thread()).unwrap();
        cmd_dispatch(temp.path(), args()).unwrap();
    }

    #[test]
    fn doctor_lints_flag_unratified_architecture_closes() {
        let temp = TempDir::new().unwrap();
        write_temp_forum(&temp);
        let path = temp.path().join("meta/thread.md");
        fs::write(&path, delegated_thread()).unwrap();
        let lints = doctor_lints(temp.path()).unwrap();
        assert!(lints.iter().any(|l| l.starts_with("LINT architecture decided without William's ratification") && l.contains("test-thread")), "{lints:?}");
        assert!(lints.last().unwrap().contains("1 decided thread(s); 1 architecture close(s) lack William's ratification; 1 Decision(s) opened since 2026-09-09 lack a Residual dissent section"), "{lints:?}");
        assert!(lints.iter().any(|l| l.starts_with("LINT decided without a Residual dissent section")));
        // A thread opened before the rule is exempt from the dissent count.
        fs::write(&path, delegated_thread().replace("opened: 2026-09-09", "opened: 2026-08-01")).unwrap();
        let lints = doctor_lints(temp.path()).unwrap();
        assert!(lints.last().unwrap().contains("0 Decision(s) opened since"), "{lints:?}");

        fs::write(&path, decided_thread()).unwrap();
        let lints = doctor_lints(temp.path()).unwrap();
        assert!(!lints.iter().any(|l| l.starts_with("LINT")), "{lints:?}");
        assert!(lints.last().unwrap().contains("0 architecture close(s) lack"));

        // An open thread and a dot-directory sample are ignored.
        fs::write(&path, sample_thread()).unwrap();
        fs::create_dir_all(temp.path().join("meta/.shadow")).unwrap();
        fs::write(temp.path().join("meta/.shadow/test-thread-r1-fake-1.md"), delegated_thread()).unwrap();
        let lints = doctor_lints(temp.path()).unwrap();
        assert!(lints.last().unwrap().contains("0 decided thread(s)"));
    }
}
