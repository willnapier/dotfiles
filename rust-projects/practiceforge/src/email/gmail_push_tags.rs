//! Gmail push-tags — reflect local notmuch tag changes back to Gmail labels.
//!
//! The missing half of an IMAP + notmuch Gmail stack. mbsync handles
//! Gmail → local (label-folders show up as maildirs, notmuch indexes).
//! This module handles local → Gmail: you `notmuch tag +archive` a
//! message, next push-tags tick calls Gmail's REST `modify` endpoint to
//! apply the same label change on the server. The result is feature
//! parity with lieer's `gmi push` without the Python dependency.
//!
//! ## Strategy
//!
//! 1. **Discover changes via notmuch's `lastmod`** counter. Every
//!    tag-modifying operation bumps it; we persist the last seen value
//!    and query for messages with `lastmod:NN..`. No notmuch content
//!    scraping; notmuch does the heavy lifting.
//! 2. **Resolve RFC Message-ID → Gmail opaque ID** *locally* by reading
//!    the maildir filename. lieer (and our gmail_pull) writes each
//!    Gmail message to a file named `<gmail-id>:2,<flags>` — the opaque
//!    Gmail ID is literally the basename. We ask notmuch for the file
//!    path of the message (`notmuch search --output=files`) and parse
//!    it. This eliminates a Gmail API round-trip per candidate that
//!    used to dominate scan-phase wall time on large backlogs (the
//!    previous one-API-call-per-candidate rate ceilinged at ~7
//!    msg/sec). If the local parse fails for any reason — non-Gmail
//!    maildir, weird filename, message not yet on disk — we fall back
//!    to the original Gmail-search path so we never silently drop a
//!    candidate.
//! 3. **Diff read-then-write**: fetch Gmail's current label set for the
//!    message, compute (add, remove) relative to notmuch's tags, and
//!    only send the delta to `/messages/{id}/modify`. This is idempotent
//!    and correct whether the tag change originated locally or came in
//!    via mbsync (mbsync-induced "tag changes" diff to zero against
//!    Gmail's existing state, so no spurious round-trips).
//! 4. **State file**: `~/.local/share/practiceforge/gmail-push-state.json`
//!    holds `last_notmuch_lastmod` + the label-ID→name cache (refreshed
//!    daily — covers user creating new labels in Gmail web).
//!
//! ## Tag ↔ Label mapping
//!
//! Notmuch system tags map to Gmail system labels by uppercase ID:
//! `inbox`→`INBOX`, `unread`→`UNREAD`, `trash`→`TRASH`, `spam`→`SPAM`,
//! `flagged`→`STARRED`, `important`→`IMPORTANT`. The `sent` and `draft`
//! notmuch tags are intentionally NOT pushed — Gmail manages those
//! system labels itself (see SYSTEM_TAG_TO_LABEL doc comment for full
//! rationale). User tags map 1:1 by display name (which Gmail resolves
//! to the stored `Label_<n>` ID via the cached map).
//!
//! ## Safety
//!
//! The tool defaults to dry-run mode; `--push` flag required to actually
//! write to Gmail. State file only advances on successful real pushes.
//! Any error aborts the whole run without updating state, so a retry
//! picks up the same change-set.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

const STATE_DIR: &str = ".local/share/practiceforge";
const STATE_FILE: &str = "gmail-push-state.json";
const LABEL_MAP_MAX_AGE_SECS: i64 = 86_400; // 24h

/// System-tag → system-label-ID mapping. These Gmail IDs are uppercase
/// literals rather than opaque `Label_<n>` strings.
///
/// **Gmail API constraint:** `SENT`, `DRAFT`, `OUTBOX`, `CHAT` are
/// auto-derived by Gmail from message state (drafts folder, sent folder,
/// outbox folder, hangouts) and the `users.messages.modify` endpoint
/// rejects attempts to add or remove them with HTTP 400 "Invalid label".
/// They MUST NOT appear in this map. The corresponding notmuch tags
/// (`sent`, `draft`) are still set locally by lieer when Gmail returns
/// the labels, but they're read-only on the Gmail side — pushing local
/// `sent`/`draft` changes is impossible regardless. Discovered 2026-04-27
/// when nimbini's gmail-push-tags hung in a 3.5h retry loop on a `SENT`
/// modify rejection.
const SYSTEM_TAG_TO_LABEL: &[(&str, &str)] = &[
    ("inbox", "INBOX"),
    ("unread", "UNREAD"),
    ("trash", "TRASH"),
    ("spam", "SPAM"),
    ("flagged", "STARRED"),
    ("important", "IMPORTANT"),
];

/// Notmuch tags that reflect local workflow only and must never be
/// pushed as Gmail labels. Contains notmuch internals + common
/// auto-tagger outputs that aren't intended to become server-side
/// labels.
///
/// See also [`is_local_only_tag`] which extends this list with
/// prefix-matching for tag families like `curator-*-seen`
/// (mail-curator's internal "already processed" markers — there can
/// be an arbitrary number of them, one per sender policy).
const LOCAL_ONLY_TAGS: &[&str] = &[
    "attachment",
    "signed",
    "encrypted",
    "replied",
    "passed",
    "new",
];

/// True if `tag` should be filtered out of anything we push to Gmail.
/// Combines the exact-match [`LOCAL_ONLY_TAGS`] list with prefix
/// rules for tag families.
fn is_local_only_tag(tag: &str) -> bool {
    if LOCAL_ONLY_TAGS.contains(&tag) {
        return true;
    }
    // mail-curator bookkeeping tags — purely local, must never
    // become Gmail labels. Matches `curator-<anything>-seen`.
    if tag.starts_with("curator-") && tag.ends_with("-seen") {
        return true;
    }
    false
}

#[derive(Serialize, Deserialize, Default)]
struct State {
    #[serde(default)]
    last_notmuch_lastmod: u64,
    #[serde(default)]
    label_name_to_id: BTreeMap<String, String>,
    #[serde(default)]
    label_map_fetched_at: i64,
    /// Message-IDs already pushed within the current in-progress scan
    /// window. Used to checkpoint partial progress when a single run is
    /// killed mid-batch (e.g. macOS launchd `StartInterval=900` SIGTERMs
    /// the process after 15 min on a multi-thousand-message backlog).
    /// On restart, candidates whose ID is in this set are skipped, so
    /// work doesn't repeat. Cleared and re-seeded each time the
    /// `pushed_in_progress_since` anchor changes.
    #[serde(default)]
    pushed_in_progress: BTreeSet<String>,
    /// The `since` (i.e. `last_notmuch_lastmod`) at the start of the
    /// scan window that produced `pushed_in_progress`. If this no
    /// longer matches the current `last_notmuch_lastmod`, the set is
    /// stale (a previous run completed and advanced state) and gets
    /// discarded.
    #[serde(default)]
    pushed_in_progress_since: u64,
}

/// CLI entry point. `dry_run=true` means: log what would change, don't
/// touch Gmail and don't advance state. `dry_run=false` issues modify
/// calls and advances the state file on success.
pub fn run(dry_run: bool) -> Result<()> {
    let state_path = state_file_path()?;
    let mut state = load_state(&state_path)?;
    let token = access_token()?;

    if label_map_stale(&state) {
        eprintln!("Refreshing Gmail label map…");
        state.label_name_to_id = fetch_label_map(&token)?;
        state.label_map_fetched_at = chrono::Utc::now().timestamp();
    }

    let current_lastmod = current_notmuch_lastmod()?;

    // First run safety: if we have no prior state, DON'T attempt to
    // reconcile the whole notmuch history against Gmail — that would
    // mean one Gmail search per existing message (tens of thousands of
    // API calls, immediate quota burn). Instead, bookmark the current
    // lastmod and exit. From next invocation on, we only process real
    // deltas.
    if state.last_notmuch_lastmod == 0 {
        eprintln!(
            "First run: bookmarking current notmuch lastmod={current_lastmod} without scanning history. \
             Future ticks will push only new changes."
        );
        if !dry_run {
            state.last_notmuch_lastmod = current_lastmod;
            save_state(&state_path, &state)?;
        }
        return Ok(());
    }

    if current_lastmod <= state.last_notmuch_lastmod {
        eprintln!(
            "No notmuch changes since lastmod={} (current lastmod={}).",
            state.last_notmuch_lastmod, current_lastmod
        );
        return Ok(());
    }

    let since = state.last_notmuch_lastmod;

    // Reset the in-progress checkpoint set if it doesn't belong to the
    // current scan window. (Either fresh state, or a previous run
    // completed and advanced `last_notmuch_lastmod` past the old anchor.)
    if state.pushed_in_progress_since != since {
        state.pushed_in_progress.clear();
        state.pushed_in_progress_since = since;
        if !dry_run {
            save_state(&state_path, &state)?;
        }
    }

    let candidates = notmuch_messages_since(since)?;
    let already_done = state.pushed_in_progress.len();
    let to_check: Vec<&String> = candidates
        .iter()
        .filter(|mid| !state.pushed_in_progress.contains(mid.as_str()))
        .collect();
    if already_done > 0 {
        eprintln!(
            "Resuming: {} candidate(s) total, {} already pushed in this window, {} to check (lastmod >{} ≤{}).",
            candidates.len(),
            already_done,
            to_check.len(),
            since,
            current_lastmod
        );
    } else {
        eprintln!(
            "Checking {} candidate message(s) with lastmod >{} ≤{}.",
            candidates.len(),
            since,
            current_lastmod
        );
    }

    let mut pushed = 0usize;
    let mut skipped_unresolved = 0usize;

    // Checkpoint every N pushes — keeps disk write rate bounded while
    // still bounding worst-case re-work to <N messages on SIGTERM.
    const CHECKPOINT_EVERY: usize = 5;
    let mut since_checkpoint = 0usize;

    for mid in to_check {
        let local_tags = notmuch_tags_for(mid)?;
        if local_tags.is_empty() && !has_message_id(mid) {
            continue;
        }

        // Local resolve first: parse Gmail's opaque ID out of the maildir
        // filename via notmuch's known file paths. Falls back to the
        // Gmail-search API only if no Gmail-shaped filename is on disk
        // for this Message-ID (rare: not-yet-mbsynced, non-Gmail account,
        // weird basename). The fallback preserves correctness for the
        // edge cases while the common path stays purely local.
        let gmail_id = match resolve_gmail_id_local(mid)? {
            Some(id) => id,
            None => match resolve_gmail_id(&token, mid)? {
                Some(id) => id,
                None => {
                    skipped_unresolved += 1;
                    continue;
                }
            },
        };

        let gmail_labels = fetch_message_labels(&token, &gmail_id)?;
        let (add, remove) = compute_diff(&local_tags, &gmail_labels, &state.label_name_to_id);

        if add.is_empty() && remove.is_empty() {
            // Nothing to push, but record progress so we don't re-check
            // this candidate next restart (its remote labels already
            // match local tags — the most expensive ops on it were the
            // resolve+fetch_labels Gmail calls, which we want to amortise).
            if !dry_run {
                state.pushed_in_progress.insert(mid.clone());
                since_checkpoint += 1;
                if since_checkpoint >= CHECKPOINT_EVERY {
                    save_state(&state_path, &state)?;
                    since_checkpoint = 0;
                }
            }
            continue;
        }

        if dry_run {
            eprintln!(
                "[dry-run] {mid} → gmail:{gmail_id}  add={add:?}  remove={remove:?}"
            );
        } else {
            modify_message_labels(&token, &gmail_id, &add, &remove)
                .with_context(|| format!("modifying Gmail message {gmail_id}"))?;
            pushed += 1;
            eprintln!("pushed {mid} → gmail:{gmail_id}");
            // Record this push immediately in the in-progress set, then
            // checkpoint to disk every CHECKPOINT_EVERY pushes. If
            // launchd SIGTERMs us before the run completes, the next
            // run skips everything in `pushed_in_progress` and resumes
            // from where we left off (modulo up to CHECKPOINT_EVERY
            // re-pushes — idempotent on Gmail's side anyway).
            state.pushed_in_progress.insert(mid.clone());
            since_checkpoint += 1;
            if since_checkpoint >= CHECKPOINT_EVERY {
                save_state(&state_path, &state)?;
                since_checkpoint = 0;
            }
        }
    }

    if !dry_run {
        // Run completed cleanly: advance the anchor and clear the
        // in-progress set so the next tick starts fresh.
        state.last_notmuch_lastmod = current_lastmod;
        state.pushed_in_progress.clear();
        state.pushed_in_progress_since = current_lastmod;
        save_state(&state_path, &state)?;
        eprintln!(
            "Pushed changes to {pushed} message(s); {skipped_unresolved} unresolved; state advanced to lastmod={current_lastmod}."
        );
    } else {
        eprintln!(
            "[dry-run] would have pushed to some subset of {} candidate(s) (state NOT advanced); {skipped_unresolved} unresolved.",
            candidates.len()
        );
    }

    Ok(())
}

fn state_file_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| anyhow!("HOME not set — cannot locate state file"))?;
    let mut p = PathBuf::from(home);
    p.push(STATE_DIR);
    std::fs::create_dir_all(&p).with_context(|| format!("creating {p:?}"))?;
    p.push(STATE_FILE);
    Ok(p)
}

fn load_state(path: &PathBuf) -> Result<State> {
    if !path.exists() {
        return Ok(State::default());
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {path:?}"))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {path:?} as State JSON"))
}

fn save_state(path: &PathBuf, state: &State) -> Result<()> {
    let text = serde_json::to_string_pretty(state).context("serialising State")?;
    std::fs::write(path, text).with_context(|| format!("writing {path:?}"))
}

fn label_map_stale(state: &State) -> bool {
    if state.label_name_to_id.is_empty() {
        return true;
    }
    chrono::Utc::now().timestamp() - state.label_map_fetched_at > LABEL_MAP_MAX_AGE_SECS
}

fn access_token() -> Result<String> {
    // Tokens are owned by the pizauth daemon. `pizauth show gmail` prints
    // a fresh access token on stdout, auto-refreshing via the stored
    // refresh token if the cached token is near expiry.
    let output = std::process::Command::new("pizauth")
        .args(["show", "gmail"])
        .output()
        .context("running `pizauth show gmail`")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("pizauth show gmail failed: {}", stderr.trim());
    }
    let token = String::from_utf8(output.stdout)
        .context("parsing pizauth output as UTF-8")?
        .trim()
        .to_string();
    if token.is_empty() {
        anyhow::bail!("pizauth show gmail returned empty token — run `pizauth refresh gmail`");
    }
    Ok(token)
}

fn current_notmuch_lastmod() -> Result<u64> {
    let out = std::process::Command::new("notmuch")
        .args(["count", "--lastmod", "*"])
        .output()
        .context("invoking notmuch count --lastmod")?;
    if !out.status.success() {
        return Err(anyhow!(
            "notmuch count --lastmod failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Output shape: "<database-uuid>\t<count>\t<lastmod>\n"
    let last = stdout
        .split_whitespace()
        .last()
        .ok_or_else(|| anyhow!("empty output from notmuch count --lastmod"))?;
    last.parse::<u64>()
        .with_context(|| format!("parsing notmuch lastmod integer from {last:?}"))
}

/// Return a list of notmuch Message-IDs (RFC-5322 `Message-ID:` values,
/// no angle brackets — notmuch's internal form) whose last tag change
/// happened after the given lastmod.
fn notmuch_messages_since(since: u64) -> Result<Vec<String>> {
    let query = format!("lastmod:{}..", since + 1);
    let out = std::process::Command::new("notmuch")
        .args(["search", "--output=messages", "--format=text"])
        .arg(&query)
        .output()
        .context("invoking notmuch search --output=messages")?;
    if !out.status.success() {
        return Err(anyhow!(
            "notmuch search failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let stdout = String::from_utf8(out.stdout).context("decoding notmuch stdout")?;
    // Output is `id:<message-id>` per line.
    Ok(stdout
        .lines()
        .filter_map(|l| l.strip_prefix("id:"))
        .map(|s| s.to_string())
        .collect())
}

/// Return the notmuch tag set for one message.
fn notmuch_tags_for(message_id: &str) -> Result<BTreeSet<String>> {
    let query = format!("id:{message_id}");
    let out = std::process::Command::new("notmuch")
        .args(["search", "--output=tags", "--format=text"])
        .arg(&query)
        .output()
        .context("invoking notmuch search --output=tags")?;
    if !out.status.success() {
        return Err(anyhow!(
            "notmuch search tags failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let stdout = String::from_utf8(out.stdout).context("decoding notmuch stdout")?;
    Ok(stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect())
}

fn has_message_id(mid: &str) -> bool {
    !mid.trim().is_empty()
}

/// Parse a maildir filename and return the Gmail opaque ID if and only
/// if the basename matches lieer/gmail_pull's `<gmail-id>:2,<flags>`
/// shape — pure-hex prefix of 12–20 chars, optionally followed by a
/// `:2,<flags>` maildir-info suffix. Returns None for any other shape
/// (mbsync's `<unix-ts>.<pid>_<n>.<host>,U=<uid>:2,<flags>` for COHS,
/// stray dots, non-hex prefix, etc.). Pulled out for testability —
/// no I/O.
fn parse_gmail_id_from_filename(name: &str) -> Option<String> {
    // Strip the `:2,<flags>` maildir-info suffix if present. The Gmail
    // ID never contains a `:` itself, so splitting on the first `:` is
    // safe — anything to the left of it is the candidate ID.
    let candidate = name.split(':').next().unwrap_or(name);

    // Reject anything that isn't pure lowercase hex of a plausible
    // length. Gmail IDs observed in the wild are 16 hex chars; allow
    // 12–20 to leave headroom for past/future formats while still
    // excluding mbsync-style names which always contain dots, commas,
    // or `=`.
    let len = candidate.len();
    if !(12..=20).contains(&len) {
        return None;
    }
    if !candidate.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    // Be even stricter: require lowercase. Rules out anything created
    // by a different tool that happens to share the length and
    // hex-charset constraints.
    if candidate.bytes().any(|b| b.is_ascii_uppercase()) {
        return None;
    }
    Some(candidate.to_string())
}

/// Resolve a Gmail opaque ID for an RFC Message-ID by inspecting
/// notmuch's known file paths. Pure local I/O, no network. Returns
/// None if no Gmail-shaped filename is found, in which case the caller
/// is expected to fall back to the Gmail search API.
fn resolve_gmail_id_local(rfc_message_id: &str) -> Result<Option<String>> {
    let query = format!("id:{rfc_message_id}");
    let out = std::process::Command::new("notmuch")
        .args(["search", "--output=files", "--format=text"])
        .arg(&query)
        .output()
        .context("invoking notmuch search --output=files")?;
    if !out.status.success() {
        return Err(anyhow!(
            "notmuch search --output=files failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let stdout = String::from_utf8(out.stdout).context("decoding notmuch stdout")?;

    // notmuch can return >1 file path when the same message is
    // delivered to multiple maildirs (Gmail label-as-folder duplication).
    // For Gmail messages, lieer/gmail_pull guarantee the same opaque ID
    // is the basename of every copy — so taking the first match that
    // parses cleanly is correct.
    for line in stdout.lines() {
        let path = line.trim();
        if path.is_empty() {
            continue;
        }
        let basename = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if let Some(id) = parse_gmail_id_from_filename(basename) {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// Look up a Gmail opaque message ID for a given RFC Message-ID via
/// Gmail's search API. Returns None if the message isn't in the
/// account (or hasn't yet been indexed by Gmail server-side).
///
/// Used as a fallback when [`resolve_gmail_id_local`] can't recover
/// the ID from a maildir filename.
fn resolve_gmail_id(token: &str, rfc_message_id: &str) -> Result<Option<String>> {
    #[derive(Deserialize)]
    struct Hit {
        id: String,
    }
    #[derive(Deserialize)]
    struct SearchResp {
        #[serde(default)]
        messages: Vec<Hit>,
    }

    let url = format!(
        "https://gmail.googleapis.com/gmail/v1/users/me/messages?q={}&maxResults=1",
        urlencoding::encode(&format!("rfc822msgid:{rfc_message_id}"))
    );
    let client = http_client()?;
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .context("GET /messages?q=rfc822msgid:")?;

    if !resp.status().is_success() {
        let s = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(anyhow!("Gmail resolve failed: HTTP {s}: {body}"));
    }
    let parsed: SearchResp = resp.json().context("parsing resolve response")?;
    Ok(parsed.messages.into_iter().next().map(|h| h.id))
}

/// Fetch the Gmail label set (label IDs) currently applied to a
/// message.
fn fetch_message_labels(token: &str, gmail_id: &str) -> Result<BTreeSet<String>> {
    #[derive(Deserialize)]
    struct Msg {
        #[serde(rename = "labelIds", default)]
        label_ids: Vec<String>,
    }
    let url = format!(
        "https://gmail.googleapis.com/gmail/v1/users/me/messages/{gmail_id}?format=minimal"
    );
    let client = http_client()?;
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .context("GET /messages/{id}?format=minimal")?;
    if !resp.status().is_success() {
        let s = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(anyhow!("Gmail label fetch failed: HTTP {s}: {body}"));
    }
    let parsed: Msg = resp.json().context("parsing message response")?;
    Ok(parsed.label_ids.into_iter().collect())
}

/// Fetch the account's label list and return a `display-name → ID`
/// map. Both system labels (`INBOX`, `UNREAD`, …) and user labels
/// (`Label_123`, …) are included, keyed by their visible name.
fn fetch_label_map(token: &str) -> Result<BTreeMap<String, String>> {
    #[derive(Deserialize)]
    struct Label {
        id: String,
        name: String,
    }
    #[derive(Deserialize)]
    struct Resp {
        #[serde(default)]
        labels: Vec<Label>,
    }
    let url = "https://gmail.googleapis.com/gmail/v1/users/me/labels";
    let client = http_client()?;
    let resp = client
        .get(url)
        .bearer_auth(token)
        .send()
        .context("GET /labels")?;
    if !resp.status().is_success() {
        let s = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(anyhow!("Gmail label-list failed: HTTP {s}: {body}"));
    }
    let parsed: Resp = resp.json().context("parsing label-list response")?;
    Ok(parsed.labels.into_iter().map(|l| (l.name, l.id)).collect())
}

/// Issue `/messages/{id}/modify` with the computed delta.
fn modify_message_labels(
    token: &str,
    gmail_id: &str,
    add: &[String],
    remove: &[String],
) -> Result<()> {
    #[derive(Serialize)]
    struct Body<'a> {
        #[serde(rename = "addLabelIds", skip_serializing_if = "Vec::is_empty")]
        add: Vec<&'a str>,
        #[serde(rename = "removeLabelIds", skip_serializing_if = "Vec::is_empty")]
        remove: Vec<&'a str>,
    }
    let body = Body {
        add: add.iter().map(|s| s.as_str()).collect(),
        remove: remove.iter().map(|s| s.as_str()).collect(),
    };
    let url = format!("https://gmail.googleapis.com/gmail/v1/users/me/messages/{gmail_id}/modify");
    let client = http_client()?;
    let resp = client
        .post(&url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .context("POST /messages/{id}/modify")?;
    if !resp.status().is_success() {
        let s = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(anyhow!("Gmail modify failed: HTTP {s}: {body}"));
    }
    Ok(())
}

fn http_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("building HTTP client")
}

/// Given local tags + remote label IDs + a name→ID map, return
/// `(add, remove)` — lists of Gmail label IDs that must be applied
/// to make the remote match local intent.
pub fn compute_diff(
    local_tags: &BTreeSet<String>,
    remote_label_ids: &BTreeSet<String>,
    label_name_to_id: &BTreeMap<String, String>,
) -> (Vec<String>, Vec<String>) {
    let local_ids = local_tags_to_label_ids(local_tags, label_name_to_id);
    let mut add: Vec<String> = local_ids
        .iter()
        .filter(|id| !remote_label_ids.contains(*id))
        .cloned()
        .collect();
    // Only propose removals of labels that we would have been able to
    // set in the first place. This keeps Gmail-internal labels
    // (CATEGORY_PROMOTIONS, CHAT, etc.) intact.
    let mut manageable_universe: BTreeSet<String> =
        label_name_to_id.values().cloned().collect();
    for (_, sys_id) in SYSTEM_TAG_TO_LABEL {
        manageable_universe.insert((*sys_id).to_string());
    }
    // Belt-and-braces guard for the same Gmail API constraint that
    // SYSTEM_TAG_TO_LABEL enforces on the *add* side: SENT, DRAFT,
    // OUTBOX and CHAT are auto-derived by Gmail and the modify endpoint
    // rejects them with HTTP 400 "Invalid label" on either side. The
    // add side is already covered by their absence from
    // SYSTEM_TAG_TO_LABEL, but they leak into manageable_universe via
    // `label_name_to_id` (Gmail's `/labels` endpoint returns them with
    // `name == id`, so they show up in the cached map). Without this
    // filter, a Gmail message that has SENT applied server-side but no
    // matching local `sent` tag would propose `removeLabelIds: ["SENT"]`
    // and crash the run, halting backlog drain on that one message.
    const PROTECTED_LABEL_IDS: &[&str] = &["SENT", "DRAFT", "OUTBOX", "CHAT"];
    for protected in PROTECTED_LABEL_IDS {
        manageable_universe.remove(*protected);
    }
    let mut remove: Vec<String> = remote_label_ids
        .iter()
        .filter(|id| !local_ids.contains(*id))
        .filter(|id| manageable_universe.contains(*id))
        .cloned()
        .collect();
    add.sort();
    remove.sort();
    (add, remove)
}

fn local_tags_to_label_ids(
    tags: &BTreeSet<String>,
    label_name_to_id: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let sys: BTreeMap<&str, &str> = SYSTEM_TAG_TO_LABEL.iter().copied().collect();
    for tag in tags {
        if is_local_only_tag(tag.as_str()) {
            continue;
        }
        if let Some(sys_id) = sys.get(tag.as_str()) {
            out.insert(sys_id.to_string());
            continue;
        }
        // User labels: look up by display name.
        if let Some(id) = label_name_to_id.get(tag.as_str()) {
            out.insert(id.clone());
        }
        // Unknown tag (no matching Gmail label) → silently ignored.
        // User can add the tag as a new Gmail label via the web UI;
        // on the next daily label-map refresh it'll round-trip.
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(s: &[&str]) -> BTreeSet<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    fn name_map() -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert("Label_projects".into(), "Label_001".into());
        m.insert("Label_research".into(), "Label_002".into());
        m
    }

    #[test]
    fn system_tag_maps_to_uppercase_label() {
        let tags = ids(&["inbox", "unread"]);
        let result = local_tags_to_label_ids(&tags, &BTreeMap::new());
        assert!(result.contains("INBOX"));
        assert!(result.contains("UNREAD"));
    }

    #[test]
    fn local_only_tags_never_propose() {
        let tags = ids(&["attachment", "signed", "replied"]);
        let result = local_tags_to_label_ids(&tags, &BTreeMap::new());
        assert!(result.is_empty(), "got {result:?}");
    }

    #[test]
    fn curator_seen_tags_never_propose() {
        let tags = ids(&[
            "curator-amazon-shipping-seen",
            "curator-nicabm-seen",
            "curator-uber-trips-seen",
        ]);
        let result = local_tags_to_label_ids(&tags, &BTreeMap::new());
        assert!(result.is_empty(), "got {result:?}");
    }

    #[test]
    fn tags_that_merely_contain_curator_still_propose_if_in_map() {
        // e.g. a user-created Gmail label called `curator-style-notes`
        // that does NOT end in `-seen` should still map if present
        // in the label map — we aren't over-filtering.
        let tags = ids(&["curator-style-notes"]);
        let mut m = BTreeMap::new();
        m.insert("curator-style-notes".to_string(), "Label_999".to_string());
        let result = local_tags_to_label_ids(&tags, &m);
        assert_eq!(result.len(), 1);
        assert!(result.contains("Label_999"));
    }

    #[test]
    fn user_tag_maps_via_name_lookup() {
        let tags = ids(&["Label_projects"]);
        let result = local_tags_to_label_ids(&tags, &name_map());
        assert!(result.contains("Label_001"), "got {result:?}");
    }

    #[test]
    fn diff_adds_missing_remote_labels() {
        let local = ids(&["inbox", "Label_projects"]);
        let remote = ids(&["UNREAD"]);
        let (add, remove) = compute_diff(&local, &remote, &name_map());
        assert!(add.contains(&"INBOX".to_string()));
        assert!(add.contains(&"Label_001".to_string()));
        // UNREAD is removed because it's in the manageable universe
        // (system tag) but not present in local intent.
        assert!(remove.contains(&"UNREAD".to_string()));
    }

    #[test]
    fn diff_preserves_gmail_internal_labels_not_in_local_universe() {
        // CATEGORY_PERSONAL isn't in SYSTEM_TAG_TO_LABEL and isn't in
        // the user label map, so push-tags mustn't volunteer to remove
        // it — that's Gmail classifier territory.
        let local = ids(&["inbox"]);
        let remote = ids(&["INBOX", "CATEGORY_PERSONAL"]);
        let (add, remove) = compute_diff(&local, &remote, &name_map());
        assert!(add.is_empty());
        assert!(!remove.contains(&"CATEGORY_PERSONAL".to_string()));
    }

    #[test]
    fn diff_never_proposes_removing_protected_system_labels() {
        // SENT/DRAFT/OUTBOX/CHAT are Gmail-managed and the modify
        // endpoint rejects modify requests that touch them. They show
        // up in `label_name_to_id` via Gmail's `/labels` endpoint
        // (name == id == "SENT" etc.), so we have to filter them out
        // of the manageable universe explicitly.
        let mut m = BTreeMap::new();
        m.insert("SENT".into(), "SENT".into());
        m.insert("DRAFT".into(), "DRAFT".into());
        m.insert("OUTBOX".into(), "OUTBOX".into());
        m.insert("CHAT".into(), "CHAT".into());
        let local = ids(&[]); // no local tags at all
        let remote = ids(&["SENT", "DRAFT", "OUTBOX", "CHAT"]);
        let (add, remove) = compute_diff(&local, &remote, &m);
        assert!(add.is_empty(), "got add={add:?}");
        assert!(
            remove.is_empty(),
            "must not propose removing protected labels; got {remove:?}"
        );
    }

    #[test]
    fn diff_empty_when_in_sync() {
        let local = ids(&["inbox", "flagged"]);
        let remote = ids(&["INBOX", "STARRED"]);
        let (add, remove) = compute_diff(&local, &remote, &BTreeMap::new());
        assert!(add.is_empty());
        assert!(remove.is_empty());
    }

    #[test]
    fn label_map_is_stale_when_empty() {
        let s = State::default();
        assert!(label_map_stale(&s));
    }

    #[test]
    fn label_map_is_stale_when_old() {
        let mut s = State::default();
        s.label_name_to_id.insert("x".into(), "y".into());
        s.label_map_fetched_at =
            chrono::Utc::now().timestamp() - LABEL_MAP_MAX_AGE_SECS - 100;
        assert!(label_map_stale(&s));
    }

    #[test]
    fn label_map_is_fresh_when_recent() {
        let mut s = State::default();
        s.label_name_to_id.insert("x".into(), "y".into());
        s.label_map_fetched_at = chrono::Utc::now().timestamp() - 100;
        assert!(!label_map_stale(&s));
    }

    // ---- parse_gmail_id_from_filename: local-resolve happy paths ----

    #[test]
    fn parse_gmail_id_from_lieer_filename_with_flags() {
        // The canonical shape lieer/gmail_pull writes for personal
        // Gmail messages: `<16 hex>:2,<flags>`.
        let got = parse_gmail_id_from_filename("19dca8d8c84f0f12:2,S");
        assert_eq!(got.as_deref(), Some("19dca8d8c84f0f12"));
    }

    #[test]
    fn parse_gmail_id_from_lieer_filename_no_flags() {
        // Some maildir tooling writes `<id>:2,` with no trailing flags.
        let got = parse_gmail_id_from_filename("19dca8d8c84f0f12:2,");
        assert_eq!(got.as_deref(), Some("19dca8d8c84f0f12"));
    }

    #[test]
    fn parse_gmail_id_from_bare_basename() {
        // Defensive: even if the maildir-info suffix is absent the
        // pure-hex basename should still parse.
        let got = parse_gmail_id_from_filename("19dca8d8c84f0f12");
        assert_eq!(got.as_deref(), Some("19dca8d8c84f0f12"));
    }

    // ---- parse_gmail_id_from_filename: rejection paths ----

    #[test]
    fn parse_gmail_id_rejects_mbsync_filename() {
        // COHS messages are pulled by mbsync (IMAP) and use a totally
        // different naming convention: dots, underscores, commas, `U=N`.
        // Must NOT be mistaken for a Gmail ID.
        assert_eq!(
            parse_gmail_id_from_filename("1776807569.72695_932.Williams-MacBook-Air,U=932:2,S"),
            None
        );
    }

    #[test]
    fn parse_gmail_id_rejects_non_hex() {
        assert_eq!(parse_gmail_id_from_filename("notahex0123456789:2,S"), None);
    }

    #[test]
    fn parse_gmail_id_rejects_short() {
        assert_eq!(parse_gmail_id_from_filename("deadbeef:2,S"), None);
    }

    #[test]
    fn parse_gmail_id_rejects_long() {
        assert_eq!(
            parse_gmail_id_from_filename("19dca8d8c84f0f1219dca8d8:2,S"),
            None
        );
    }

    #[test]
    fn parse_gmail_id_rejects_uppercase_hex() {
        // Uppercase hex would be a non-Gmail-tooling signature.
        assert_eq!(parse_gmail_id_from_filename("19DCA8D8C84F0F12:2,S"), None);
    }

    #[test]
    fn parse_gmail_id_rejects_empty() {
        assert_eq!(parse_gmail_id_from_filename(""), None);
        assert_eq!(parse_gmail_id_from_filename(":2,S"), None);
    }
}
