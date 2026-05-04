//! gmpull — Gmail REST API → maildir, lieer's Rust replacement.
//!
//! See `~/Assistants/shared/gmpull.md` for the architecture rationale
//! and cutover plan from lieer.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

mod api;
mod auth;
mod labels;
mod maildir;
mod state;

use api::{
    DEFAULT_FETCH_CONCURRENCY, DEFAULT_RATE_BURST_UNITS, DEFAULT_RATE_UNITS_PER_SEC,
    SharedRateLimiter, build_fetch_semaphore, build_rate_limiter,
};

/// Outcome of one `fetch_and_write_one` call. We split "deduped"
/// (skipped because the file already exists on disk — the common
/// case on resume / steady-state ticks) from "filtered" (server-
/// returned a TRASH/SPAM message we don't want) so the operator can
/// read the log and immediately see which case dominates.
#[derive(Debug, Clone, Copy)]
enum FetchOutcome {
    Written,
    Deduped,
    Filtered,
}

/// Save the checkpoint every N messages. 100 keeps disk writes cheap
/// while bounding redo on crash to one page worth.
const CHECKPOINT_EVERY: u64 = 100;

#[derive(Parser, Debug)]
#[command(name = "gmpull", version, about = "Pull Gmail via REST API into a maildir")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Pull messages from Gmail into the maildir.
    Pull {
        /// Maildir root (default: ~/Mail/gmail-rs).
        #[arg(long)]
        maildir: Option<PathBuf>,
        /// Resume from checkpoint (default behaviour). Reserved for
        /// symmetry with a future `--restart` flag.
        #[arg(long, default_value_t = true)]
        resume: bool,
        /// Stop after writing this many messages this session. Useful
        /// for smoke testing.
        #[arg(long)]
        max_messages: Option<u64>,
        /// Quota-units/second cap (default 150 — Gmail's per-100 s
        /// sustained budget is 15 000 units → 150/s). Lower this to
        /// 100 if 150/s still trips the quota; raise it cautiously
        /// only if Gmail confirms a higher per-user allowance.
        #[arg(long, default_value_t = DEFAULT_RATE_UNITS_PER_SEC)]
        rate_limit: u32,
        /// Burst-bucket size in quota units (default 750 — ~5 s of
        /// the rate cap).
        #[arg(long, default_value_t = DEFAULT_RATE_BURST_UNITS)]
        rate_burst: u32,
        /// Concurrent in-flight `messages.get` cap (default 3).
        #[arg(long, default_value_t = DEFAULT_FETCH_CONCURRENCY)]
        concurrency: usize,
    },
}

fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;

    runtime.block_on(async move {
        match cli.cmd {
            Cmd::Pull {
                maildir,
                resume,
                max_messages,
                rate_limit,
                rate_burst,
                concurrency,
            } => {
                pull(
                    maildir,
                    resume,
                    max_messages,
                    rate_limit,
                    rate_burst,
                    concurrency,
                )
                .await
            }
        }
    })
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,reqwest=warn,hyper=warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

async fn pull(
    maildir_arg: Option<PathBuf>,
    _resume: bool,
    max_messages: Option<u64>,
    rate_limit: u32,
    rate_burst: u32,
    concurrency: usize,
) -> Result<()> {
    let maildir_root = maildir_arg
        .map(Ok)
        .unwrap_or_else(state::default_maildir)?;

    tracing::info!(
        maildir = %maildir_root.display(),
        rate_limit_units_per_s = rate_limit,
        rate_burst_units = rate_burst,
        concurrency,
        "starting pull"
    );
    maildir::ensure_maildir(&maildir_root).await?;
    state::ensure_state_dir().await?;

    let mut state = state::load().await?;
    let prior_pulled = state.messages_pulled;
    tracing::info!(
        resuming_from_token = ?state.last_page_token,
        prior_messages_pulled = prior_pulled,
        "loaded state"
    );

    // Build the on-disk dedup set ONCE. Each entry is ~16 bytes; a
    // 112k-message mailbox is ~2 MB heap. The walk takes well under
    // a second on APFS even at six figures. Without this set, every
    // tick of a fully-populated maildir would re-fetch every message
    // via `messages.get` (5 quota units each) and rely on the
    // tmp→cur rename to overwrite — burning ~560k units to make
    // zero net progress on a 112k mailbox.
    let load_started = Instant::now();
    let existing_ids = {
        let root = maildir_root.clone();
        tokio::task::spawn_blocking(move || maildir::load_existing_gmail_ids(&root))
            .await
            .context("joining existing-ids load")??
    };
    tracing::info!(
        existing_message_count = existing_ids.len(),
        load_ms = load_started.elapsed().as_millis() as u64,
        "loaded on-disk dedup set"
    );
    let existing_ids_arc: Arc<tokio::sync::RwLock<HashSet<String>>> =
        Arc::new(tokio::sync::RwLock::new(existing_ids));

    let http = api::build_client()?;
    let limiter: SharedRateLimiter = build_rate_limiter(rate_limit, rate_burst);
    let fetch_sem = build_fetch_semaphore(concurrency);
    let token = auth::access_token().context("getting initial pizauth token")?;
    let token_arc: Arc<tokio::sync::RwLock<String>> = Arc::new(tokio::sync::RwLock::new(token));

    let labels_map = {
        let t = token_arc.read().await;
        labels::list_labels(&http, &t).await?
    };
    tracing::info!(label_count = labels_map.len(), "fetched label cache");
    let labels_arc = Arc::new(labels_map);

    let session_written = AtomicU64::new(0);
    let session_deduped = AtomicU64::new(0);
    let session_filtered = AtomicU64::new(0);
    let session_errored = AtomicU64::new(0);
    let started = Instant::now();

    let mut page_token = state.last_page_token.clone();
    let mut pages_processed: u64 = 0;
    let mut last_log = Instant::now();

    'pages: loop {
        // List one page of IDs.
        let (ids, next_token) = {
            let t = token_arc.read().await;
            match api::list_messages_page(&http, &t, page_token.as_deref(), &limiter).await {
                Ok(v) => v,
                Err(e) if e.to_string().contains("401 unauthorized") => {
                    drop(t);
                    refresh_token(&token_arc).await?;
                    let t = token_arc.read().await;
                    api::list_messages_page(&http, &t, page_token.as_deref(), &limiter).await?
                }
                Err(e) => return Err(e.context("messages.list failed")),
            }
        };

        if ids.is_empty() && next_token.is_none() {
            tracing::info!("no more pages — pull complete");
            // Flag completion in state so watchers can detect it.
            state.last_page_token = None;
            state.messages_pulled =
                prior_pulled.saturating_add(session_written.load(Ordering::Relaxed));
            state::save_lossy(&state).await;
            break;
        }
        pages_processed += 1;
        tracing::debug!(page = pages_processed, ids = ids.len(), "page fetched");

        // Fetch this page concurrently. The limiter and semaphore
        // are the real concurrency governors — `FuturesUnordered`
        // is just bookkeeping.
        use futures::stream::{FuturesUnordered, StreamExt};
        let mut in_flight = FuturesUnordered::new();
        for id in ids.iter() {
            let http_c = http.clone();
            let token_c = token_arc.clone();
            let labels_c = labels_arc.clone();
            let root_c = maildir_root.clone();
            let id_c = id.id.clone();
            let limiter_c = limiter.clone();
            let sem_c = fetch_sem.clone();
            let existing_c = existing_ids_arc.clone();
            in_flight.push(tokio::spawn(async move {
                let _permit = sem_c
                    .acquire_owned()
                    .await
                    .context("acquiring fetch semaphore")?;
                fetch_and_write_one(
                    http_c, token_c, &id_c, &root_c, &labels_c, &limiter_c, &existing_c,
                )
                .await
            }));

            // Allow up to `concurrency * 4` queued tasks before we
            // start draining; this keeps the semaphore the real
            // concurrency cap rather than `FuturesUnordered`.
            let queue_cap = concurrency.saturating_mul(4).max(8);
            while in_flight.len() >= queue_cap {
                if let Some(joined) = in_flight.next().await {
                    handle_one(
                        joined,
                        &session_written,
                        &session_deduped,
                        &session_filtered,
                        &session_errored,
                    );
                }
            }
        }
        // Drain remaining tasks for this page.
        while let Some(joined) = in_flight.next().await {
            handle_one(
                joined,
                &session_written,
                &session_deduped,
                &session_filtered,
                &session_errored,
            );
        }

        // Save the *next* page token so a crash here resumes from
        // the next page rather than re-doing this one.
        state.last_page_token = next_token.clone();

        // Periodic progress log.
        if last_log.elapsed().as_secs() >= 30 {
            log_progress(
                &session_written,
                &session_deduped,
                &session_filtered,
                &session_errored,
                started,
            );
            last_log = Instant::now();
        }

        // Save state at end of every page (500 messages or fewer ≪
        // CHECKPOINT_EVERY worth of redo on crash).
        let _ = CHECKPOINT_EVERY; // future-proof: per-message checkpoint hook
        state.messages_pulled =
            prior_pulled.saturating_add(session_written.load(Ordering::Relaxed));
        state::save_lossy(&state).await;

        if let Some(cap) = max_messages
            && session_written.load(Ordering::Relaxed) >= cap
        {
            tracing::info!(cap, "reached --max-messages");
            break 'pages;
        }

        match next_token {
            Some(t) => page_token = Some(t),
            None => {
                tracing::info!("reached final page — pull complete");
                state.last_page_token = None;
                state.messages_pulled =
                    prior_pulled.saturating_add(session_written.load(Ordering::Relaxed));
                state::save_lossy(&state).await;
                break;
            }
        }
    }

    log_progress(
        &session_written,
        &session_deduped,
        &session_filtered,
        &session_errored,
        started,
    );

    // Final flush — always preserve cumulative `messages_pulled`.
    state.messages_pulled =
        prior_pulled.saturating_add(session_written.load(Ordering::Relaxed));
    state::save_lossy(&state).await;

    Ok(())
}

/// Fetch one message and write it to the maildir. Returns a
/// [`FetchOutcome`] distinguishing the four cases the caller cares
/// about: wrote a new file, deduped against the on-disk set (no
/// network call made), filtered by label (TRASH/SPAM — network call
/// happened, message discarded), or errored.
///
/// The `existing_ids` set is consulted *before* we hit `messages.get`.
/// On a hit we return immediately, saving 5 quota units. On a write
/// we insert the id into the set so a later page that lists the
/// same id doesn't re-fetch it (Gmail's pagination isn't perfectly
/// dedupe-clean across pages).
async fn fetch_and_write_one(
    http: reqwest::Client,
    token: Arc<tokio::sync::RwLock<String>>,
    id: &str,
    maildir_root: &std::path::Path,
    labels_map: &std::collections::HashMap<String, String>,
    limiter: &SharedRateLimiter,
    existing_ids: &Arc<tokio::sync::RwLock<HashSet<String>>>,
) -> Result<FetchOutcome> {
    // Cheapest possible early-exit: the file is already on disk.
    // This is the path that converts a re-pull of a fully-backed-up
    // mailbox from "560k quota units doing zero work" into "500
    // quota units of pure messages.list enumeration".
    {
        let set = existing_ids.read().await;
        if set.contains(id) {
            return Ok(FetchOutcome::Deduped);
        }
    }

    let msg = {
        let t = token.read().await;
        match api::get_message_raw(&http, &t, id, limiter).await {
            Ok(m) => m,
            Err(e) if e.to_string().contains("401 unauthorized") => {
                drop(t);
                refresh_token(&token).await?;
                let t = token.read().await;
                api::get_message_raw(&http, &t, id, limiter).await?
            }
            Err(e) => return Err(e.context(format!("messages.get id={id}"))),
        }
    };

    if maildir::should_skip(&msg.label_ids) {
        return Ok(FetchOutcome::Filtered);
    }

    maildir::write_message(maildir_root, &msg, labels_map).await?;

    // Record the id so a duplicate listing later in this same pull
    // doesn't pay for `messages.get` a second time. The write lock
    // is held briefly (one HashSet insert) — contention is minimal
    // because dedup hits take only the read lock and writes are rare.
    {
        let mut set = existing_ids.write().await;
        set.insert(id.to_string());
    }

    Ok(FetchOutcome::Written)
}

/// Refresh the cached token via pizauth. Holds the write lock for
/// the duration of the subprocess (~10ms).
async fn refresh_token(slot: &Arc<tokio::sync::RwLock<String>>) -> Result<()> {
    let new_token =
        tokio::task::spawn_blocking(auth::access_token)
            .await
            .context("joining token refresh task")??;
    let mut w = slot.write().await;
    *w = new_token;
    Ok(())
}

fn handle_one(
    joined: Result<Result<FetchOutcome>, tokio::task::JoinError>,
    written: &AtomicU64,
    deduped: &AtomicU64,
    filtered: &AtomicU64,
    errored: &AtomicU64,
) {
    match joined {
        Ok(Ok(FetchOutcome::Written)) => {
            written.fetch_add(1, Ordering::Relaxed);
        }
        Ok(Ok(FetchOutcome::Deduped)) => {
            deduped.fetch_add(1, Ordering::Relaxed);
        }
        Ok(Ok(FetchOutcome::Filtered)) => {
            filtered.fetch_add(1, Ordering::Relaxed);
        }
        Ok(Err(e)) => {
            errored.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(error = %e, "fetch_and_write_one failed");
        }
        Err(e) => {
            errored.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(error = %e, "task join failed");
        }
    }
}

fn log_progress(
    written: &AtomicU64,
    deduped: &AtomicU64,
    filtered: &AtomicU64,
    errored: &AtomicU64,
    started: Instant,
) {
    let w = written.load(Ordering::Relaxed);
    let d = deduped.load(Ordering::Relaxed);
    let f = filtered.load(Ordering::Relaxed);
    let e = errored.load(Ordering::Relaxed);
    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    let rate = w as f64 / elapsed;
    // `skipped` is preserved as a sum (deduped + filtered) so older
    // log scrapers still get the same field. The new `deduped`
    // counter tells the operator how many `messages.get` calls were
    // saved by the on-disk filename check, which is the dominant
    // signal once the mailbox is fully backed up.
    let skipped = d.saturating_add(f);
    tracing::info!(
        written = w,
        skipped = skipped,
        deduped = d,
        filtered = f,
        errored = e,
        elapsed_s = elapsed as u64,
        msg_per_s = format!("{:.1}", rate),
        "progress"
    );
}
