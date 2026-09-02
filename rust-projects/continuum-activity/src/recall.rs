use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use clap::Subcommand;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
#[cfg(test)]
use std::time::SystemTime;
use std::time::{Instant, UNIX_EPOCH};
use walkdir::WalkDir;

const SCHEMA_VERSION: u32 = 1;
const MAX_INDEX_AGE_HOURS: i64 = 24;
const MAX_PRIMARY_HITS: usize = 3;
const MAX_OUTPUT_BYTES: usize = 800;
const PRIMARY_SCORE_FLOOR: f64 = 3.0;

#[derive(Debug, Subcommand)]
pub enum RecallCommand {
    /// Build or incrementally refresh the disposable local index
    Build {
        /// Override the home directory used to discover source trees
        #[arg(long)]
        home_root: Option<PathBuf>,
        /// Override the cache file (primarily for tests)
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Emit machine-readable build statistics
        #[arg(long)]
        json: bool,
        /// Re-extract every eligible source instead of reusing unchanged records
        #[arg(long)]
        force: bool,
    },
    /// Query the index with a task prompt; a miss writes nothing to stdout
    Query {
        /// The current task prompt
        task: String,
        /// Suppress operational diagnostics as required by prompt hooks
        #[arg(long)]
        probe: bool,
        /// Emit compact machine-readable output
        #[arg(long)]
        json: bool,
        /// Override the cache file (primarily for tests)
        #[arg(long)]
        cache: Option<PathBuf>,
    },
    /// Report whether the local index is fresh, stale, absent, or corrupt
    Status {
        /// Override the cache file (primarily for tests)
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Emit machine-readable status
        #[arg(long)]
        json: bool,
    },
    /// Run the evidence gate against built-in or user-supplied fixtures
    Backtest {
        /// JSON fixture file. Omit to run the built-in PHI-free suite.
        #[arg(long)]
        fixtures: Option<PathBuf>,
        /// Emit machine-readable results
        #[arg(long)]
        json: bool,
    },
    /// Measure the full local probe path, including cache load and source checks
    Benchmark {
        /// A representative technical task prompt
        task: String,
        /// Number of probes to measure
        #[arg(long, default_value_t = 200)]
        iterations: usize,
        /// Override the cache file (primarily for tests)
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Emit machine-readable timings
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum SourceKind {
    Continuum,
    DevLog,
    Forum,
    Fixture,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum EntityKind {
    Path,
    File,
    Command,
    Service,
    Project,
    Error,
    Host,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct EntityEvidence {
    kind: EntityKind,
    count: u32,
    first_line: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct IndexDocument {
    source: String,
    source_kind: SourceKind,
    line: usize,
    date: String,
    content_hash: String,
    modified_ns: u128,
    size: u64,
    entities: BTreeMap<String, EntityEvidence>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RecallIndex {
    schema_version: u32,
    built_at: String,
    corpus_fingerprint: String,
    documents: Vec<IndexDocument>,
}

#[derive(Debug, Default, Serialize)]
struct BuildStats {
    examined_sources: usize,
    eligible_sources: usize,
    updated_sources: usize,
    reused_sources: usize,
    indexed_documents: usize,
    skipped_policy: usize,
    skipped_sensitive: usize,
    skipped_no_entities: usize,
    failed_sources: usize,
    cache: String,
    corpus_fingerprint: String,
}

#[derive(Clone, Debug)]
struct SourceSpec {
    path: PathBuf,
    kind: SourceKind,
    date: String,
}

#[derive(Clone, Debug, Serialize)]
struct RecallHit {
    relation: &'static str,
    source: String,
    line: usize,
    date: String,
    matched: Vec<String>,
    score: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QueryState {
    Hit,
    NoHit,
    NoTechnicalCue,
    SensitivePrompt,
    StaleIndex,
}

#[derive(Debug)]
struct QueryOutcome {
    state: QueryState,
    hits: Vec<RecallHit>,
}

#[derive(Debug)]
struct RankedCandidate {
    document_index: usize,
    rank_score: f64,
    matched: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SessionMeta {
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    start_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FixtureFile {
    documents: Vec<FixtureDocument>,
    cases: Vec<FixtureCase>,
}

#[derive(Debug, Deserialize)]
struct FixtureDocument {
    source: String,
    date: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct FixtureCase {
    name: String,
    query: String,
    #[serde(default)]
    expected_sources: Vec<String>,
    #[serde(default)]
    expect_silent: bool,
    #[serde(default)]
    baseline_must_miss: bool,
}

#[derive(Debug, Serialize)]
struct CaseResult {
    name: String,
    passed: bool,
    detail: String,
}

#[derive(Debug, Serialize)]
struct BacktestReport {
    passed: usize,
    total: usize,
    red_control_detected: bool,
    p95_microseconds: u128,
    cases: Vec<CaseResult>,
}

pub fn run(command: RecallCommand) -> Result<()> {
    match command {
        RecallCommand::Build {
            home_root,
            cache,
            json,
            force,
        } => {
            let home = match home_root {
                Some(path) => path,
                None => dirs::home_dir().context("No home directory")?,
            };
            let cache = cache.unwrap_or(default_cache_path()?);
            let stats = build_index(&home, &cache, !force)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&stats)?);
            } else {
                println!(
                    "Indexed {} document(s) from {} eligible source(s): {} updated, {} reused; {} policy skips, {} sensitive skips, {} failures.\nCache: {}\nFingerprint: {}",
                    stats.indexed_documents,
                    stats.eligible_sources,
                    stats.updated_sources,
                    stats.reused_sources,
                    stats.skipped_policy,
                    stats.skipped_sensitive,
                    stats.failed_sources,
                    stats.cache,
                    stats.corpus_fingerprint,
                );
            }
            Ok(())
        }
        RecallCommand::Query {
            task,
            probe,
            json,
            cache,
        } => {
            let cache = cache.unwrap_or(default_cache_path()?);
            let index = match load_index(&cache) {
                Ok(index) => index,
                Err(error) => {
                    if !probe {
                        eprintln!(
                            "task recall unavailable ({error}); run `continuum-activity recall build`"
                        );
                    }
                    return Ok(());
                }
            };
            let outcome = query_index(&index, &task, true, true)?;
            if outcome.hits.is_empty() {
                if !probe {
                    match outcome.state {
                        QueryState::StaleIndex => eprintln!(
                            "task recall index is older than {MAX_INDEX_AGE_HOURS} hours; run `continuum-activity recall build`"
                        ),
                        QueryState::SensitivePrompt => {
                            eprintln!("task recall declined a sensitive prompt")
                        }
                        QueryState::NoTechnicalCue => {
                            eprintln!("task recall found no structural technical cue")
                        }
                        QueryState::NoHit | QueryState::Hit => {}
                    }
                }
                return Ok(());
            }
            let rendered = if json {
                render_json(&outcome.hits)?
            } else {
                render_text(&outcome.hits)
            };
            debug_assert!(rendered.len() <= MAX_OUTPUT_BYTES);
            print!("{rendered}");
            Ok(())
        }
        RecallCommand::Status { cache, json } => {
            let cache = cache.unwrap_or(default_cache_path()?);
            let (state, documents, built_at, fingerprint) = match load_index(&cache) {
                Ok(index) => {
                    let state = if index_is_fresh(&index) {
                        "fresh"
                    } else {
                        "stale"
                    };
                    (
                        state,
                        index.documents.len(),
                        index.built_at,
                        index.corpus_fingerprint,
                    )
                }
                Err(_error) if !cache.exists() => ("absent", 0, String::new(), String::new()),
                Err(error) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::json!({
                                "state": "corrupt",
                                "cache": cache,
                                "error": error.to_string(),
                            })
                        );
                    } else {
                        println!("corrupt — {} ({})", cache.display(), error);
                    }
                    return Ok(());
                }
            };
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "state": state,
                        "cache": cache,
                        "documents": documents,
                        "built_at": built_at,
                        "corpus_fingerprint": fingerprint,
                    })
                );
            } else {
                println!(
                    "{} — {} document(s) — {}{}",
                    state,
                    documents,
                    cache.display(),
                    if built_at.is_empty() {
                        String::new()
                    } else {
                        format!(" — built {built_at}")
                    }
                );
            }
            Ok(())
        }
        RecallCommand::Backtest { fixtures, json } => {
            let report = match fixtures {
                Some(path) => run_fixture_file(&path)?,
                None => run_builtin_backtest()?,
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "Backtest: {}/{} passed; red control {}; query p95 {} µs",
                    report.passed,
                    report.total,
                    if report.red_control_detected {
                        "detected"
                    } else {
                        "FAILED"
                    },
                    report.p95_microseconds,
                );
                for case in &report.cases {
                    println!(
                        "{} {} — {}",
                        if case.passed { "PASS" } else { "FAIL" },
                        case.name,
                        case.detail
                    );
                }
            }
            if report.passed != report.total
                || !report.red_control_detected
                || report.p95_microseconds > 50_000
            {
                bail!("task recall evidence gate failed");
            }
            Ok(())
        }
        RecallCommand::Benchmark {
            task,
            iterations,
            cache,
            json,
        } => {
            if iterations == 0 {
                bail!("--iterations must be greater than zero");
            }
            let cache = cache.unwrap_or(default_cache_path()?);
            let mut timings = Vec::with_capacity(iterations);
            let mut last_hits = 0;
            for _ in 0..iterations {
                let started = Instant::now();
                let index = load_index(&cache)?;
                let outcome = query_index(&index, &task, true, true)?;
                last_hits = outcome.hits.len();
                timings.push(started.elapsed().as_micros());
            }
            timings.sort_unstable();
            let percentile = |fraction: f64| {
                let index = ((timings.len() as f64 * fraction).ceil() as usize)
                    .saturating_sub(1)
                    .min(timings.len() - 1);
                timings[index]
            };
            let p50 = percentile(0.50);
            let p95 = percentile(0.95);
            let max = *timings.last().expect("non-empty timings");
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "iterations": iterations,
                        "hits": last_hits,
                        "p50_microseconds": p50,
                        "p95_microseconds": p95,
                        "max_microseconds": max,
                        "budget_microseconds": 50_000,
                        "within_budget": p95 <= 50_000,
                    })
                );
            } else {
                println!(
                    "{} probes, {} hit(s): p50 {} µs, p95 {} µs, max {} µs — {} 50 ms budget",
                    iterations,
                    last_hits,
                    p50,
                    p95,
                    max,
                    if p95 <= 50_000 { "within" } else { "OVER" },
                );
            }
            if p95 > 50_000 {
                bail!("task recall probe exceeds the 50 ms p95 budget");
            }
            Ok(())
        }
    }
}

fn default_cache_path() -> Result<PathBuf> {
    Ok(dirs::cache_dir()
        .context("No platform cache directory")?
        .join("continuum/task-recall-v1.json"))
}

fn build_index(home: &Path, cache: &Path, reuse_unchanged: bool) -> Result<BuildStats> {
    let mut stats = BuildStats {
        cache: cache.display().to_string(),
        ..BuildStats::default()
    };
    let sources = discover_sources(home, &mut stats)?;
    if stats.examined_sources == 0 {
        bail!(
            "no candidate sources were examined under {}",
            home.display()
        );
    }
    if sources.is_empty() {
        bail!(
            "examined {} source(s), but none passed the source policy",
            stats.examined_sources
        );
    }

    let old_index = reuse_unchanged.then(|| load_index(cache).ok()).flatten();
    let mut old_by_source: HashMap<String, Vec<IndexDocument>> = HashMap::new();
    if let Some(old) = old_index {
        for document in old.documents {
            old_by_source
                .entry(document.source.clone())
                .or_default()
                .push(document);
        }
    }

    let mut documents = Vec::new();
    let mut fingerprints = Vec::new();
    for source in sources {
        let pointer = source_pointer(&source.path, home);
        let metadata = match fs::metadata(&source.path) {
            Ok(metadata) => metadata,
            Err(error) => {
                stats.failed_sources += 1;
                eprintln!("warning: cannot stat {}: {}", source.path.display(), error);
                continue;
            }
        };
        let modified_ns = modified_ns(&metadata);
        let size = metadata.len();
        if let Some(existing) = old_by_source.remove(&pointer) {
            if existing
                .first()
                .is_some_and(|doc| doc.modified_ns == modified_ns && doc.size == size)
            {
                if let Some(first) = existing.first() {
                    fingerprints.push((pointer.clone(), first.content_hash.clone()));
                }
                documents.extend(existing);
                stats.reused_sources += 1;
                continue;
            }
        }

        match index_source(&source, home, modified_ns, size) {
            Ok(SourceIndexResult::Indexed(mut indexed)) => {
                if let Some(first) = indexed.first() {
                    fingerprints.push((pointer, first.content_hash.clone()));
                }
                stats.updated_sources += 1;
                documents.append(&mut indexed);
            }
            Ok(SourceIndexResult::Sensitive) => stats.skipped_sensitive += 1,
            Ok(SourceIndexResult::NoEntities) => stats.skipped_no_entities += 1,
            Err(error) => {
                stats.failed_sources += 1;
                eprintln!(
                    "warning: cannot index {}: {:#}",
                    source.path.display(),
                    error
                );
            }
        }
    }

    if documents.is_empty() {
        bail!("eligible sources produced zero index documents");
    }
    documents.sort_by(|a, b| a.source.cmp(&b.source).then_with(|| a.line.cmp(&b.line)));
    fingerprints.sort();
    let mut corpus_hasher = Sha256::new();
    for (source, hash) in &fingerprints {
        corpus_hasher.update(source.as_bytes());
        corpus_hasher.update([0]);
        corpus_hasher.update(hash.as_bytes());
        corpus_hasher.update([0]);
    }
    let corpus_fingerprint = format!("{:x}", corpus_hasher.finalize());
    let index = RecallIndex {
        schema_version: SCHEMA_VERSION,
        built_at: Utc::now().to_rfc3339(),
        corpus_fingerprint: corpus_fingerprint.clone(),
        documents,
    };
    write_index_atomic(cache, &index)?;
    stats.indexed_documents = index.documents.len();
    stats.corpus_fingerprint = corpus_fingerprint;
    Ok(stats)
}

fn discover_sources(home: &Path, stats: &mut BuildStats) -> Result<Vec<SourceSpec>> {
    let mut sources = Vec::new();
    let forum_root = home.join("Assistants/shared/design-forum");
    if forum_root.exists() {
        for entry in WalkDir::new(&forum_root).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            stats.examined_sources += 1;
            let filename = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            if !dated_markdown_name(filename)
                || path.components().any(|component| {
                    component
                        .as_os_str()
                        .to_str()
                        .is_some_and(|part| part.starts_with('.'))
                })
            {
                stats.skipped_policy += 1;
                continue;
            }
            stats.eligible_sources += 1;
            sources.push(SourceSpec {
                path: path.to_path_buf(),
                kind: SourceKind::Forum,
                date: date_from_path(path).unwrap_or_else(|| modified_date(path)),
            });
        }
    }

    let devlog_root = home.join("Forge/NapierianLogs/DevLog");
    if devlog_root.exists() {
        for entry in WalkDir::new(&devlog_root)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            stats.examined_sources += 1;
            stats.eligible_sources += 1;
            sources.push(SourceSpec {
                path: path.to_path_buf(),
                kind: SourceKind::DevLog,
                date: date_from_path(path).unwrap_or_else(|| modified_date(path)),
            });
        }
    }

    let continuum_root = home.join("Assistants/continuum-logs");
    if continuum_root.exists() {
        for entry in WalkDir::new(&continuum_root)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if !path.is_file() || path.file_name().and_then(|s| s.to_str()) != Some("session.json")
            {
                continue;
            }
            stats.examined_sources += 1;
            let metadata = match fs::read_to_string(path)
                .ok()
                .and_then(|text| serde_json::from_str::<SessionMeta>(&text).ok())
            {
                Some(metadata) => metadata,
                None => {
                    stats.failed_sources += 1;
                    continue;
                }
            };
            let mut skills = metadata.skills;
            skills.sort();
            skills.dedup();
            if skills.as_slice() != ["senior-dev"] {
                stats.skipped_policy += 1;
                continue;
            }
            let messages = path.with_file_name("messages.jsonl");
            if !messages.exists() {
                stats.failed_sources += 1;
                continue;
            }
            stats.eligible_sources += 1;
            sources.push(SourceSpec {
                path: messages,
                kind: SourceKind::Continuum,
                date: metadata
                    .start_time
                    .as_deref()
                    .and_then(|date| date.get(..10))
                    .filter(|date| is_iso_date(date))
                    .map(str::to_string)
                    .or_else(|| date_from_path(path))
                    .unwrap_or_else(|| modified_date(path)),
            });
        }
    }
    Ok(sources)
}

fn dated_markdown_name(filename: &str) -> bool {
    filename.len() > 14 && filename.ends_with(".md") && filename.get(..10).is_some_and(is_iso_date)
}

fn is_iso_date(value: &str) -> bool {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    let regex = REGEX.get_or_init(|| Regex::new(r"^\d{4}-\d{2}-\d{2}$").expect("valid date regex"));
    regex.is_match(value) && NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
}

fn date_from_path(path: &Path) -> Option<String> {
    for component in path.components() {
        let value = component.as_os_str().to_string_lossy();
        for start in 0..value.len().saturating_sub(9) {
            let Some(candidate) = value.get(start..start + 10) else {
                continue;
            };
            if is_iso_date(candidate) {
                return Some(candidate.to_string());
            }
        }
    }
    None
}

fn modified_date(path: &Path) -> String {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .map(DateTime::<Utc>::from)
        .map(|date| date.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn modified_ns(metadata: &fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

enum SourceIndexResult {
    Indexed(Vec<IndexDocument>),
    Sensitive,
    NoEntities,
}

fn index_source(
    source: &SourceSpec,
    home: &Path,
    expected_modified_ns: u128,
    size: u64,
) -> Result<SourceIndexResult> {
    let bytes =
        fs::read(&source.path).with_context(|| format!("cannot read {}", source.path.display()))?;
    let after_read = fs::metadata(&source.path)
        .with_context(|| format!("cannot restat {}", source.path.display()))?;
    if after_read.len() != size || modified_ns(&after_read) != expected_modified_ns {
        bail!("source changed while it was being indexed");
    }
    let content_hash = sha256(&bytes);
    let raw_text = String::from_utf8_lossy(&bytes);
    let text = if source.kind == SourceKind::Continuum {
        continuum_message_text(&raw_text)
    } else {
        raw_text.into_owned()
    };
    if is_sensitive_text(&text) {
        return Ok(SourceIndexResult::Sensitive);
    }

    let chunks = if source.kind == SourceKind::Continuum {
        vec![(1, text)]
    } else {
        markdown_chunks(&text)
    };
    let pointer = source_pointer(&source.path, home);
    let mut documents = Vec::new();
    for (line, chunk) in chunks {
        let entities = extract_entities(&chunk);
        if entities.is_empty() {
            continue;
        }
        let date = chunk
            .lines()
            .next()
            .and_then(date_from_text)
            .unwrap_or_else(|| source.date.clone());
        documents.push(IndexDocument {
            source: pointer.clone(),
            source_kind: source.kind,
            line,
            date,
            content_hash: content_hash.clone(),
            modified_ns: expected_modified_ns,
            size,
            entities,
        });
    }
    if documents.is_empty() {
        Ok(SourceIndexResult::NoEntities)
    } else {
        Ok(SourceIndexResult::Indexed(documents))
    }
}

fn continuum_message_text(raw: &str) -> String {
    let mut output = String::new();
    for line in raw.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let role = value.get("role").and_then(|value| value.as_str());
        if !matches!(role, Some("user" | "assistant")) {
            continue;
        }
        if let Some(content) = value.get("content").and_then(|value| value.as_str()) {
            output.push_str(content);
            output.push('\n');
        }
    }
    output
}

fn markdown_chunks(text: &str) -> Vec<(usize, String)> {
    let mut chunks = Vec::new();
    let mut start_line = 1;
    let mut current = String::new();
    for (offset, line) in text.lines().enumerate() {
        let line_number = offset + 1;
        if line.starts_with("## ") && !current.trim().is_empty() {
            chunks.push((start_line, std::mem::take(&mut current)));
            start_line = line_number;
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        chunks.push((start_line, current));
    }
    chunks
}

fn date_from_text(text: &str) -> Option<String> {
    for start in 0..text.len().saturating_sub(9) {
        let Some(candidate) = text.get(start..start + 10) else {
            continue;
        };
        if is_iso_date(candidate) {
            return Some(candidate.to_string());
        }
    }
    None
}

fn sensitive_markers() -> &'static [&'static str] {
    &[
        "/clinical",
        "\\clinical",
        "clinical client",
        "patient name",
        "nhs number",
        "date of birth",
        "client::",
        "#client/",
        "tm3 client",
    ]
}

fn is_sensitive_text(text: &str) -> bool {
    let lower = text.to_lowercase();
    sensitive_markers()
        .iter()
        .any(|marker| lower.contains(marker))
}

fn path_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?:~|/|\./|\.\./)[A-Za-z0-9_@.+~/-]+(?:\.[A-Za-z0-9_-]+)?")
            .expect("valid path regex")
    })
}

fn valid_path_candidate(line: &str, start: usize, candidate: &str) -> bool {
    let has_valid_boundary = start == 0
        || line[..start].chars().next_back().is_some_and(|character| {
            character.is_whitespace() || matches!(character, '\'' | '"' | '`')
        });
    if !has_valid_boundary
        || !candidate
            .chars()
            .any(|character| character.is_ascii_alphabetic())
        || candidate
            .strip_prefix('~')
            .and_then(|suffix| suffix.chars().next())
            .is_some_and(|character| character.is_ascii_digit())
    {
        return false;
    }

    let segments: Vec<&str> = candidate
        .split('/')
        .filter(|segment| {
            !segment.is_empty() && *segment != "." && *segment != ".." && *segment != "~"
        })
        .collect();
    segments.len() >= 2 || segments.first().is_some_and(|segment| segment.len() >= 3)
}

fn file_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"\b[A-Za-z0-9_][A-Za-z0-9_.-]{1,80}\.(?:rs|toml|md|jsonl?|nu|sh|service|timer|socket|path|target|plist|kdl|nix|ya?ml|lock|uf2)\b")
            .expect("valid filename regex")
    })
}

fn code_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"`([^`\n]{1,160})`").expect("valid code regex"))
}

fn token_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"[A-Za-z][A-Za-z0-9_-]{1,79}").expect("valid token regex"))
}

fn tag_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"#(?:project|topic|pr|issue)/[A-Za-z0-9_.-]+").expect("valid tag regex")
    })
}

fn error_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"\bE[0-9]{4}\b").expect("valid error regex"))
}

fn known_commands() -> &'static [&'static str] {
    &[
        "ai-brief",
        "cargo",
        "codex",
        "continuum",
        "continuum-activity",
        "cross-machine-sync-check",
        "dev-catchup",
        "dotter",
        "dotter-drift-monitor",
        "dotter-orphan-detector-v2",
        "forum",
        "git",
        "git-push-reliability-monitor",
        "helix",
        "hx",
        "journalctl",
        "link-service",
        "mailcurator",
        "niri",
        "npm",
        "practiceforge",
        "rust-redeploy",
        "script-ready-deploy",
        "ssh",
        "sync-service-monitor",
        "systemctl",
        "tm3-diary-capture",
        "wiki-link-service",
        "zellij",
    ]
}

fn generic_commands() -> &'static [&'static str] {
    &["cargo", "git", "hx", "npm", "ssh", "systemctl"]
}

fn subcommand_stopwords() -> &'static [&'static str] {
    &[
        "a", "an", "and", "are", "as", "at", "but", "by", "for", "from", "has", "in", "is", "it",
        "not", "of", "on", "or", "that", "the", "this", "to", "was", "with",
    ]
}

fn extract_entities(text: &str) -> BTreeMap<String, EntityEvidence> {
    extract_entities_with_mode(text, false)
}

fn extract_query_entities(text: &str) -> BTreeMap<String, EntityEvidence> {
    extract_entities_with_mode(text, true)
}

fn extract_entities_with_mode(
    text: &str,
    include_plain_subcommands: bool,
) -> BTreeMap<String, EntityEvidence> {
    let mut entities = BTreeMap::new();
    for (offset, line) in text.lines().enumerate() {
        let line_number = offset + 1;
        for found in path_regex()
            .find_iter(line)
            .filter(|found| valid_path_candidate(line, found.start(), found.as_str()))
        {
            let path = normalize_path(found.as_str());
            if path.is_empty() || is_sensitive_entity(&path) {
                continue;
            }
            add_entity(&mut entities, path.clone(), EntityKind::Path, line_number);
            if let Some(name) = path.rsplit('/').next().filter(|name| !name.is_empty()) {
                if looks_like_technical_file(name) {
                    add_entity(
                        &mut entities,
                        name.to_lowercase(),
                        classify_file(name),
                        line_number,
                    );
                }
            }
        }
        for found in file_regex().find_iter(line) {
            add_entity(
                &mut entities,
                found.as_str().to_lowercase(),
                classify_file(found.as_str()),
                line_number,
            );
        }
        for found in tag_regex().find_iter(line) {
            add_entity(
                &mut entities,
                found.as_str().to_lowercase(),
                EntityKind::Project,
                line_number,
            );
        }
        for found in error_regex().find_iter(line) {
            add_entity(
                &mut entities,
                found.as_str().to_lowercase(),
                EntityKind::Error,
                line_number,
            );
        }
        for capture in code_regex().captures_iter(line) {
            if let Some(value) = capture.get(1) {
                extract_code_span(value.as_str(), line_number, &mut entities);
            }
        }

        let words: Vec<String> = token_regex()
            .find_iter(line)
            .map(|found| found.as_str().to_lowercase())
            .collect();
        for (index, word) in words.iter().enumerate() {
            if word == "nimbini" || word == "williams-macbook-air" {
                add_entity(&mut entities, word.clone(), EntityKind::Host, line_number);
            }
            if !known_commands().contains(&word.as_str()) {
                continue;
            }
            add_entity(
                &mut entities,
                word.clone(),
                EntityKind::Command,
                line_number,
            );
            if include_plain_subcommands {
                if let Some(next) = words.get(index + 1).filter(|next| valid_subcommand(next)) {
                    add_entity(
                        &mut entities,
                        format!("{word}:{next}"),
                        EntityKind::Command,
                        line_number,
                    );
                }
            }
        }
    }
    entities
}

fn extract_code_span(span: &str, line: usize, entities: &mut BTreeMap<String, EntityEvidence>) {
    let trimmed = span.trim().trim_start_matches('$').trim();
    if trimmed.is_empty() {
        return;
    }
    if path_regex().is_match(trimmed) {
        for found in path_regex()
            .find_iter(trimmed)
            .filter(|found| valid_path_candidate(trimmed, found.start(), found.as_str()))
        {
            let path = normalize_path(found.as_str());
            if !path.is_empty() && !is_sensitive_entity(&path) {
                add_entity(entities, path, EntityKind::Path, line);
            }
        }
    }
    let words: Vec<String> = token_regex()
        .find_iter(trimmed)
        .map(|found| found.as_str().to_lowercase())
        .collect();
    let Some(first) = words.first() else {
        return;
    };
    if known_commands().contains(&first.as_str()) {
        add_entity(entities, first.clone(), EntityKind::Command, line);
        if let Some(next) = words.get(1).filter(|next| valid_subcommand(next)) {
            add_entity(
                entities,
                format!("{first}:{next}"),
                EntityKind::Command,
                line,
            );
        }
    } else if words.len() == 1 && first.contains('-') && first.len() >= 4 {
        add_entity(entities, first.clone(), EntityKind::Command, line);
    }
}

fn valid_subcommand(value: &str) -> bool {
    value.len() >= 2
        && !subcommand_stopwords().contains(&value)
        && !value.chars().all(|character| character.is_ascii_digit())
}

fn normalize_path(value: &str) -> String {
    let trimmed = value.trim_matches(|character: char| {
        matches!(
            character,
            '`' | '\'' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | ':'
        )
    });
    if trimmed.starts_with("//") {
        return String::new();
    }
    let normalized = trimmed
        .replace("/Users/williamnapier", "~")
        .replace("/home/will", "~")
        .trim_end_matches('.')
        .to_lowercase();
    if matches!(normalized.as_str(), "~" | "~/" | "/" | "./" | "../") {
        String::new()
    } else {
        normalized
    }
}

fn looks_like_technical_file(value: &str) -> bool {
    file_regex().is_match(value)
}

fn classify_file(value: &str) -> EntityKind {
    if value.ends_with(".service")
        || value.ends_with(".timer")
        || value.ends_with(".socket")
        || value.ends_with(".target")
        || value.ends_with(".path")
        || value.starts_with("com.") && value.ends_with(".plist")
    {
        EntityKind::Service
    } else {
        EntityKind::File
    }
}

fn is_sensitive_entity(value: &str) -> bool {
    let lower = value.to_lowercase();
    sensitive_markers()
        .iter()
        .any(|marker| lower.contains(marker))
        || (lower.starts_with("sk-") && lower.len() > 12)
        || lower.starts_with("ghp_")
        || lower.starts_with("github_pat_")
        || lower.starts_with("xoxb-")
        || lower.starts_with("xoxp-")
        || lower.starts_with("akia") && lower.len() >= 16
}

fn kind_weight(kind: EntityKind, entity: &str) -> f64 {
    match kind {
        EntityKind::Path => 3.0,
        EntityKind::Service => 3.0,
        EntityKind::Error => 3.0,
        EntityKind::File => 2.5,
        EntityKind::Project => 2.3,
        EntityKind::Host => 1.8,
        EntityKind::Command if entity.contains(':') => 2.8,
        EntityKind::Command if generic_commands().contains(&entity) => 0.7,
        EntityKind::Command => 1.8,
    }
}

fn add_entity(
    entities: &mut BTreeMap<String, EntityEvidence>,
    entity: String,
    kind: EntityKind,
    line: usize,
) {
    if entity.len() < 2 || entity.len() > 180 || is_sensitive_entity(&entity) {
        return;
    }
    entities
        .entry(entity)
        .and_modify(|evidence| {
            evidence.count = evidence.count.saturating_add(1);
            evidence.first_line = evidence.first_line.min(line);
            if kind_weight(kind, "") > kind_weight(evidence.kind, "") {
                evidence.kind = kind;
            }
        })
        .or_insert(EntityEvidence {
            kind,
            count: 1,
            first_line: line,
        });
}

fn source_pointer(path: &Path, home: &Path) -> String {
    path.strip_prefix(home)
        .map(|relative| format!("~/{}", relative.display()))
        .unwrap_or_else(|_| path.display().to_string())
}

fn expand_pointer(pointer: &str) -> Option<PathBuf> {
    if let Some(relative) = pointer.strip_prefix("~/") {
        return dirs::home_dir().map(|home| home.join(relative));
    }
    if pointer.starts_with('/') {
        return Some(PathBuf::from(pointer));
    }
    None
}

fn sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn write_index_atomic(path: &Path, index: &RecallIndex) -> Result<()> {
    let parent = path.parent().context("cache path has no parent")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("cannot create cache directory {}", parent.display()))?;
    let temporary = parent.join(format!(
        ".{}.tmp.{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("task-recall"),
        std::process::id()
    ));
    let bytes = serde_json::to_vec(index)?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .with_context(|| format!("cannot create {}", temporary.display()))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path).with_context(|| format!("cannot replace {}", path.display()))?;
    Ok(())
}

fn load_index(path: &Path) -> Result<RecallIndex> {
    let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let index: RecallIndex = serde_json::from_slice(&bytes)
        .with_context(|| format!("cannot parse {}", path.display()))?;
    if index.schema_version != SCHEMA_VERSION {
        bail!(
            "unsupported schema {} (expected {})",
            index.schema_version,
            SCHEMA_VERSION
        );
    }
    Ok(index)
}

fn index_is_fresh(index: &RecallIndex) -> bool {
    DateTime::parse_from_rfc3339(&index.built_at)
        .map(|built| Utc::now().signed_duration_since(built.with_timezone(&Utc)))
        .is_ok_and(|age| age >= Duration::zero() && age <= Duration::hours(MAX_INDEX_AGE_HOURS))
}

fn is_non_task_prompt(task: &str) -> bool {
    let normalized = task
        .trim()
        .trim_matches(|character: char| character.is_ascii_punctuation())
        .to_lowercase();
    matches!(
        normalized.as_str(),
        "hi" | "hello"
            | "hey"
            | "how are you"
            | "how are you doing"
            | "how's it going"
            | "hows it going"
            | "senior-dev"
    ) || task.trim().eq_ignore_ascii_case("$senior-dev")
        || task.trim().eq_ignore_ascii_case("/senior-dev")
}

fn query_index(
    index: &RecallIndex,
    task: &str,
    verify_sources: bool,
    enforce_index_freshness: bool,
) -> Result<QueryOutcome> {
    if enforce_index_freshness && !index_is_fresh(index) {
        return Ok(QueryOutcome {
            state: QueryState::StaleIndex,
            hits: Vec::new(),
        });
    }
    if is_sensitive_text(task) {
        return Ok(QueryOutcome {
            state: QueryState::SensitivePrompt,
            hits: Vec::new(),
        });
    }
    if is_non_task_prompt(task) {
        return Ok(QueryOutcome {
            state: QueryState::NoTechnicalCue,
            hits: Vec::new(),
        });
    }
    let query_entities = extract_query_entities(task);
    if query_entities.is_empty() {
        return Ok(QueryOutcome {
            state: QueryState::NoTechnicalCue,
            hits: Vec::new(),
        });
    }

    let document_count = index.documents.len().max(1);
    let mut document_frequency: HashMap<&str, usize> = HashMap::new();
    for document in &index.documents {
        for entity in document.entities.keys() {
            *document_frequency.entry(entity.as_str()).or_default() += 1;
        }
    }

    let mut candidates = Vec::new();
    for (document_index, document) in index.documents.iter().enumerate() {
        let mut matched = Vec::new();
        let mut base_score = 0.0;
        let mut strong_count = 0;
        let mut frequency_bonus: f64 = 0.0;
        for (entity, query_evidence) in &query_entities {
            let Some(document_evidence) = document.entities.get(entity) else {
                continue;
            };
            let df = *document_frequency
                .get(entity.as_str())
                .unwrap_or(&document_count);
            let idf = ((document_count as f64 + 1.0) / (df as f64 + 1.0)).ln() + 1.0;
            let weight = kind_weight(query_evidence.kind, entity);
            base_score += idf * weight;
            frequency_bonus += (document_evidence.count as f64 + 1.0).ln() * 0.04;
            if weight >= 1.8 && !generic_commands().contains(&entity.as_str()) {
                strong_count += 1;
            }
            matched.push(entity.clone());
        }
        if matched.is_empty()
            || base_score < PRIMARY_SCORE_FLOOR
            || (matched.len() == 1 && strong_count == 0)
        {
            continue;
        }
        matched.sort_by(|a, b| {
            let a_kind = query_entities[a].kind;
            let b_kind = query_entities[b].kind;
            kind_weight(b_kind, b)
                .partial_cmp(&kind_weight(a_kind, a))
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.cmp(b))
        });
        let recency_bonus = recency_bonus(&document.date);
        candidates.push(RankedCandidate {
            document_index,
            rank_score: base_score + frequency_bonus.min(0.15) + recency_bonus,
            matched,
        });
    }
    candidates.sort_by(|a, b| {
        b.rank_score
            .partial_cmp(&a.rank_score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                index.documents[b.document_index]
                    .date
                    .cmp(&index.documents[a.document_index].date)
            })
    });

    let mut verified: HashMap<String, bool> = HashMap::new();
    let mut selected = Vec::new();
    let mut seen_locations = BTreeSet::new();
    for candidate in &candidates {
        if selected.len() == MAX_PRIMARY_HITS {
            break;
        }
        let document = &index.documents[candidate.document_index];
        let location = format!("{}:{}", document.source, document.line);
        if !seen_locations.insert(location) {
            continue;
        }
        if verify_sources && !verify_document(document, &mut verified) {
            continue;
        }
        selected.push(RecallHit {
            relation: "primary",
            source: document.source.clone(),
            line: document.line,
            date: document.date.clone(),
            matched: candidate.matched.iter().take(3).cloned().collect(),
            score: candidate.rank_score,
        });
    }

    let connection = selected.first().and_then(|selected_primary| {
        candidates
            .iter()
            .find(|candidate| {
                let document = &index.documents[candidate.document_index];
                document.source == selected_primary.source && document.line == selected_primary.line
            })
            .and_then(|primary| {
                associative_connection(
                    index,
                    &query_entities,
                    primary,
                    &document_frequency,
                    &selected,
                    verify_sources,
                    &mut verified,
                )
            })
    });
    if let Some(connection) = connection {
        selected.push(connection);
    }
    Ok(QueryOutcome {
        state: if selected.is_empty() {
            QueryState::NoHit
        } else {
            QueryState::Hit
        },
        hits: selected,
    })
}

fn recency_bonus(date: &str) -> f64 {
    let Ok(date) = NaiveDate::parse_from_str(date, "%Y-%m-%d") else {
        return 0.0;
    };
    let age_days = (Utc::now().date_naive() - date).num_days().max(0) as f64;
    (-age_days / 90.0).exp() * 0.2
}

fn verify_document(document: &IndexDocument, cache: &mut HashMap<String, bool>) -> bool {
    if let Some(result) = cache.get(&document.source) {
        return *result;
    }
    let result = expand_pointer(&document.source)
        .and_then(|path| {
            let metadata = fs::metadata(&path).ok()?;
            // The builder hashes every source into a snapshot no more than 24 hours old.
            // On the latency-critical path, nanosecond mtime + size proves that the source
            // is still the file whose hash the snapshot carries. Re-reading multi-megabyte
            // transcripts here made the probe slower than the 50 ms p95 budget.
            Some(metadata.len() == document.size && modified_ns(&metadata) == document.modified_ns)
        })
        .unwrap_or(false);
    cache.insert(document.source.clone(), result);
    result
}

fn associative_connection(
    index: &RecallIndex,
    query_entities: &BTreeMap<String, EntityEvidence>,
    primary: &RankedCandidate,
    document_frequency: &HashMap<&str, usize>,
    selected: &[RecallHit],
    verify_sources: bool,
    verified: &mut HashMap<String, bool>,
) -> Option<RecallHit> {
    let primary_document = &index.documents[primary.document_index];
    let document_count = index.documents.len().max(1);
    let max_bridge_frequency = (document_count / 4).max(2);
    let mut bridges: Vec<(&str, f64)> = primary_document
        .entities
        .iter()
        .filter(|(entity, evidence)| {
            !query_entities.contains_key(*entity)
                && kind_weight(evidence.kind, entity) >= 1.8
                && !generic_commands().contains(&entity.as_str())
        })
        .filter_map(|(entity, evidence)| {
            let df = *document_frequency.get(entity.as_str())?;
            if !(2..=max_bridge_frequency).contains(&df) {
                return None;
            }
            let idf = ((document_count as f64 + 1.0) / (df as f64 + 1.0)).ln() + 1.0;
            Some((entity.as_str(), idf * kind_weight(evidence.kind, entity)))
        })
        .collect();
    bridges.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));

    let selected_locations: BTreeSet<String> = selected
        .iter()
        .map(|hit| format!("{}:{}", hit.source, hit.line))
        .collect();
    let mut options = Vec::new();
    for (bridge, bridge_score) in bridges.into_iter().take(8) {
        for document in &index.documents {
            let location = format!("{}:{}", document.source, document.line);
            if selected_locations.contains(&location)
                || document.source == primary_document.source
                || !document.entities.contains_key(bridge)
                || document
                    .entities
                    .keys()
                    .any(|entity| query_entities.contains_key(entity))
            {
                continue;
            }
            options.push((
                document,
                bridge,
                bridge_score + recency_bonus(&document.date),
            ));
        }
    }
    options.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(Ordering::Equal));
    for (document, bridge, score) in options {
        if verify_sources && !verify_document(document, verified) {
            continue;
        }
        return Some(RecallHit {
            relation: "possible-connection",
            source: document.source.clone(),
            line: document.line,
            date: document.date.clone(),
            matched: vec![format!("via {}", bridge)],
            score,
        });
    }
    None
}

fn render_text(hits: &[RecallHit]) -> String {
    let mut output = String::from("Task recall — source pointers only; verify current state\n");
    for hit in hits {
        let label = if hit.relation == "possible-connection" {
            "possible connection"
        } else {
            "precedent"
        };
        let source = compact_source(&hit.source, 180);
        let matches = hit
            .matched
            .iter()
            .take(3)
            .map(|value| format!("`{}`", compact_entity(value, 70)))
            .collect::<Vec<_>>()
            .join(", ");
        let line = format!(
            "- {}: {}:{} ({}) — {}\n",
            label, source, hit.line, hit.date, matches
        );
        if output.len() + line.len() > MAX_OUTPUT_BYTES {
            break;
        }
        output.push_str(&line);
    }
    output
}

fn render_json(hits: &[RecallHit]) -> Result<String> {
    #[derive(Serialize)]
    struct CompactHit<'a> {
        relation: &'a str,
        source: String,
        line: usize,
        date: &'a str,
        matched: Vec<String>,
    }
    let mut compact: Vec<CompactHit<'_>> = hits
        .iter()
        .map(|hit| CompactHit {
            relation: hit.relation,
            source: compact_source(&hit.source, 160),
            line: hit.line,
            date: &hit.date,
            matched: hit
                .matched
                .iter()
                .take(3)
                .map(|value| compact_entity(value, 60))
                .collect(),
        })
        .collect();
    loop {
        let output = serde_json::to_string(&serde_json::json!({
            "warning": "source pointers only; verify current state",
            "hits": compact,
        }))?;
        if output.len() <= MAX_OUTPUT_BYTES || compact.len() <= 1 {
            return Ok(output);
        }
        compact.pop();
    }
}

fn compact_source(source: &str, max_chars: usize) -> String {
    if source.chars().count() <= max_chars {
        return source.to_string();
    }
    let parts: Vec<&str> = source.split('/').collect();
    let tail = parts
        .iter()
        .rev()
        .take(3)
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("/");
    let compact = format!("~/…/{tail}");
    if compact.chars().count() <= max_chars {
        compact
    } else {
        let suffix: String = compact
            .chars()
            .rev()
            .take(max_chars.saturating_sub(2))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("…/{suffix}")
    }
}

fn compact_entity(entity: &str, max_chars: usize) -> String {
    if entity.chars().count() <= max_chars {
        return entity.to_string();
    }
    let prefix: String = entity.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{prefix}…")
}

fn run_fixture_file(path: &Path) -> Result<BacktestReport> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("cannot read fixture file {}", path.display()))?;
    let fixtures: FixtureFile = serde_json::from_str(&text)
        .with_context(|| format!("cannot parse fixture file {}", path.display()))?;
    evaluate_fixtures(fixtures)
}

fn fixture_document_text(document: &FixtureDocument) -> Result<String> {
    match (&document.content, &document.path) {
        (Some(content), None) => Ok(content.clone()),
        (None, Some(path)) => fs::read_to_string(path)
            .with_context(|| format!("cannot read fixture source {}", path.display())),
        (Some(_), Some(_)) => bail!(
            "fixture document {} must set content or path, not both",
            document.source
        ),
        (None, None) => bail!(
            "fixture document {} must set either content or path",
            document.source
        ),
    }
}

fn fixture_to_index(fixtures: &FixtureFile) -> Result<(RecallIndex, Vec<(String, String)>)> {
    let mut documents = Vec::new();
    let mut baseline_corpus = Vec::new();
    for fixture in &fixtures.documents {
        let text = fixture_document_text(fixture)?;
        baseline_corpus.push((fixture.source.clone(), text.clone()));
        if is_sensitive_text(&text) {
            continue;
        }
        let entities = extract_entities(&text);
        if entities.is_empty() {
            continue;
        }
        documents.push(IndexDocument {
            source: fixture.source.clone(),
            source_kind: SourceKind::Fixture,
            line: 1,
            date: fixture.date.clone(),
            content_hash: sha256(text.as_bytes()),
            modified_ns: 0,
            size: text.len() as u64,
            entities,
        });
    }
    Ok((
        RecallIndex {
            schema_version: SCHEMA_VERSION,
            built_at: Utc::now().to_rfc3339(),
            corpus_fingerprint: "fixture".to_string(),
            documents,
        },
        baseline_corpus,
    ))
}

fn evaluate_fixtures(fixtures: FixtureFile) -> Result<BacktestReport> {
    if fixtures.documents.is_empty() || fixtures.cases.is_empty() {
        bail!("fixture must contain at least one document and one case");
    }
    let (index, baseline_corpus) = fixture_to_index(&fixtures)?;
    if index.documents.is_empty() {
        bail!("fixture source policy produced zero index documents");
    }
    let mut results = Vec::new();
    let mut red_control_detected = false;
    for case in &fixtures.cases {
        let outcome = query_index(&index, &case.query, false, true)?;
        let sources: BTreeSet<&str> = outcome.hits.iter().map(|hit| hit.source.as_str()).collect();
        let expected_present = case
            .expected_sources
            .iter()
            .all(|expected| sources.contains(expected.as_str()));
        let silent_ok = !case.expect_silent || outcome.hits.is_empty();
        let baseline_sources = strongest_cue_baseline(&baseline_corpus, &case.query);
        let baseline_missed = case
            .expected_sources
            .iter()
            .any(|expected| !baseline_sources.contains(expected));
        if case.baseline_must_miss && baseline_missed && expected_present {
            red_control_detected = true;
        }
        let passed = expected_present && silent_ok && (!case.baseline_must_miss || baseline_missed);
        results.push(CaseResult {
            name: case.name.clone(),
            passed,
            detail: if case.expect_silent {
                format!("{} hit(s); expected silence", outcome.hits.len())
            } else {
                format!(
                    "{} hit(s); expected {:?}; strongest-cue baseline {:?}",
                    outcome.hits.len(),
                    case.expected_sources,
                    baseline_sources
                )
            },
        });
    }

    let benchmark_query = fixtures
        .cases
        .iter()
        .find(|case| !case.expect_silent)
        .map(|case| case.query.as_str())
        .unwrap_or("`dotter deploy` failed");
    let mut timings = Vec::with_capacity(300);
    for _ in 0..300 {
        let started = Instant::now();
        let _ = query_index(&index, benchmark_query, false, true)?;
        timings.push(started.elapsed().as_micros());
    }
    timings.sort_unstable();
    let p95_index = ((timings.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(timings.len() - 1);
    let passed = results.iter().filter(|result| result.passed).count();
    Ok(BacktestReport {
        passed,
        total: results.len(),
        red_control_detected,
        p95_microseconds: timings[p95_index],
        cases: results,
    })
}

fn strongest_cue_baseline(corpus: &[(String, String)], query: &str) -> BTreeSet<String> {
    let query_entities = extract_query_entities(query);
    let mut candidates: Vec<(&String, &EntityEvidence)> = query_entities.iter().collect();
    candidates.sort_by(
        |(left_entity, left_evidence), (right_entity, right_evidence)| {
            kind_weight(right_evidence.kind, right_entity)
                .partial_cmp(&kind_weight(left_evidence.kind, left_entity))
                .unwrap_or(Ordering::Equal)
                .then_with(|| right_entity.len().cmp(&left_entity.len()))
                .then_with(|| left_entity.cmp(right_entity))
        },
    );

    for (entity, evidence) in candidates {
        let cue = if evidence.kind == EntityKind::Command && entity.contains(':') {
            entity.replace(':', " ")
        } else {
            entity.clone()
        };
        let matches: BTreeSet<String> = corpus
            .iter()
            .filter(|(_, content)| content.to_lowercase().contains(&cue))
            .map(|(source, _)| source.clone())
            .collect();
        if !matches.is_empty() {
            return matches;
        }
    }
    BTreeSet::new()
}

fn run_builtin_backtest() -> Result<BacktestReport> {
    evaluate_fixtures(FixtureFile {
        documents: vec![
            FixtureDocument {
                source: "fixture://bridge".to_string(),
                date: "2026-08-20".to_string(),
                content: Some(
                    "`cross-machine-sync-check` reported drift immediately after `dotter deploy`."
                        .to_string(),
                ),
                path: None,
            },
            FixtureDocument {
                source: "fixture://rare-analogue".to_string(),
                date: "2026-01-10".to_string(),
                content: Some(
                    "`dotter deploy` succeeded, but `dotter-orphan-detector-v2` exposed an unregistered source."
                        .to_string(),
                ),
                path: None,
            },
            FixtureDocument {
                source: "fixture://specific-old".to_string(),
                date: "2025-12-01".to_string(),
                content: Some(
                    "`sync-service-monitor` disagreed with `dotter-drift-monitor` after a platform mapping changed."
                        .to_string(),
                ),
                path: None,
            },
            FixtureDocument {
                source: "fixture://frecency-bait".to_string(),
                date: "2026-09-02".to_string(),
                content: Some("Repeated `git status` checks during unrelated cleanup.".to_string()),
                path: None,
            },
            FixtureDocument {
                source: "fixture://sensitive".to_string(),
                date: "2026-09-02".to_string(),
                content: Some(
                    "Do not inspect /Clinical/clients/Example-Person/session.md with `tm3-diary-capture`."
                        .to_string(),
                ),
                path: None,
            },
        ],
        cases: vec![
            FixtureCase {
                name: "rare analogue via bounded co-occurrence".to_string(),
                query: "`cross-machine-sync-check` says clean, but the deployed target is absent"
                    .to_string(),
                expected_sources: vec![
                    "fixture://bridge".to_string(),
                    "fixture://rare-analogue".to_string(),
                ],
                expect_silent: false,
                baseline_must_miss: true,
            },
            FixtureCase {
                name: "specific precedent beats recent generic bait".to_string(),
                query: "Why do `sync-service-monitor` and `dotter-drift-monitor` disagree?"
                    .to_string(),
                expected_sources: vec!["fixture://specific-old".to_string()],
                expect_silent: false,
                baseline_must_miss: false,
            },
            FixtureCase {
                name: "greeting remains silent".to_string(),
                query: "how's it going?".to_string(),
                expected_sources: Vec::new(),
                expect_silent: true,
                baseline_must_miss: false,
            },
            FixtureCase {
                name: "bare role invocation remains silent".to_string(),
                query: "$senior-dev".to_string(),
                expected_sources: Vec::new(),
                expect_silent: true,
                baseline_must_miss: false,
            },
            FixtureCase {
                name: "self-contained edit remains silent".to_string(),
                query: "rename this function".to_string(),
                expected_sources: Vec::new(),
                expect_silent: true,
                baseline_must_miss: false,
            },
            FixtureCase {
                name: "mid-session task shift is queryable".to_string(),
                query: "Check `sync-service-monitor` after the topic changed".to_string(),
                expected_sources: vec!["fixture://specific-old".to_string()],
                expect_silent: false,
                baseline_must_miss: false,
            },
            FixtureCase {
                name: "sensitive prompt is refused".to_string(),
                query: "Run `tm3-diary-capture` for /Clinical/clients/Example-Person"
                    .to_string(),
                expected_sources: Vec::new(),
                expect_silent: true,
                baseline_must_miss: false,
            },
            FixtureCase {
                name: "prose without structural cues remains silent".to_string(),
                query: "What did we learn from the strange problem last winter?".to_string(),
                expected_sources: Vec::new(),
                expect_silent: true,
                baseline_must_miss: false,
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_directory(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "continuum-task-recall-{}-{}-{}",
            name,
            std::process::id(),
            nonce
        ));
        fs::create_dir_all(&path).expect("create temp directory");
        path
    }

    #[test]
    fn builtin_backtest_has_green_algorithm_and_red_control() {
        let report = run_builtin_backtest().expect("backtest runs");
        assert_eq!(report.passed, report.total, "{:#?}", report.cases);
        assert!(report.red_control_detected);
        assert!(report.p95_microseconds <= 50_000);
    }

    #[test]
    fn rendered_hit_payload_is_bounded() {
        let hits = (0..4)
            .map(|index| RecallHit {
                relation: if index == 3 {
                    "possible-connection"
                } else {
                    "primary"
                },
                source: format!("~/very/long/source/{}/{}.md", "x".repeat(1_000), index),
                line: 42,
                date: "2026-09-02".to_string(),
                matched: vec!["y".repeat(200), "z".repeat(200)],
                score: 10.0,
            })
            .collect::<Vec<_>>();
        assert!(render_text(&hits).len() <= MAX_OUTPUT_BYTES);
        assert!(render_json(&hits).expect("json renders").len() <= MAX_OUTPUT_BYTES);
    }

    #[test]
    fn changed_source_is_rejected_even_when_size_is_unchanged() {
        let root = temporary_directory("stale-source");
        let source = root.join("incident.md");
        fs::write(&source, "Use `dotter deploy` today.").expect("write source");
        let metadata = fs::metadata(&source).expect("metadata");
        let original = fs::read(&source).expect("read source");
        let document = IndexDocument {
            source: source.display().to_string(),
            source_kind: SourceKind::Fixture,
            line: 1,
            date: "2026-09-02".to_string(),
            content_hash: sha256(&original),
            modified_ns: modified_ns(&metadata),
            size: metadata.len(),
            entities: extract_entities("Use `dotter deploy` today."),
        };
        fs::write(&source, "Use `dotter status` today.").expect("change source");
        let index = RecallIndex {
            schema_version: SCHEMA_VERSION,
            built_at: Utc::now().to_rfc3339(),
            corpus_fingerprint: "test".to_string(),
            documents: vec![document],
        };
        let outcome =
            query_index(&index, "Why did `dotter deploy` fail?", true, true).expect("query runs");
        assert!(outcome.hits.is_empty());
        fs::remove_dir_all(root).expect("remove isolated temp directory");
    }

    #[test]
    fn stale_index_fails_open_without_hits() {
        let index = RecallIndex {
            schema_version: SCHEMA_VERSION,
            built_at: (Utc::now() - Duration::hours(24) - Duration::minutes(1)).to_rfc3339(),
            corpus_fingerprint: "test".to_string(),
            documents: vec![IndexDocument {
                source: "fixture://old".to_string(),
                source_kind: SourceKind::Fixture,
                line: 1,
                date: "2026-01-01".to_string(),
                content_hash: "hash".to_string(),
                modified_ns: 0,
                size: 0,
                entities: extract_entities("`dotter deploy`"),
            }],
        };
        let outcome =
            query_index(&index, "`dotter deploy` failed", false, true).expect("query runs");
        assert_eq!(outcome.state, QueryState::StaleIndex);
        assert!(outcome.hits.is_empty());
    }

    #[test]
    fn stale_primary_cannot_seed_an_associative_hit() {
        let root = temporary_directory("stale-primary");
        let associated_source = root.join("associated.md");
        let associated_text = "`dotter deploy` exposed an old configuration incident.";
        fs::write(&associated_source, associated_text).expect("write associated source");
        let associated_metadata = fs::metadata(&associated_source).expect("associated metadata");
        let index = RecallIndex {
            schema_version: SCHEMA_VERSION,
            built_at: Utc::now().to_rfc3339(),
            corpus_fingerprint: "test".to_string(),
            documents: vec![
                IndexDocument {
                    source: "/definitely/missing/direct.md".to_string(),
                    source_kind: SourceKind::Fixture,
                    line: 1,
                    date: "2026-09-02".to_string(),
                    content_hash: "missing".to_string(),
                    modified_ns: 0,
                    size: 0,
                    entities: extract_entities(
                        "`cross-machine-sync-check` failed on nimbini after `dotter deploy`.",
                    ),
                },
                IndexDocument {
                    source: associated_source.display().to_string(),
                    source_kind: SourceKind::Fixture,
                    line: 1,
                    date: "2026-01-01".to_string(),
                    content_hash: sha256(associated_text.as_bytes()),
                    modified_ns: modified_ns(&associated_metadata),
                    size: associated_metadata.len(),
                    entities: extract_entities(associated_text),
                },
            ],
        };
        let outcome = query_index(
            &index,
            "`cross-machine-sync-check` says clean on nimbini",
            true,
            true,
        )
        .expect("query runs");
        assert!(outcome.hits.is_empty());
        fs::remove_dir_all(root).expect("remove isolated temp directory");
    }

    #[test]
    fn empty_corpus_is_not_reported_as_success() {
        let root = temporary_directory("empty-corpus");
        let cache = root.join("cache/index.json");
        let error = build_index(&root, &cache, true).expect_err("empty corpus must fail");
        assert!(error.to_string().contains("no candidate sources"));
        fs::remove_dir_all(root).expect("remove isolated temp directory");
    }

    #[test]
    fn mixed_persona_or_clinical_skills_are_not_eligible() {
        let root = temporary_directory("policy");
        let session = root.join("Assistants/continuum-logs/claude-code/2026-09-02/session");
        fs::create_dir_all(&session).expect("create fixture session");
        fs::write(
            session.join("session.json"),
            r#"{"skills":["clinical-notes","senior-dev"],"start_time":"2026-09-02T10:00:00Z"}"#,
        )
        .expect("write metadata");
        fs::write(
            session.join("messages.jsonl"),
            r#"{"role":"user","content":"Run `dotter deploy`"}"#,
        )
        .expect("write messages");
        let mut stats = BuildStats::default();
        let sources = discover_sources(&root, &mut stats).expect("discover sources");
        assert!(sources.is_empty());
        assert_eq!(stats.examined_sources, 1);
        assert_eq!(stats.skipped_policy, 1);
        fs::remove_dir_all(root).expect("remove isolated temp directory");
    }

    #[test]
    fn credential_shaped_code_spans_are_not_indexed() {
        let entities = extract_entities(
            "The rejected values were `sk-proj-examplecredential123` and `ghp_examplecredential`.",
        );
        assert!(!entities
            .keys()
            .any(|entity| entity.starts_with("sk-") || entity.starts_with("ghp_")));
    }

    #[test]
    fn web_urls_are_not_misclassified_as_filesystem_paths() {
        let entities = extract_entities("See https://example.com/project/status for details.");
        assert!(entities.is_empty());
    }

    #[test]
    fn prose_fragments_are_not_misclassified_as_filesystem_paths() {
        for prose in [
            "this will take ~1 hour, maybe ~80% of the afternoon",
            "this ran ~1hr, began ~10am, and repeated ~5x",
            "should I do this and/or that with the ui",
            "the counter is 23/126 today",
        ] {
            let entities = extract_entities(prose);
            assert!(
                entities.is_empty(),
                "unexpected entities for {prose:?}: {entities:?}"
            );
        }
    }

    #[test]
    fn structural_filesystem_paths_remain_technical_cues() {
        for path in ["/tmp/example", "~/dotfiles/scripts", "./src"] {
            let entities = extract_entities(path);
            assert!(
                entities
                    .values()
                    .any(|evidence| evidence.kind == EntityKind::Path),
                "missing path cue for {path:?}: {entities:?}"
            );
        }
    }

    #[test]
    fn iso_dates_require_exact_zero_padded_syntax() {
        assert!(is_iso_date("2026-06-01"));
        for invalid in [
            " 2026-06-01",
            "2026-6-01",
            "2026-06-1",
            "2026-06-01 ",
            "2026-02-30",
        ] {
            assert!(!is_iso_date(invalid), "accepted invalid date {invalid:?}");
        }
    }

    #[test]
    fn clinical_root_without_a_trailing_slash_is_sensitive() {
        assert!(is_sensitive_text("Never inspect ~/Clinical"));
        assert!(is_sensitive_entity("~/clinical"));
    }

    #[test]
    fn strongest_cue_baseline_finds_direct_document_but_not_associative_analogue() {
        let corpus = vec![
            (
                "fixture://bridge".to_string(),
                "`cross-machine-sync-check` reported drift after `dotter deploy`.".to_string(),
            ),
            (
                "fixture://analogue".to_string(),
                "`dotter deploy` succeeded but an unregistered source was absent.".to_string(),
            ),
        ];
        let sources = strongest_cue_baseline(
            &corpus,
            "`cross-machine-sync-check` says clean, but the deployed target is absent",
        );
        assert!(sources.contains("fixture://bridge"));
        assert!(!sources.contains("fixture://analogue"));
    }

    #[test]
    fn prose_subcommands_are_loose_in_queries_but_strict_in_index_documents() {
        let prose_document = extract_entities("The operator ran dotter deploy yesterday.");
        assert!(prose_document.contains_key("dotter"));
        assert!(!prose_document.contains_key("dotter:deploy"));

        let explicit_document = extract_entities("The operator ran `dotter deploy` yesterday.");
        assert!(explicit_document.contains_key("dotter:deploy"));

        let prose_query = extract_query_entities("Why did dotter deploy fail?");
        assert!(prose_query.contains_key("dotter:deploy"));
    }
}
