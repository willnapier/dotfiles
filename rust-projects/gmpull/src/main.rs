//! gmpull — Gmail REST API → maildir, lieer's Rust replacement.
//!
//! See `~/Assistants/shared/gmpull.md` for the architecture rationale
//! and cutover plan from lieer.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
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
    let session_skipped = AtomicU64::new(0);
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
            in_flight.push(tokio::spawn(async move {
                let _permit = sem_c
                    .acquire_owned()
                    .await
                    .context("acquiring fetch semaphore")?;
                fetch_and_write_one(http_c, token_c, &id_c, &root_c, &labels_c, &limiter_c)
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
                        &session_skipped,
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
                &session_skipped,
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
                &session_skipped,
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
        &session_skipped,
        &session_errored,
        started,
    );

    // Final flush — always preserve cumulative `messages_pulled`.
    state.messages_pulled =
        prior_pulled.saturating_add(session_written.load(Ordering::Relaxed));
    state::save_lossy(&state).await;

    Ok(())
}

/// Fetch one message and write it to the maildir. Returns:
///   Ok(true)  — wrote (or already existed at the right path)
///   Ok(false) — skipped (e.g. TRASH)
///   Err(e)    — fetch or write failed
async fn fetch_and_write_one(
    http: reqwest::Client,
    token: Arc<tokio::sync::RwLock<String>>,
    id: &str,
    maildir_root: &std::path::Path,
    labels_map: &std::collections::HashMap<String, String>,
    limiter: &SharedRateLimiter,
) -> Result<bool> {
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
        return Ok(false);
    }

    maildir::write_message(maildir_root, &msg, labels_map).await?;
    Ok(true)
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
    joined: Result<Result<bool>, tokio::task::JoinError>,
    written: &AtomicU64,
    skipped: &AtomicU64,
    errored: &AtomicU64,
) {
    match joined {
        Ok(Ok(true)) => {
            written.fetch_add(1, Ordering::Relaxed);
        }
        Ok(Ok(false)) => {
            skipped.fetch_add(1, Ordering::Relaxed);
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
    skipped: &AtomicU64,
    errored: &AtomicU64,
    started: Instant,
) {
    let w = written.load(Ordering::Relaxed);
    let s = skipped.load(Ordering::Relaxed);
    let e = errored.load(Ordering::Relaxed);
    let elapsed = started.elapsed().as_secs_f64().max(0.001);
    let rate = w as f64 / elapsed;
    tracing::info!(
        written = w,
        skipped = s,
        errored = e,
        elapsed_s = elapsed as u64,
        msg_per_s = format!("{:.1}", rate),
        "progress"
    );
}
