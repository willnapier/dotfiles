//! Safe Git history coordination for the Syncthing-transported Assistants tree.
//!
//! Syncthing owns working-file transport. Exactly one historian may create and
//! push commits. A follower may fetch and advance its local branch and index
//! only after proving that the already-synchronised working tree exactly equals
//! the remote tree. Neither role runs checkout, pull, merge, rebase, or a
//! worktree-writing reset.

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Role {
    Historian,
    Follower,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Self::Historian => "historian",
            Self::Follower => "follower",
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "assistants-git-sync", version, about)]
struct Cli {
    /// This host's role. Only historian can create or push commits.
    #[arg(long, value_enum)]
    role: Role,

    /// Refuse to start unless the normalised hostname matches this value.
    #[arg(long)]
    expected_host: String,

    /// Assistants repository.
    #[arg(long, default_value_os_t = default_repo())]
    repo: PathBuf,

    /// Syncthing folder ID used by the historian's publication gate.
    #[arg(long, default_value = "Assistants")]
    syncthing_folder: String,

    /// Seconds between checks.
    #[arg(long, default_value_t = 30)]
    tick: u64,

    /// Seconds the newest dirty file must remain untouched before committing.
    #[arg(long, default_value_t = 90)]
    quiet: u64,

    /// Run one cycle and exit.
    #[arg(long)]
    once: bool,

    /// Report a safe action but do not move refs, commit, or push.
    #[arg(long)]
    dry_run: bool,

    /// Append-only operational log.
    #[arg(long, default_value_os_t = default_log())]
    log: PathBuf,

    /// Directory holding the atomic heartbeat JSON.
    #[arg(long, default_value_os_t = default_state_dir())]
    state_dir: PathBuf,
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn default_repo() -> PathBuf {
    home().join("Assistants")
}

fn default_log() -> PathBuf {
    home().join(".local/share/assistants-git-sync.log")
}

fn default_state_dir() -> PathBuf {
    home().join(".local/state/watchers")
}

fn lock_path() -> PathBuf {
    // One lock for either role. An accidentally duplicated service on one
    // host must not be able to run historian and follower concurrently.
    PathBuf::from("/tmp/assistants-git-sync.lock")
}

const LEGACY_ASSISTANTS_PUSH_LOCK: &str = "/tmp/git-auto-push-watcher-Assistants.lock";

fn main() -> Result<()> {
    let cli = Cli::parse();
    verify_host(&cli.expected_host)?;

    let logger = Logger {
        path: cli.log.clone(),
    };
    if let Some(parent) = cli.log.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    if !cli.once {
        take_lock(&lock_path(), &logger)?;
        logger.log(&format!(
            "starting assistants-git-sync {} as {} on {} — repo {}, tick {}s, quiet {}s",
            env!("CARGO_PKG_VERSION"),
            cli.role.as_str(),
            host_label(),
            cli.repo.display(),
            cli.tick,
            cli.quiet
        ));
    }

    let mut heartbeat = Heartbeat::new(&cli.state_dir, cli.role, cli.tick);
    let mut quiescence = Quiescence::default();
    loop {
        let sync_folder = cli.syncthing_folder.clone();
        let sync_gate = || syncthing_ready(&sync_folder);
        let outcome = cycle(
            &cli.repo,
            cli.role,
            Duration::from_secs(cli.quiet),
            cli.dry_run,
            &logger,
            &sync_gate,
            &mut quiescence,
        );
        heartbeat.record(&outcome);

        match &outcome {
            Ok(Outcome::Waiting(message)) => logger.log(&format!("waiting: {message}")),
            Ok(Outcome::Blocked(message)) => logger.log(&format!("blocked: {message}")),
            Ok(Outcome::Adopted { from, to }) => logger.log(&format!(
                "adopted origin/main without writing worktree files: {from} -> {to}"
            )),
            Ok(Outcome::Pushed(subject)) => logger.log(&format!("pushed: {subject}")),
            Ok(Outcome::PushFailed(message)) => logger.log(&format!("push failed: {message}")),
            Ok(Outcome::DryRun(message)) => logger.log(&format!("dry-run: {message}")),
            Ok(Outcome::Clean) => {}
            Err(error) => logger.log(&format!("cycle failed: {error:#}")),
        }

        if cli.once {
            if matches!(
                &outcome,
                Ok(Outcome::Blocked(_)) | Ok(Outcome::PushFailed(_)) | Err(_)
            ) {
                std::process::exit(1);
            }
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(cli.tick));
    }
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Clean,
    Waiting(String),
    Blocked(String),
    DryRun(String),
    Adopted { from: String, to: String },
    Pushed(String),
    PushFailed(String),
}

#[derive(Clone, Debug, PartialEq)]
struct SyncReadiness {
    ready: bool,
    detail: String,
}

fn cycle(
    repo: &Path,
    role: Role,
    quiet: Duration,
    dry_run: bool,
    logger: &Logger,
    sync_gate: &dyn Fn() -> Result<SyncReadiness>,
    quiescence: &mut Quiescence,
) -> Result<Outcome> {
    if let Some(pid) = live_pid_lock(Path::new(LEGACY_ASSISTANTS_PUSH_LOCK)) {
        return Ok(Outcome::Blocked(format!(
            "legacy Assistants auto-pusher is still running as pid {pid}; dual-writer recurrence refused"
        )));
    }
    ensure_main(repo)?;

    let conflicts = sync_conflicts(repo)?;
    if !conflicts.is_empty() {
        return Ok(Outcome::Blocked(format!(
            "Syncthing conflict copies exist: {}",
            conflicts.join(", ")
        )));
    }

    let fetch = git(repo, &["fetch", "-q", "origin", "main"])?;
    if !fetch.ok {
        return Ok(Outcome::Blocked(format!(
            "git fetch failed: {}",
            fetch.err.trim()
        )));
    }

    let head = rev_parse(repo, "HEAD")?;
    let remote = rev_parse(repo, "origin/main")?;

    if head != remote {
        if is_ancestor(repo, &head, &remote)? {
            if !worktree_matches(repo, &remote)? {
                return Ok(Outcome::Blocked(
                    "origin/main is ahead, but the Syncthing worktree is not an exact tree match; refusing to move HEAD or write files"
                        .into(),
                ));
            }
            if dry_run {
                return Ok(Outcome::DryRun(format!(
                    "would adopt origin/main {head} -> {remote} without writing worktree files"
                )));
            }
            adopt(repo, &head, &remote)?;
            quiescence.reset();
            return Ok(Outcome::Adopted {
                from: head,
                to: remote,
            });
        }

        if role == Role::Historian && is_ancestor(repo, &remote, &head)? {
            if dry_run {
                return Ok(Outcome::DryRun(format!(
                    "would retry push of historian-only commits {remote} -> {head}"
                )));
            }
            return push_head(repo, "previously committed historian changes", logger);
        }

        return Ok(Outcome::Blocked(format!(
            "{} must not create history: HEAD {head} and origin/main {remote} have diverged or local HEAD is ahead",
            role.as_str()
        )));
    }

    let paths = changed_paths(repo)?;
    if paths.is_empty() {
        quiescence.reset();
        return Ok(Outcome::Clean);
    }

    if role == Role::Follower {
        return Ok(Outcome::Waiting(format!(
            "{} tracked paths await the nimbini historian",
            paths.len()
        )));
    }

    // An exact content tree, rather than mtimes, makes deletions and renames
    // obey the quiet gate too. A restart conservatively restarts the window.
    let approved_tree = worktree_tree(repo, &head)?;
    if let Some(remaining) = quiescence.remaining(&approved_tree, quiet) {
        return Ok(Outcome::Waiting(format!(
            "dirty tree has not remained unchanged for {}s; about {}s remain",
            quiet.as_secs(),
            remaining.as_secs().max(1)
        )));
    }

    let sync = sync_gate().context("checking Syncthing publication gate")?;
    if !sync.ready {
        return Ok(Outcome::Waiting(format!(
            "Syncthing publication gate is not ready: {}",
            sync.detail
        )));
    }

    // Close the foreground-commit race after the quiet and Syncthing gates.
    let second_fetch = git(repo, &["fetch", "-q", "origin", "main"])?;
    if !second_fetch.ok {
        return Ok(Outcome::Blocked(format!(
            "second git fetch failed: {}",
            second_fetch.err.trim()
        )));
    }
    let remote_after_gate = rev_parse(repo, "origin/main")?;
    let head_after_gate = rev_parse(repo, "HEAD")?;
    if remote_after_gate != head || head_after_gate != head {
        quiescence.reset();
        return Ok(Outcome::Waiting(
            "HEAD or origin/main moved while publication gates were evaluated; retrying from fresh state"
                .into(),
        ));
    }

    let failing_crates: Vec<String> = dirty_crate_roots(repo, &paths)
        .into_iter()
        .filter(|root| !cargo_check_ok(&repo.join(root)))
        .collect();
    if !failing_crates.is_empty() {
        return Ok(Outcome::Blocked(format!(
            "Rust validation failed in: {}",
            failing_crates.join(", ")
        )));
    }
    if let Some(bad) = paths
        .iter()
        .find(|path| is_nu_script(&repo.join(path)) && !nu_check_ok(&repo.join(path)))
    {
        return Ok(Outcome::Blocked(format!(
            "Nushell validation failed for {bad}"
        )));
    }

    // Validation and the REST calls can take time. Require that the exact
    // content tree which passed them is still present before creating history.
    let final_tree = worktree_tree(repo, &head)?;
    if final_tree != approved_tree {
        quiescence.observe_new(&final_tree);
        return Ok(Outcome::Waiting(
            "working tree changed while publication gates were evaluated; quiet window restarted"
                .into(),
        ));
    }

    let message = commit_message(&paths, quiet);
    let subject = message.lines().next().unwrap_or_default().to_string();
    if dry_run {
        return Ok(Outcome::DryRun(format!(
            "would commit {subject:?} after {}",
            sync.detail,
        )));
    }

    // Build directly from the proved temporary-index tree. This leaves any
    // user's live index untouched until the new ref exists.
    let commit = commit_tree(repo, &final_tree, &head, &message)?;
    let update = git(repo, &["update-ref", "refs/heads/main", &commit, &head])?;
    if !update.ok {
        return Ok(Outcome::Blocked(format!(
            "HEAD moved before historian publication; update-ref refused: {}",
            update.err.trim()
        )));
    }
    let read = git(repo, &["read-tree", &commit])?;
    if !read.ok {
        bail!(
            "historian commit exists but index refresh failed; worktree was not changed: {}",
            read.err.trim()
        );
    }
    quiescence.reset();
    logger.log(&format!("committed after {}: {subject}", sync.detail));
    push_head(repo, &subject, logger)
}

fn ensure_main(repo: &Path) -> Result<()> {
    let branch = git(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if !branch.ok {
        bail!("Assistants checkout has detached HEAD");
    }
    if branch.out.trim() != "main" {
        bail!(
            "Assistants checkout must remain on main, found {}",
            branch.out.trim()
        );
    }
    Ok(())
}

fn changed_paths(repo: &Path) -> Result<Vec<String>> {
    let status = git(repo, &["status", "--porcelain=v1", "-uall"])?;
    if !status.ok {
        bail!("git status failed: {}", status.err.trim());
    }
    let mut paths = parse_porcelain(&status.out);
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn parse_porcelain(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| line.len() > 3)
        .map(|line| {
            let path = &line[3..];
            path.split(" -> ")
                .last()
                .unwrap_or(path)
                .trim_matches('"')
                .to_string()
        })
        .collect()
}

#[derive(Default)]
struct Quiescence {
    tree: Option<String>,
    unchanged_since: Option<Instant>,
}

impl Quiescence {
    fn remaining(&mut self, tree: &str, quiet: Duration) -> Option<Duration> {
        let now = Instant::now();
        if self.tree.as_deref() != Some(tree) {
            self.tree = Some(tree.to_string());
            self.unchanged_since = Some(now);
        }
        let elapsed = self
            .unchanged_since
            .map(|since| now.saturating_duration_since(since))
            .unwrap_or(Duration::ZERO);
        quiet
            .checked_sub(elapsed)
            .filter(|remaining| !remaining.is_zero())
    }

    fn observe_new(&mut self, tree: &str) {
        self.tree = Some(tree.to_string());
        self.unchanged_since = Some(Instant::now());
    }

    fn reset(&mut self) {
        self.tree = None;
        self.unchanged_since = None;
    }
}

fn rev_parse(repo: &Path, revision: &str) -> Result<String> {
    let output = git(repo, &["rev-parse", revision])?;
    if !output.ok {
        bail!("git rev-parse {revision} failed: {}", output.err.trim());
    }
    Ok(output.out.trim().to_string())
}

fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let output = git(repo, &["merge-base", "--is-ancestor", ancestor, descendant])?;
    match output.code {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!("git merge-base failed: {}", output.err.trim()),
    }
}

fn worktree_matches(repo: &Path, revision: &str) -> Result<bool> {
    let live_tree = worktree_tree(repo, revision)?;
    let revision_tree = rev_parse(repo, &format!("{revision}^{{tree}}"))?;
    Ok(live_tree == revision_tree)
}

/// Materialise the current worktree into a temporary index seeded from the
/// target revision. This writes Git objects and a temporary index only.
fn worktree_tree(repo: &Path, revision: &str) -> Result<String> {
    let temp = tempfile::Builder::new()
        .prefix("assistants-git-sync-index-")
        .tempdir()
        .context("creating temporary Git index directory")?;
    let index = temp.path().join("index");

    let read = git_with_index(repo, &index, &["read-tree", revision])?;
    if !read.ok {
        bail!("temporary git read-tree failed: {}", read.err.trim());
    }
    let add = git_with_index(repo, &index, &["add", "-A", "--", "."])?;
    if !add.ok {
        bail!("temporary git add failed: {}", add.err.trim());
    }
    let tree = git_with_index(repo, &index, &["write-tree"])?;
    if !tree.ok {
        bail!("temporary git write-tree failed: {}", tree.err.trim());
    }
    Ok(tree.out.trim().to_string())
}

fn adopt(repo: &Path, old: &str, new: &str) -> Result<()> {
    let update = git(repo, &["update-ref", "refs/heads/main", new, old])?;
    if !update.ok {
        bail!("git update-ref refused adoption: {}", update.err.trim());
    }
    let index = git(repo, &["read-tree", new])?;
    if !index.ok {
        bail!(
            "main advanced but index refresh failed; worktree was not changed: {}",
            index.err.trim()
        );
    }
    Ok(())
}

fn commit_tree(repo: &Path, tree: &str, parent: &str, message: &str) -> Result<String> {
    let mut child = Command::new("git")
        .args(["commit-tree", tree, "-p", parent])
        .current_dir(repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running git commit-tree")?;
    child
        .stdin
        .take()
        .context("opening git commit-tree stdin")?
        .write_all(message.as_bytes())
        .context("writing git commit-tree message")?;
    let output = child
        .wait_with_output()
        .context("waiting for git commit-tree")?;
    if !output.status.success() {
        bail!(
            "git commit-tree failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn push_head(repo: &Path, subject: &str, logger: &Logger) -> Result<Outcome> {
    let mut last_error = String::new();
    for (attempt, wait) in [(1u32, 0u64), (2, 5), (3, 20)] {
        if wait > 0 {
            std::thread::sleep(Duration::from_secs(wait));
        }
        if attempt > 1 {
            logger.log(&format!("push attempt {attempt}/3"));
        }
        let push = git(repo, &["push", "origin", "HEAD:refs/heads/main"])?;
        if push.ok {
            return Ok(Outcome::Pushed(subject.to_string()));
        }
        last_error = push.err.trim().to_string();
        if last_error.contains("non-fast-forward") || last_error.contains("fetch first") {
            break;
        }
    }
    Ok(Outcome::PushFailed(last_error))
}

fn commit_message(paths: &[String], quiet: Duration) -> String {
    const LIMIT: usize = 72;
    let prefix = "Auto-commit (nimbini historian): ";
    let mut subject = prefix.to_string();
    let mut shown = 0usize;
    for path in paths {
        let separator = if shown == 0 { "" } else { ", " };
        let candidate = format!("{subject}{separator}{path}");
        let remaining = paths.len() - shown - 1;
        let suffix_len = if remaining > 0 {
            format!(", +{remaining} more").len()
        } else {
            0
        };
        if shown > 0 && candidate.chars().count() + suffix_len > LIMIT {
            break;
        }
        subject = candidate;
        shown += 1;
    }
    if shown < paths.len() {
        subject.push_str(&format!(", +{} more", paths.len() - shown));
    }

    let mut body = String::new();
    for path in paths {
        body.push_str(&format!("- {path}\n"));
    }
    format!(
        "{subject}\n\n{body}\nCommitted by the elected nimbini historian after {}s quiet.\n\nCo-Authored-By: Claude <noreply@anthropic.com>\n",
        quiet.as_secs()
    )
}

fn dirty_crate_roots(repo: &Path, paths: &[String]) -> Vec<String> {
    let mut roots: Vec<String> = paths
        .iter()
        .filter_map(|path| {
            let mut parts = path.split('/');
            match (parts.next(), parts.next()) {
                (Some("rust-projects"), Some(name)) if !name.is_empty() => {
                    Some(format!("rust-projects/{name}"))
                }
                _ => None,
            }
        })
        .filter(|root| repo.join(root).join("Cargo.toml").is_file())
        .collect();
    roots.sort();
    roots.dedup();
    roots
}

fn cargo_check_ok(dir: &Path) -> bool {
    Command::new("cargo")
        .args(["check", "--quiet", "--message-format=short"])
        .current_dir(dir)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn is_nu_script(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    if path.extension().and_then(|extension| extension.to_str()) == Some("nu") {
        return true;
    }
    let mut prefix = [0u8; 64];
    let read = File::open(path)
        .and_then(|mut file| file.read(&mut prefix))
        .unwrap_or(0);
    let first_line = String::from_utf8_lossy(&prefix[..read])
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    first_line.starts_with("#!") && (first_line.ends_with("/nu") || first_line.ends_with(" nu"))
}

fn nu_check_ok(path: &Path) -> bool {
    if !on_path("nu") {
        return true;
    }
    let quoted = format!("'{}'", path.display().to_string().replace('\'', "''"));
    let expression = format!("if (nu-check {quoted}) {{ exit 0 }} else {{ exit 1 }}");
    Command::new("nu")
        .args(["-c", &expression])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|directory| directory.join(program).is_file())
    })
}

fn sync_conflicts(repo: &Path) -> Result<Vec<String>> {
    let mut results = Vec::new();
    scan_conflicts(repo, repo, &mut results)?;
    results.sort();
    Ok(results)
}

fn scan_conflicts(root: &Path, dir: &Path, results: &mut Vec<String>) -> Result<()> {
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("reading directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry.file_type()?.is_dir() {
            if name == ".git" || name == ".stversions" {
                continue;
            }
            scan_conflicts(root, &path, results)?;
        } else if name.contains(".sync-conflict-") {
            results.push(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn syncthing_ready(folder: &str) -> Result<SyncReadiness> {
    let key_output = command("syncthing", &["cli", "config", "gui", "apikey", "get"])?;
    if !key_output.ok {
        bail!("cannot read Syncthing API key: {}", key_output.err.trim());
    }
    let key = key_output.out.trim().trim_matches('"');
    if key.is_empty() {
        bail!("Syncthing API key is empty");
    }

    let folder_encoded = query_component(folder);
    let local = syncthing_json(key, &format!("/rest/db/status?folder={folder_encoded}"))?;
    let local_state = local
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let local_need = json_u64(&local, "needTotalItems")
        + json_u64(&local, "needBytes")
        + json_u64(&local, "needDeletes");
    if local_state != "idle" || local_need != 0 {
        return Ok(SyncReadiness {
            ready: false,
            detail: format!("local folder state={local_state}, outstanding={local_need}"),
        });
    }

    let devices_output = command(
        "syncthing",
        &["cli", "config", "folders", folder, "devices", "list"],
    )?;
    if !devices_output.ok {
        bail!(
            "cannot list Syncthing folder devices: {}",
            devices_output.err.trim()
        );
    }
    let connections = syncthing_json(key, "/rest/system/connections")?;
    let connection_map = connections
        .get("connections")
        .and_then(Value::as_object)
        .context("Syncthing connections response has no connections object")?;

    let mut connected_peers = 0usize;
    for device in devices_output
        .out
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let Some(connection) = connection_map.get(device) else {
            continue;
        };
        if connection.get("connected").and_then(Value::as_bool) != Some(true)
            || connection.get("paused").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        connected_peers += 1;
        let completion = syncthing_json(
            key,
            &format!(
                "/rest/db/completion?folder={folder_encoded}&device={}",
                query_component(device)
            ),
        )?;
        let percent = completion
            .get("completion")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let outstanding = json_u64(&completion, "needItems")
            + json_u64(&completion, "needBytes")
            + json_u64(&completion, "needDeletes");
        let remote_state = completion
            .get("remoteState")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if percent < 100.0 || outstanding != 0 || remote_state != "valid" {
            return Ok(SyncReadiness {
                ready: false,
                detail: format!(
                    "peer {device} completion={percent}, outstanding={outstanding}, state={remote_state}"
                ),
            });
        }
    }

    Ok(SyncReadiness {
        ready: true,
        detail: format!(
            "local folder idle and {connected_peers} connected folder peer(s) complete"
        ),
    })
}

fn syncthing_json(api_key: &str, endpoint: &str) -> Result<Value> {
    let url = format!("http://127.0.0.1:8384{endpoint}");
    let header = format!("X-API-Key: {api_key}");
    let output = command(
        "curl",
        &[
            "--fail",
            "--silent",
            "--show-error",
            "--header",
            &header,
            &url,
        ],
    )?;
    if !output.ok {
        bail!("Syncthing API request failed: {}", output.err.trim());
    }
    serde_json::from_str(&output.out).context("parsing Syncthing API JSON")
}

fn json_u64(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn query_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn verify_host(expected: &str) -> Result<()> {
    let actual = host_label();
    if normalise_host(expected) != normalise_host(&actual) {
        bail!(
            "host fence refused role: expected {}, running on {}",
            normalise_host(expected),
            normalise_host(&actual)
        );
    }
    Ok(())
}

fn host_label() -> String {
    if let Ok(value) = std::fs::read_to_string("/etc/hostname") {
        let value = value.trim().to_string();
        if !value.is_empty() {
            return value;
        }
    }
    if let Ok(value) = std::env::var("HOSTNAME") {
        let value = value.trim().to_string();
        if !value.is_empty() {
            return value;
        }
    }
    for binary in ["/bin/hostname", "/usr/bin/hostname", "hostname"] {
        if let Ok(output) = Command::new(binary).output() {
            if output.status.success() {
                let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !value.is_empty() {
                    return value;
                }
            }
        }
    }
    "unknown".into()
}

fn normalise_host(value: &str) -> String {
    let normalised = value.trim().to_lowercase();
    normalised
        .strip_suffix(".local")
        .unwrap_or(&normalised)
        .to_string()
}

fn take_lock(lock: &Path, logger: &Logger) -> Result<()> {
    if let Ok(text) = std::fs::read_to_string(lock) {
        if let Ok(pid) = text.trim().parse::<u32>() {
            if pid != std::process::id() && pid_alive(pid) {
                bail!("already running with pid {pid}");
            }
            logger.log(&format!("removing stale lock for pid {pid}"));
        }
    }
    std::fs::write(lock, std::process::id().to_string())
        .with_context(|| format!("writing {}", lock.display()))
}

fn live_pid_lock(lock: &Path) -> Option<u32> {
    let pid = std::fs::read_to_string(lock).ok()?.trim().parse().ok()?;
    (pid != std::process::id() && pid_alive(pid)).then_some(pid)
}

fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

struct CommandOut {
    ok: bool,
    code: Option<i32>,
    out: String,
    err: String,
}

fn git(repo: &Path, args: &[&str]) -> Result<CommandOut> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    Ok(CommandOut {
        ok: output.status.success(),
        code: output.status.code(),
        out: String::from_utf8_lossy(&output.stdout).into_owned(),
        err: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn git_with_index(repo: &Path, index: &Path, args: &[&str]) -> Result<CommandOut> {
    let output = Command::new("git")
        .args(args)
        .env("GIT_INDEX_FILE", index)
        .current_dir(repo)
        .output()
        .with_context(|| format!("running temporary-index git {}", args.join(" ")))?;
    Ok(CommandOut {
        ok: output.status.success(),
        code: output.status.code(),
        out: String::from_utf8_lossy(&output.stdout).into_owned(),
        err: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn command(program: &str, args: &[&str]) -> Result<CommandOut> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("running {program} {}", args.join(" ")))?;
    Ok(CommandOut {
        ok: output.status.success(),
        code: output.status.code(),
        out: String::from_utf8_lossy(&output.stdout).into_owned(),
        err: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

struct Logger {
    path: PathBuf,
}

impl Logger {
    fn log(&self, message: &str) {
        let line = format!(
            "[{}] {message}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
        println!("{line}");
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

struct Heartbeat {
    path: PathBuf,
    role: Role,
    tick: u64,
    started_at: String,
    actions: u64,
    last_action: Option<String>,
    last_error: Option<String>,
    last_state: String,
}

impl Heartbeat {
    fn new(state_dir: &Path, role: Role, tick: u64) -> Self {
        std::fs::create_dir_all(state_dir).ok();
        Self {
            path: state_dir.join(format!("assistants-git-sync-{}.json", role.as_str())),
            role,
            tick,
            started_at: now(),
            actions: 0,
            last_action: None,
            last_error: None,
            last_state: "starting".into(),
        }
    }

    fn record(&mut self, outcome: &Result<Outcome>) {
        self.last_state = match outcome {
            Ok(Outcome::Clean) => "clean".into(),
            Ok(Outcome::Waiting(message)) => format!("waiting: {message}"),
            Ok(Outcome::Blocked(message)) => {
                self.last_error = Some(message.clone());
                format!("blocked: {message}")
            }
            Ok(Outcome::DryRun(message)) => format!("dry-run: {message}"),
            Ok(Outcome::Adopted { from, to }) => {
                self.actions += 1;
                self.last_action = Some(now());
                self.last_error = None;
                format!("adopted {from} -> {to}")
            }
            Ok(Outcome::Pushed(subject)) => {
                self.actions += 1;
                self.last_action = Some(now());
                self.last_error = None;
                format!("pushed: {subject}")
            }
            Ok(Outcome::PushFailed(message)) => {
                self.last_error = Some(message.clone());
                format!("push failed: {message}")
            }
            Err(error) => {
                self.last_error = Some(format!("{error:#}"));
                format!("error: {error:#}")
            }
        };
        if matches!(
            outcome,
            Ok(Outcome::Clean) | Ok(Outcome::Waiting(_)) | Ok(Outcome::DryRun(_))
        ) {
            self.last_error = None;
        }

        let document = json!({
            "watcher": "assistants-git-sync",
            "version": env!("CARGO_PKG_VERSION"),
            "role": self.role.as_str(),
            "host": host_label(),
            "started_at": self.started_at,
            "last_cycle": now(),
            "last_action": self.last_action,
            "actions": self.actions,
            "last_error": self.last_error,
            "last_state": self.last_state,
            "interval_secs": self.tick,
        });
        let temporary = self.path.with_extension("json.tmp");
        if serde_json::to_vec_pretty(&document)
            .ok()
            .and_then(|bytes| std::fs::write(&temporary, bytes).ok())
            .is_some()
        {
            let _ = std::fs::rename(&temporary, &self.path);
        }
    }
}

fn now() -> String {
    chrono::Local::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use filetime::FileTime;

    struct Fixture {
        _root: tempfile::TempDir,
        remote: PathBuf,
        writer: PathBuf,
        follower: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let remote = root.path().join("remote.git");
            let writer = root.path().join("writer");
            let follower = root.path().join("follower");

            std::fs::create_dir_all(&remote).unwrap();
            sh(&remote, &["init", "--bare", "-q", "-b", "main"]);
            std::fs::create_dir_all(&writer).unwrap();
            sh(&writer, &["init", "-q", "-b", "main"]);
            configure_identity(&writer);
            sh(
                &writer,
                &["remote", "add", "origin", remote.to_str().unwrap()],
            );
            std::fs::write(writer.join(".gitignore"), "continuum-usage/\n").unwrap();
            std::fs::write(writer.join("doc.md"), "seed\n").unwrap();
            sh(&writer, &["add", "-A"]);
            sh(&writer, &["commit", "-q", "-m", "seed"]);
            sh(&writer, &["push", "-q", "origin", "main"]);

            let output = Command::new("git")
                .args([
                    "clone",
                    "-q",
                    "--branch",
                    "main",
                    remote.to_str().unwrap(),
                    follower.to_str().unwrap(),
                ])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            configure_identity(&follower);

            Self {
                _root: root,
                remote,
                writer,
                follower,
            }
        }

        fn push_writer(&self, value: &str) {
            std::fs::write(self.writer.join("doc.md"), value).unwrap();
            sh(&self.writer, &["commit", "-q", "-am", "writer change"]);
            sh(&self.writer, &["push", "-q", "origin", "main"]);
        }
    }

    fn configure_identity(repo: &Path) {
        sh(repo, &["config", "user.email", "test@example.invalid"]);
        sh(repo, &["config", "user.name", "Test"]);
    }

    fn sh(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn logger(root: &Path) -> Logger {
        Logger {
            path: root.join("test.log"),
        }
    }

    fn ready() -> Result<SyncReadiness> {
        Ok(SyncReadiness {
            ready: true,
            detail: "fixture complete".into(),
        })
    }

    #[test]
    fn host_names_normalise_without_local_suffix() {
        assert_eq!(
            normalise_host("Williams-MacBook-Air.local"),
            "williams-macbook-air"
        );
        assert_eq!(normalise_host("NIMBINI"), "nimbini");
    }

    #[test]
    fn blocked_outcome_becomes_a_durable_role_heartbeat_error() {
        let directory = tempfile::tempdir().unwrap();
        let mut heartbeat = Heartbeat::new(directory.path(), Role::Follower, 30);
        heartbeat.record(&Ok(Outcome::Blocked("tree mismatch".into())));
        let value: Value = serde_json::from_slice(
            &std::fs::read(directory.path().join("assistants-git-sync-follower.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(value["role"], "follower");
        assert_eq!(value["last_error"], "tree mismatch");
        assert_eq!(value["last_state"], "blocked: tree mismatch");
    }

    #[test]
    fn follower_adopts_exact_syncthing_tree_without_touching_file() {
        let fixture = Fixture::new();
        fixture.push_writer("remote\n");
        std::fs::write(fixture.follower.join("doc.md"), "remote\n").unwrap();
        let before = FileTime::from_last_modification_time(
            &std::fs::metadata(fixture.follower.join("doc.md")).unwrap(),
        );

        let result = cycle(
            &fixture.follower,
            Role::Follower,
            Duration::ZERO,
            false,
            &logger(fixture._root.path()),
            &ready,
            &mut Quiescence::default(),
        )
        .unwrap();
        assert!(matches!(result, Outcome::Adopted { .. }), "{result:?}");
        let after = FileTime::from_last_modification_time(
            &std::fs::metadata(fixture.follower.join("doc.md")).unwrap(),
        );
        assert_eq!(before, after);
        assert_eq!(
            rev_parse(&fixture.follower, "HEAD").unwrap(),
            rev_parse(&fixture.follower, "origin/main").unwrap()
        );
        assert!(changed_paths(&fixture.follower).unwrap().is_empty());
    }

    #[test]
    fn follower_refuses_remote_adoption_when_bytes_differ() {
        let fixture = Fixture::new();
        let old = rev_parse(&fixture.follower, "HEAD").unwrap();
        fixture.push_writer("remote\n");
        std::fs::write(fixture.follower.join("doc.md"), "different\n").unwrap();

        let result = cycle(
            &fixture.follower,
            Role::Follower,
            Duration::ZERO,
            false,
            &logger(fixture._root.path()),
            &ready,
            &mut Quiescence::default(),
        )
        .unwrap();
        assert!(matches!(result, Outcome::Blocked(_)), "{result:?}");
        assert_eq!(rev_parse(&fixture.follower, "HEAD").unwrap(), old);
        assert_eq!(
            std::fs::read_to_string(fixture.follower.join("doc.md")).unwrap(),
            "different\n"
        );
    }

    #[test]
    fn follower_refuses_diverged_history() {
        let fixture = Fixture::new();
        std::fs::write(fixture.follower.join("doc.md"), "local\n").unwrap();
        sh(&fixture.follower, &["commit", "-q", "-am", "local"]);
        fixture.push_writer("remote\n");

        let result = cycle(
            &fixture.follower,
            Role::Follower,
            Duration::ZERO,
            false,
            &logger(fixture._root.path()),
            &ready,
            &mut Quiescence::default(),
        )
        .unwrap();
        assert!(matches!(result, Outcome::Blocked(_)), "{result:?}");
    }

    #[test]
    fn follower_never_commits_local_changes() {
        let fixture = Fixture::new();
        let old = rev_parse(&fixture.follower, "HEAD").unwrap();
        std::fs::write(fixture.follower.join("doc.md"), "mac edit\n").unwrap();

        let result = cycle(
            &fixture.follower,
            Role::Follower,
            Duration::ZERO,
            false,
            &logger(fixture._root.path()),
            &ready,
            &mut Quiescence::default(),
        )
        .unwrap();
        assert!(matches!(result, Outcome::Waiting(_)), "{result:?}");
        assert_eq!(rev_parse(&fixture.follower, "HEAD").unwrap(), old);
        assert_eq!(rev_parse(&fixture.remote, "main").unwrap(), old);
    }

    #[test]
    fn historian_commits_and_pushes_after_both_gates() {
        let fixture = Fixture::new();
        std::fs::write(fixture.follower.join("doc.md"), "historian edit\n").unwrap();

        let result = cycle(
            &fixture.follower,
            Role::Historian,
            Duration::ZERO,
            false,
            &logger(fixture._root.path()),
            &ready,
            &mut Quiescence::default(),
        )
        .unwrap();
        assert!(matches!(result, Outcome::Pushed(_)), "{result:?}");
        assert_eq!(
            rev_parse(&fixture.follower, "HEAD").unwrap(),
            rev_parse(&fixture.remote, "main").unwrap()
        );
        let message = git(&fixture.follower, &["log", "-1", "--format=%B"])
            .unwrap()
            .out;
        assert!(message.contains("nimbini historian"), "{message}");
        assert!(message.contains("Co-Authored-By: Claude"), "{message}");
    }

    #[test]
    fn historian_does_nothing_until_syncthing_gate_is_ready() {
        let fixture = Fixture::new();
        let old = rev_parse(&fixture.follower, "HEAD").unwrap();
        std::fs::write(fixture.follower.join("doc.md"), "not settled\n").unwrap();
        let not_ready = || {
            Ok(SyncReadiness {
                ready: false,
                detail: "one item pending".into(),
            })
        };

        let result = cycle(
            &fixture.follower,
            Role::Historian,
            Duration::ZERO,
            false,
            &logger(fixture._root.path()),
            &not_ready,
            &mut Quiescence::default(),
        )
        .unwrap();
        assert!(matches!(result, Outcome::Waiting(_)), "{result:?}");
        assert_eq!(rev_parse(&fixture.follower, "HEAD").unwrap(), old);
        assert_eq!(rev_parse(&fixture.remote, "main").unwrap(), old);
    }

    #[test]
    fn deletion_cannot_bypass_the_quiet_window() {
        let fixture = Fixture::new();
        let old = rev_parse(&fixture.follower, "HEAD").unwrap();
        std::fs::remove_file(fixture.follower.join("doc.md")).unwrap();
        let mut quiescence = Quiescence::default();

        let result = cycle(
            &fixture.follower,
            Role::Historian,
            Duration::from_secs(90),
            false,
            &logger(fixture._root.path()),
            &ready,
            &mut quiescence,
        )
        .unwrap();
        assert!(matches!(result, Outcome::Waiting(_)), "{result:?}");
        assert_eq!(rev_parse(&fixture.follower, "HEAD").unwrap(), old);
        assert_eq!(rev_parse(&fixture.remote, "main").unwrap(), old);
    }

    #[test]
    fn historian_restarts_quiet_window_if_tree_changes_during_gate() {
        let fixture = Fixture::new();
        let old = rev_parse(&fixture.follower, "HEAD").unwrap();
        std::fs::write(fixture.follower.join("doc.md"), "first state\n").unwrap();
        let during_gate = || {
            std::fs::write(fixture.follower.join("doc.md"), "second state\n").unwrap();
            ready()
        };

        let result = cycle(
            &fixture.follower,
            Role::Historian,
            Duration::ZERO,
            false,
            &logger(fixture._root.path()),
            &during_gate,
            &mut Quiescence::default(),
        )
        .unwrap();
        assert!(matches!(result, Outcome::Waiting(_)), "{result:?}");
        assert_eq!(rev_parse(&fixture.follower, "HEAD").unwrap(), old);
        assert_eq!(rev_parse(&fixture.remote, "main").unwrap(), old);
    }

    #[test]
    fn ignored_usage_state_does_not_prevent_exact_adoption() {
        let fixture = Fixture::new();
        fixture.push_writer("remote\n");
        std::fs::write(fixture.follower.join("doc.md"), "remote\n").unwrap();
        std::fs::create_dir_all(fixture.follower.join("continuum-usage")).unwrap();
        std::fs::write(
            fixture.follower.join("continuum-usage/local-state.json"),
            "different on every host\n",
        )
        .unwrap();
        let result = cycle(
            &fixture.follower,
            Role::Follower,
            Duration::ZERO,
            false,
            &logger(fixture._root.path()),
            &ready,
            &mut Quiescence::default(),
        )
        .unwrap();
        assert!(matches!(result, Outcome::Adopted { .. }), "{result:?}");
        assert!(fixture
            .follower
            .join("continuum-usage/local-state.json")
            .is_file());
        assert!(changed_paths(&fixture.follower).unwrap().is_empty());
    }

    #[test]
    fn conflict_copy_blocks_both_roles() {
        let fixture = Fixture::new();
        std::fs::write(
            fixture
                .follower
                .join("doc.sync-conflict-20260903-010101-AAAAAAA.md"),
            "conflict\n",
        )
        .unwrap();
        let conflicts = sync_conflicts(&fixture.follower).unwrap();
        assert_eq!(conflicts.len(), 1);
        let result = cycle(
            &fixture.follower,
            Role::Follower,
            Duration::ZERO,
            false,
            &logger(fixture._root.path()),
            &ready,
            &mut Quiescence::default(),
        )
        .unwrap();
        assert!(matches!(result, Outcome::Blocked(_)), "{result:?}");
    }

    #[test]
    fn query_encoding_is_deterministic() {
        assert_eq!(query_component("Assistants"), "Assistants");
        assert_eq!(query_component("a b/c"), "a%20b%2Fc");
    }
}
