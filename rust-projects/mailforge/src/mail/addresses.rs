//! Address book derived from the notmuch index.
//!
//! Powers the compose-form recipient autocomplete (To/Cc/Bcc). The address
//! book is a flat list of [`AddressEntry`] rows — one per unique email
//! address — built once at startup by shelling out to `notmuch address`,
//! cached in a global [`OnceLock<RwLock<...>>`], and refreshed on a
//! background tokio task every 10 minutes.
//!
//! ## Why an in-memory cache
//!
//! Building the book requires two `notmuch address` invocations
//! (recipients of `tag:sent`; senders of everything else). On William's
//! 218k-message corpus this takes 10-15 seconds end to end — far too
//! slow to do inside a request handler. We do an EAGER initial build on
//! startup (in `spawn_refresh_task`) so the first request to
//! `/api/addresses` finds a warm cache; the refresh loop then rebuilds
//! every 10 minutes to pick up newly-corresponded addresses.
//!
//! The two-call shape exploits a notmuch quirk: with
//! `--deduplicate=no`, each row carries both the (latest-seen-on-this-
//! row) display name AND lets us count occurrences per address. So one
//! call yields BOTH the universe and the count — saving a third call
//! against `*` which is prohibitively slow (~2 minutes on a 218k corpus
//! because notmuch has to walk every message to gather senders +
//! recipients).
//!
//! ## Ranking
//!
//! For autocomplete we want "people YOU have emailed" to surface first —
//! they're the addresses you actually compose to. Secondary key is
//! "people who have emailed you" (frequent senders); tertiary is
//! alphabetical for stable output.
//!
//! ## What this module does NOT do
//!
//! - Does not parse mail files or read on-disk corpus directly.
//! - Does not write back to notmuch (read-only).
//! - Does not store contacts that aren't already in the index.
//!
//! See `src/mail/notmuch_db.rs` for the broader notmuch-CLI shape; this
//! module follows the same subprocess + JSON pattern.

use anyhow::{Context, Result};
use axum::{extract::Query, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

/// One entry in the address book. The fields are computed at build time
/// and don't change for the lifetime of a cache snapshot — readers get
/// stable view by cloning out of the [`RwLock`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddressEntry {
    /// Display name as it appeared on the most-recent message we saw,
    /// or `None` if the address always appeared bare (`<a@b>` form).
    pub name: Option<String>,
    /// Lowercased email address. The dedup key.
    pub address: String,
    /// Pre-formatted `Name <addr>` (or just `addr`) for direct insertion
    /// into the To/Cc/Bcc field. Saves clients from re-deriving the
    /// quoting rules.
    pub display: String,
    /// Number of times this address appeared as a recipient of a message
    /// tagged `sent`. Heuristic: addresses *you have written to*.
    pub sent_count: u32,
    /// Number of times this address appeared as the sender of any
    /// message in the index. Heuristic: addresses that have written
    /// to you.
    pub recv_count: u32,
}

// ------------------------------------------------------------------
// JSON shape from `notmuch address --format=json`
// ------------------------------------------------------------------

/// Schema for a single notmuch-address JSON row. notmuch emits
/// `{"name": "...", "address": "...", "name-addr": "..."}` for every
/// (deduplicated) address. We ignore `name-addr` because we re-derive
/// the display string ourselves — notmuch's quoting is fine but we want
/// a single code path so the unit tests cover it.
///
/// Public so unit tests + future callers (e.g. an admin "rebuild now"
/// route) can construct fixtures and pass them to
/// [`build_from_streams`].
#[derive(Debug, Deserialize)]
pub struct NotmuchAddrRow {
    #[serde(default)]
    pub name: String,
    pub address: String,
}

// ------------------------------------------------------------------
// Cache
// ------------------------------------------------------------------

/// Process-global cache. Initialised to an empty Vec on first call to
/// [`get`] or [`spawn_refresh_task`]; the actual address book is loaded
/// in the background by `spawn_refresh_task` and assigned via the
/// `RwLock`.
static CACHE: OnceLock<RwLock<Vec<AddressEntry>>> = OnceLock::new();

/// How often the background refresh task rebuilds the cache. 10 minutes
/// matches the spec's "refresh every 10 min" guidance — long enough
/// that we're not pegging notmuch on a busy machine, short enough that
/// freshly-corresponded addresses appear within one mail-cycle iteration.
const REFRESH_INTERVAL: Duration = Duration::from_secs(600);

/// Read-only access to the cached address book. Returns whatever the
/// cache currently holds — never blocks waiting for a (re)build. If the
/// background task hasn't finished its first rebuild yet, callers see
/// an empty Vec (and the autocomplete dropdown stays empty); the next
/// request after the rebuild completes will see the populated book.
///
/// Failure handling: a poisoned RwLock or a missing OnceLock cell both
/// degrade to an empty Vec — the autocomplete endpoint then returns
/// `{"matches": []}` rather than 500ing the compose page.
pub fn get() -> Vec<AddressEntry> {
    cache()
        .read()
        .map(|v| v.clone())
        .unwrap_or_default()
}

fn cache() -> &'static RwLock<Vec<AddressEntry>> {
    CACHE.get_or_init(|| RwLock::new(Vec::new()))
}

/// Spawn the background refresh task. Should be called once from the
/// daemon's `run` after the runtime is up. Idempotent: subsequent calls
/// after the first will return without spawning a duplicate.
///
/// The task does an EAGER first build (immediately, on the blocking
/// pool) so the cache is warm soon after startup. After each build it
/// sleeps [`REFRESH_INTERVAL`] then rebuilds again. A failed rebuild
/// leaves the previous snapshot in place (we don't want a transient
/// notmuch failure to wipe the book).
pub fn spawn_refresh_task() {
    static SPAWNED: OnceLock<()> = OnceLock::new();
    if SPAWNED.set(()).is_err() {
        return; // already spawned
    }
    // Touch the cache so the OnceLock cell is initialised before any
    // refresh / read tries to write into it.
    let _ = cache();
    tokio::spawn(async {
        loop {
            // notmuch CLI is blocking; run on the blocking pool so we
            // don't tie up an async worker for 10-15s.
            let result = tokio::task::spawn_blocking(build_address_book).await;
            match result {
                Ok(Ok(fresh)) => {
                    if let Ok(mut guard) = cache().write() {
                        let n = fresh.len();
                        *guard = fresh;
                        tracing::info!("address book refreshed: {n} entries");
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!("address book build failed: {e:#}");
                }
                Err(e) => {
                    tracing::warn!("address book build join error: {e}");
                }
            }
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    });
}

// ------------------------------------------------------------------
// Build
// ------------------------------------------------------------------

/// Build the full address book by issuing 2 `notmuch address` calls and
/// merging the results. Public so test harnesses (and any future
/// admin/refresh endpoint) can drive a synchronous rebuild.
///
/// Why only 2 calls — see the module-level docstring. We avoid the
/// `--output=sender --output=recipients '*'` shape (universe in one go)
/// because on a large corpus notmuch needs ~2 minutes to walk every
/// message; the two split queries `tag:sent` and `not tag:sent` are
/// cumulatively ~15 seconds and partition the corpus exactly the same
/// way for the count and universe-derivation purposes we have here.
pub fn build_address_book() -> Result<Vec<AddressEntry>> {
    // 1. Recipients of sent messages, NOT deduplicated. Each row gives
    //    us (a) an address and its display name, (b) one tally toward
    //    sent_count for that address. We dedupe + count in one pass.
    let sent_json = run_notmuch_address(&[
        "--output=recipients",
        "--deduplicate=no",
        "--sort=newest-first",
        "tag:sent",
    ])
    .context("listing sent recipients")?;
    let sent_rows: Vec<NotmuchAddrRow> = serde_json::from_slice(&sent_json)
        .context("parsing notmuch sent-recipients JSON")?;

    // 2. Senders of incoming (non-sent) messages, NOT deduplicated.
    //    Same shape; populates recv_count and the rest of the universe.
    let recv_json = run_notmuch_address(&[
        "--output=sender",
        "--deduplicate=no",
        "--sort=newest-first",
        "not tag:sent",
    ])
    .context("listing incoming senders")?;
    let recv_rows: Vec<NotmuchAddrRow> = serde_json::from_slice(&recv_json)
        .context("parsing notmuch incoming-senders JSON")?;

    Ok(build_from_streams(&sent_rows, &recv_rows))
}

/// Invoke `notmuch address --format=json <args...>` and return raw stdout.
fn run_notmuch_address(extra_args: &[&str]) -> Result<Vec<u8>> {
    let mut cmd = Command::new("notmuch");
    cmd.arg("address").arg("--format=json");
    for a in extra_args {
        cmd.arg(a);
    }
    let output = cmd
        .stderr(Stdio::null())
        .output()
        .context("spawning `notmuch address`")?;
    if !output.status.success() {
        anyhow::bail!(
            "notmuch address failed (exit {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output.stdout)
}

// ------------------------------------------------------------------
// Pure helpers (unit-testable without notmuch)
// ------------------------------------------------------------------

/// Build the address book from two streams of (non-deduplicated)
/// `notmuch address` rows: one for sent recipients, one for incoming
/// senders. Each row contributes one tally to the relevant count, and
/// (the first time an address is seen) seeds the [`AddressEntry`]
/// with a display name — preferring sent-recipients order (which is
/// already newest-first) so the most-recent name we used wins.
///
/// Public for unit-testing; production code goes through
/// [`build_address_book`].
pub fn build_from_streams(
    sent_rows: &[NotmuchAddrRow],
    recv_rows: &[NotmuchAddrRow],
) -> Vec<AddressEntry> {
    let mut by_addr: HashMap<String, AddressEntry> = HashMap::new();

    // Helper: insert/update an entry based on a notmuch row. Bumps the
    // chosen counter; sets name on first sight only (so the freshly-
    // seen, in-newest-first-order name sticks).
    fn upsert(
        by_addr: &mut HashMap<String, AddressEntry>,
        row: &NotmuchAddrRow,
        bump_sent: bool,
    ) {
        let addr_lc = row.address.to_lowercase();
        if addr_lc.is_empty() {
            return;
        }
        let name_opt = if row.name.trim().is_empty() {
            None
        } else {
            Some(row.name.trim().to_string())
        };
        let entry = by_addr
            .entry(addr_lc.clone())
            .or_insert_with(|| AddressEntry {
                name: name_opt.clone(),
                address: addr_lc.clone(),
                // display filled in once we've finalised the entry
                display: String::new(),
                sent_count: 0,
                recv_count: 0,
            });
        // Late name promotion: a row without a name shouldn't overwrite
        // an earlier seen name. A row with a name fills in if absent.
        if entry.name.is_none() && name_opt.is_some() {
            entry.name = name_opt;
        }
        if bump_sent {
            entry.sent_count = entry.sent_count.saturating_add(1);
        } else {
            entry.recv_count = entry.recv_count.saturating_add(1);
        }
    }

    for row in sent_rows {
        upsert(&mut by_addr, row, true);
    }
    for row in recv_rows {
        upsert(&mut by_addr, row, false);
    }

    let mut entries: Vec<AddressEntry> = by_addr
        .into_values()
        .map(|mut e| {
            e.display = format_display(e.name.as_deref(), &e.address);
            e
        })
        .collect();
    rank(&mut entries);
    entries
}

/// Format `Name <address>` with quoting where necessary. RFC 5322 names
/// containing `,`, `;`, `<`, `>`, `"`, `\`, `(`, `)`, `:`, `@`, or `.`
/// must be wrapped in double quotes (with embedded `"` and `\` escaped).
/// Bare addresses get no name prefix.
pub fn format_display(name: Option<&str>, address: &str) -> String {
    match name {
        Some(n) if !n.is_empty() => {
            if needs_quoting(n) {
                let escaped = n.replace('\\', "\\\\").replace('"', "\\\"");
                format!("\"{escaped}\" <{address}>")
            } else {
                format!("{n} <{address}>")
            }
        }
        _ => address.to_string(),
    }
}

/// Whether an RFC 5322 display name requires double-quoting. The set of
/// "specials" is from the standard plus a few practical additions
/// (`.` is technically allowed unquoted in the form `John.Smith` but
/// quoting it is harmless and avoids edge cases).
fn needs_quoting(name: &str) -> bool {
    name.chars().any(|c| {
        matches!(c, ',' | ';' | '<' | '>' | '"' | '\\' | '(' | ')' | '[' | ']' | ':' | '@')
    }) || name.starts_with(' ')
        || name.ends_with(' ')
}

/// Sort entries by the ranking contract: sent_count desc, recv_count
/// desc, then display ascending (case-insensitive). Mutates in place.
fn rank(entries: &mut [AddressEntry]) {
    entries.sort_by(|a, b| {
        b.sent_count
            .cmp(&a.sent_count)
            .then(b.recv_count.cmp(&a.recv_count))
            .then_with(|| {
                a.display
                    .to_lowercase()
                    .cmp(&b.display.to_lowercase())
            })
    });
}

// ------------------------------------------------------------------
// Search (used by the endpoint)
// ------------------------------------------------------------------

/// Filter the cached address book by case-insensitive substring match
/// against name OR address. Empty `q` returns the top-N by ranking.
/// Results are ordered by the same key as the cache (already ranked).
pub fn search(q: &str, limit: usize) -> Vec<AddressEntry> {
    let book = get();
    search_in(&book, q, limit)
}

/// Inner search — separated so unit tests can drive a fixed input.
pub fn search_in(book: &[AddressEntry], q: &str, limit: usize) -> Vec<AddressEntry> {
    let q_lc = q.trim().to_lowercase();
    let iter = book.iter().filter(|e| {
        if q_lc.is_empty() {
            true
        } else {
            e.address.contains(&q_lc)
                || e.name
                    .as_deref()
                    .map(|n| n.to_lowercase().contains(&q_lc))
                    .unwrap_or(false)
        }
    });
    iter.take(limit).cloned().collect()
}

// ------------------------------------------------------------------
// HTTP endpoint
// ------------------------------------------------------------------

/// Query string for `GET /api/addresses`. `q` is the user's typed
/// fragment; `limit` defaults to 10 (the spec's recommended dropdown
/// size). Both are optional — an empty `q` returns the top-N by rank,
/// useful for "show me my most-frequently-emailed contacts" smoke
/// testing.
#[derive(Debug, Default, Deserialize)]
pub struct AddressesQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Response shape: `{"matches": [...]}`. Single-key wrapper so we can
/// extend with metadata (cache age, total-matches-before-limit) without
/// breaking the client schema.
#[derive(Debug, Serialize)]
pub struct AddressesResponse {
    pub matches: Vec<AddressEntry>,
}

/// GET `/api/addresses?q=<prefix>&limit=10`.
///
/// Reads from the cached address book — never invokes notmuch
/// synchronously. If the cache hasn't been built yet, [`get`] blocks
/// long enough for the first call (5-15s on a 218k-message corpus) but
/// every subsequent caller hits the warm cache (<1ms).
pub async fn addresses_get(Query(q): Query<AddressesQuery>) -> impl IntoResponse {
    let needle = q.q.unwrap_or_default();
    let limit = q.limit.unwrap_or(10).clamp(1, 100);
    let matches = tokio::task::spawn_blocking(move || search(&needle, limit))
        .await
        .unwrap_or_default();
    Json(AddressesResponse { matches })
}

// ------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_notmuch_address_json() {
        // Real-shape fixture from `notmuch address --format=json`.
        // Confirms our struct shape matches notmuch's JSON layout, with
        // the awkward cases the spec called out: empty-name rows,
        // mixed-case addresses, quoted display name with embedded comma.
        let rows: Vec<NotmuchAddrRow> = serde_json::from_str(
            r#"[
              {"name": "Alice Adams", "address": "alice@example.com", "name-addr": "Alice Adams <alice@example.com>"},
              {"name": "", "address": "newsletter@example.org", "name-addr": "newsletter@example.org"},
              {"name": "Bob Brown", "address": "Bob.Brown@Example.NET", "name-addr": "Bob Brown <Bob.Brown@Example.NET>"},
              {"name": "Maras, Katie", "address": "katie@city.ac.uk", "name-addr": "\"Maras, Katie\" <katie@city.ac.uk>"}
            ]"#,
        )
        .unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].name, "Alice Adams");
        assert_eq!(rows[0].address, "alice@example.com");
        // Empty name comes through as empty string (not None) from notmuch.
        assert_eq!(rows[1].name, "");
        assert_eq!(rows[1].address, "newsletter@example.org");
        // Quoted display name with comma is preserved verbatim — the
        // quoting is a notmuch artifact, the parsed `name` field is
        // the inner string.
        assert_eq!(rows[3].name, "Maras, Katie");
    }

    #[test]
    fn build_from_streams_lowercases_addresses() {
        // Single recv row mentions Bob Brown with mixed-case address;
        // book should normalise to lowercase.
        let recv: Vec<NotmuchAddrRow> = serde_json::from_str(
            r#"[{"name": "Bob Brown", "address": "Bob.Brown@Example.NET", "name-addr": ""}]"#,
        )
        .unwrap();
        let book = build_from_streams(&[], &recv);
        let bob = book.iter().find(|e| e.name.as_deref() == Some("Bob Brown")).unwrap();
        assert_eq!(bob.address, "bob.brown@example.net");
    }

    #[test]
    fn build_from_streams_skips_empty_addresses() {
        // notmuch occasionally emits rows with an empty `address` field
        // (degenerate List-Unsubscribe etc.). They should never appear
        // in the book.
        let recv: Vec<NotmuchAddrRow> = serde_json::from_str(
            r#"[{"name": "Anon", "address": "", "name-addr": ""},
                {"name": "Real", "address": "real@example.com", "name-addr": ""}]"#,
        )
        .unwrap();
        let book = build_from_streams(&[], &recv);
        assert_eq!(book.len(), 1);
        assert_eq!(book[0].address, "real@example.com");
    }

    #[test]
    fn build_from_streams_dedupes_case_insensitively() {
        // Two rows for the same address (different casing). Both should
        // collapse into a single entry; recv_count should be 2.
        let recv: Vec<NotmuchAddrRow> = serde_json::from_str(
            r#"[{"name": "First", "address": "x@y.com", "name-addr": ""},
                {"name": "Second", "address": "X@Y.COM", "name-addr": ""}]"#,
        )
        .unwrap();
        let book = build_from_streams(&[], &recv);
        assert_eq!(book.len(), 1);
        assert_eq!(book[0].address, "x@y.com");
        assert_eq!(book[0].recv_count, 2);
        // First row's name wins (sets entry on insert).
        assert_eq!(book[0].name.as_deref(), Some("First"));
    }

    #[test]
    fn build_from_streams_attaches_counts() {
        // alice gets 3 sent + 2 recv; newsletter gets 0 sent + 4 recv.
        let sent: Vec<NotmuchAddrRow> = serde_json::from_str(
            r#"[{"name": "Alice", "address": "alice@example.com", "name-addr": ""},
                {"name": "Alice", "address": "alice@example.com", "name-addr": ""},
                {"name": "Alice", "address": "alice@example.com", "name-addr": ""}]"#,
        )
        .unwrap();
        let recv: Vec<NotmuchAddrRow> = serde_json::from_str(
            r#"[{"name": "", "address": "alice@example.com", "name-addr": ""},
                {"name": "", "address": "alice@example.com", "name-addr": ""},
                {"name": "", "address": "newsletter@example.org", "name-addr": ""},
                {"name": "", "address": "newsletter@example.org", "name-addr": ""},
                {"name": "", "address": "newsletter@example.org", "name-addr": ""},
                {"name": "", "address": "newsletter@example.org", "name-addr": ""}]"#,
        )
        .unwrap();
        let book = build_from_streams(&sent, &recv);
        let alice = book.iter().find(|e| e.address == "alice@example.com").unwrap();
        assert_eq!(alice.sent_count, 3);
        assert_eq!(alice.recv_count, 2);
        let news = book.iter().find(|e| e.address == "newsletter@example.org").unwrap();
        assert_eq!(news.sent_count, 0);
        assert_eq!(news.recv_count, 4);
        // Newsletter had empty names in every row → name remains None.
        assert!(news.name.is_none());
    }

    #[test]
    fn build_from_streams_promotes_late_name() {
        // First sighting has empty name, second has a name. The name
        // should fill in (we don't want to lock in "no name" forever).
        let recv: Vec<NotmuchAddrRow> = serde_json::from_str(
            r#"[{"name": "", "address": "x@y.com", "name-addr": ""},
                {"name": "Eventually Named", "address": "x@y.com", "name-addr": ""}]"#,
        )
        .unwrap();
        let book = build_from_streams(&[], &recv);
        assert_eq!(book.len(), 1);
        assert_eq!(book[0].name.as_deref(), Some("Eventually Named"));
        assert_eq!(book[0].recv_count, 2);
    }

    /// Helper for the ranking test — build a row vec from
    /// (name, address, count) tuples.
    fn rows(spec: &[(&str, &str, usize)]) -> Vec<NotmuchAddrRow> {
        let mut out = Vec::new();
        for &(name, addr, n) in spec {
            for _ in 0..n {
                out.push(NotmuchAddrRow {
                    name: name.to_string(),
                    address: addr.to_string(),
                });
            }
        }
        out
    }

    #[test]
    fn ranking_orders_by_sent_then_recv_then_display() {
        // sent: alice=5, charlie=5; recv: bob=10, dave=1.
        // Expect order: alice (sent=5, alphabetical), charlie (sent=5),
        //               bob (sent=0 / recv=10), dave (sent=0 / recv=1).
        let sent = rows(&[("Alice", "alice@x.com", 5), ("Charlie", "charlie@x.com", 5)]);
        let recv = rows(&[("Bob", "bob@x.com", 10), ("Dave", "dave@x.com", 1)]);

        let book = build_from_streams(&sent, &recv);
        assert_eq!(book[0].address, "alice@x.com");
        assert_eq!(book[1].address, "charlie@x.com");
        assert_eq!(book[2].address, "bob@x.com");
        assert_eq!(book[3].address, "dave@x.com");
    }

    #[test]
    fn format_display_bare_address_when_no_name() {
        assert_eq!(format_display(None, "a@b.com"), "a@b.com");
        assert_eq!(format_display(Some(""), "a@b.com"), "a@b.com");
    }

    #[test]
    fn format_display_quotes_name_with_comma() {
        // RFC 5322 specials trigger quoting.
        assert_eq!(
            format_display(Some("Maras, Katie"), "katie@city.ac.uk"),
            "\"Maras, Katie\" <katie@city.ac.uk>"
        );
    }

    #[test]
    fn format_display_escapes_embedded_quotes() {
        // Pathological but legal: a name with literal double-quote.
        assert_eq!(
            format_display(Some(r#"Bob "The Builder""#), "bob@x.com"),
            r#""Bob \"The Builder\"" <bob@x.com>"#
        );
    }

    #[test]
    fn format_display_no_quoting_for_plain_name() {
        assert_eq!(
            format_display(Some("Alice Adams"), "alice@example.com"),
            "Alice Adams <alice@example.com>"
        );
    }

    #[test]
    fn format_display_quotes_when_name_has_angle_brackets() {
        // Defensive: a literal `<` in the display name would otherwise
        // collide with the `<addr>` syntax.
        assert_eq!(
            format_display(Some("a<b"), "x@y.com"),
            "\"a<b\" <x@y.com>"
        );
    }

    #[test]
    fn search_substring_matches_name_or_address() {
        let mut book = vec![
            AddressEntry {
                name: Some("Alice Adams".into()),
                address: "alice@example.com".into(),
                display: "Alice Adams <alice@example.com>".into(),
                sent_count: 10,
                recv_count: 0,
            },
            AddressEntry {
                name: Some("Bob Brown".into()),
                address: "bob@elsewhere.org".into(),
                display: "Bob Brown <bob@elsewhere.org>".into(),
                sent_count: 5,
                recv_count: 0,
            },
            AddressEntry {
                name: None,
                address: "alerts@example.com".into(),
                display: "alerts@example.com".into(),
                sent_count: 0,
                recv_count: 1,
            },
        ];
        rank(&mut book);

        // Match by name fragment.
        let r = search_in(&book, "alice", 10);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].address, "alice@example.com");

        // Match by address fragment, case-insensitive.
        let r = search_in(&book, "EXAMPLE", 10);
        assert_eq!(r.len(), 2); // alice + alerts
        // sent_count desc puts alice first.
        assert_eq!(r[0].address, "alice@example.com");
        assert_eq!(r[1].address, "alerts@example.com");
    }

    #[test]
    fn search_empty_query_returns_top_n_by_rank() {
        let mut book = vec![
            AddressEntry {
                name: Some("Low".into()),
                address: "low@x.com".into(),
                display: "Low <low@x.com>".into(),
                sent_count: 1,
                recv_count: 0,
            },
            AddressEntry {
                name: Some("High".into()),
                address: "high@x.com".into(),
                display: "High <high@x.com>".into(),
                sent_count: 100,
                recv_count: 0,
            },
        ];
        rank(&mut book);
        let r = search_in(&book, "", 10);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].address, "high@x.com");
    }

    #[test]
    fn search_respects_limit() {
        let mut book: Vec<AddressEntry> = (0..20)
            .map(|i| AddressEntry {
                name: Some(format!("Person {i}")),
                address: format!("p{i}@x.com"),
                display: format!("Person {i} <p{i}@x.com>"),
                sent_count: 100 - i, // descending so order is stable
                recv_count: 0,
            })
            .collect();
        rank(&mut book);
        let r = search_in(&book, "person", 5);
        assert_eq!(r.len(), 5);
        assert_eq!(r[0].address, "p0@x.com");
    }

    #[test]
    fn build_handles_quoted_display_name_with_comma() {
        // Make sure the "Maras, Katie" type rows survive the
        // build pipeline and produce a properly-quoted display.
        let recv: Vec<NotmuchAddrRow> = serde_json::from_str(
            r#"[{"name": "Maras, Katie", "address": "katie@city.ac.uk", "name-addr": ""}]"#,
        )
        .unwrap();
        let book = build_from_streams(&[], &recv);
        assert_eq!(book.len(), 1);
        assert_eq!(book[0].display, "\"Maras, Katie\" <katie@city.ac.uk>");
    }
}
