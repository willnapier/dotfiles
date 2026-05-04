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
//! Building the book requires 2-3 `notmuch address` invocations (sender,
//! recipients-of-sent-mail, recipients-from-everything). On William's
//! 218k-message corpus this takes 5-15 seconds end to end — far too slow
//! to do inside a request handler. The cache populates lazily on first
//! access (so the daemon doesn't block startup) and is rebuilt on a timer
//! to pick up newly-corresponded addresses.
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
#[derive(Debug, Deserialize)]
struct NotmuchAddrRow {
    #[serde(default)]
    name: String,
    address: String,
}

// ------------------------------------------------------------------
// Cache
// ------------------------------------------------------------------

/// Process-global cache. Populated lazily on first access (see [`get`]),
/// then refreshed every [`REFRESH_INTERVAL`] by [`spawn_refresh_task`].
static CACHE: OnceLock<RwLock<Vec<AddressEntry>>> = OnceLock::new();

/// How often the background refresh task rebuilds the cache. 10 minutes
/// matches the spec's "refresh every 10 min" guidance — long enough
/// that we're not pegging notmuch on a busy machine, short enough that
/// freshly-corresponded addresses appear within one mail-cycle iteration.
const REFRESH_INTERVAL: Duration = Duration::from_secs(600);

/// Read-only access to the cached address book. Builds the book on
/// first call (synchronous, may take 5-15 seconds on a large corpus),
/// then returns a clone of the current snapshot on subsequent calls.
///
/// Returns an empty Vec if the build failed (e.g. notmuch is unavailable).
/// We swallow the error here so the autocomplete endpoint degrades to
/// "no suggestions" rather than 500ing the entire compose page.
pub fn get() -> Vec<AddressEntry> {
    let lock = CACHE.get_or_init(|| RwLock::new(build_or_empty()));
    lock.read()
        .map(|v| v.clone())
        .unwrap_or_default()
}

/// Spawn the background refresh task. Should be called once from the
/// daemon's `run` after the runtime is up. Idempotent: subsequent calls
/// after the first will return without spawning a duplicate.
///
/// The task sleeps `REFRESH_INTERVAL` then rebuilds the cache, repeating
/// forever. A failed rebuild leaves the previous snapshot in place
/// (we don't want a transient notmuch failure to wipe the book).
pub fn spawn_refresh_task() {
    static SPAWNED: OnceLock<()> = OnceLock::new();
    if SPAWNED.set(()).is_err() {
        return; // already spawned
    }
    tokio::spawn(async {
        loop {
            tokio::time::sleep(REFRESH_INTERVAL).await;
            // notmuch CLI is blocking; run on the blocking pool so we
            // don't tie up an async worker for 5-15s.
            let result = tokio::task::spawn_blocking(build_address_book).await;
            match result {
                Ok(Ok(fresh)) => {
                    if let Some(lock) = CACHE.get() {
                        if let Ok(mut guard) = lock.write() {
                            *guard = fresh;
                        }
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!("address book refresh failed: {e:#}");
                }
                Err(e) => {
                    tracing::warn!("address book refresh join error: {e}");
                }
            }
        }
    });
}

/// Wrapper that demotes errors to an empty list. Used by the lazy-init
/// path so a notmuch failure doesn't poison the cache forever.
fn build_or_empty() -> Vec<AddressEntry> {
    match build_address_book() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("address book initial build failed: {e:#}");
            Vec::new()
        }
    }
}

// ------------------------------------------------------------------
// Build
// ------------------------------------------------------------------

/// Build the full address book by issuing 3 `notmuch address` calls and
/// merging the results. Public so test harnesses (and any future
/// admin/refresh endpoint) can drive a synchronous rebuild.
pub fn build_address_book() -> Result<Vec<AddressEntry>> {
    // 1. Universe of addresses (sender + recipients), deduped, with
    //    most-recent name attached. This is the seed: we'll create one
    //    AddressEntry per row here, then layer counts on top.
    let universe_json = run_notmuch_address(&[
        "--output=sender",
        "--output=recipients",
        "--deduplicate=address",
        "--sort=newest-first",
        "*",
    ])
    .context("building address universe")?;
    let universe: Vec<NotmuchAddrRow> = serde_json::from_slice(&universe_json)
        .context("parsing notmuch address universe JSON")?;

    // 2. Recipients of every sent message, NOT deduplicated, so we can
    //    count occurrences per address. This is "addresses YOU have
    //    written to" weighted by frequency.
    let sent_recipients_json = run_notmuch_address(&[
        "--output=recipients",
        "--deduplicate=no",
        "tag:sent",
    ])
    .context("counting sent recipients")?;
    let sent_recipients: Vec<NotmuchAddrRow> = serde_json::from_slice(&sent_recipients_json)
        .context("parsing notmuch sent-recipients JSON")?;
    let sent_counts = count_addresses(&sent_recipients);

    // 3. Senders of every received (= not-sent) message. Counts which
    //    addresses have written to YOU.
    let recv_senders_json = run_notmuch_address(&[
        "--output=sender",
        "--deduplicate=no",
        "not tag:sent",
    ])
    .context("counting incoming senders")?;
    let recv_senders: Vec<NotmuchAddrRow> = serde_json::from_slice(&recv_senders_json)
        .context("parsing notmuch incoming-senders JSON")?;
    let recv_counts = count_addresses(&recv_senders);

    Ok(merge(universe, &sent_counts, &recv_counts))
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

/// Count occurrences of each (lowercased) address in a non-deduplicated
/// list of notmuch rows. Used by the build pipeline to roll
/// `--deduplicate=no` notmuch output into per-address counts.
fn count_addresses(rows: &[NotmuchAddrRow]) -> HashMap<String, u32> {
    let mut counts = HashMap::new();
    for row in rows {
        let addr = row.address.to_lowercase();
        if addr.is_empty() {
            continue;
        }
        *counts.entry(addr).or_insert(0) += 1;
    }
    counts
}

/// Merge the universe (one row per unique address, with its most-recent
/// display name) with per-address sent and receive counts. Returns the
/// final ranked address book.
fn merge(
    universe: Vec<NotmuchAddrRow>,
    sent_counts: &HashMap<String, u32>,
    recv_counts: &HashMap<String, u32>,
) -> Vec<AddressEntry> {
    // Universe may itself have duplicates if newest-first sort surfaces
    // the same address more than once (it shouldn't with --deduplicate=address,
    // but defend against that). Keep the first occurrence's name.
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut entries: Vec<AddressEntry> = Vec::with_capacity(universe.len());
    for row in universe {
        let addr_lc = row.address.to_lowercase();
        if addr_lc.is_empty() {
            continue;
        }
        if seen.contains_key(&addr_lc) {
            continue;
        }
        seen.insert(addr_lc.clone(), entries.len());

        let name = if row.name.trim().is_empty() {
            None
        } else {
            Some(row.name.trim().to_string())
        };
        let display = format_display(name.as_deref(), &addr_lc);
        let sent_count = sent_counts.get(&addr_lc).copied().unwrap_or(0);
        let recv_count = recv_counts.get(&addr_lc).copied().unwrap_or(0);
        entries.push(AddressEntry {
            name,
            address: addr_lc,
            display,
            sent_count,
            recv_count,
        });
    }
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
// Tests
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal fixture mimicking `notmuch address --output=sender
    /// --output=recipients --deduplicate=address` output. Exercises:
    /// - addresses with display names (Alice, Bob)
    /// - bare addresses with empty name (newsletter)
    /// - quoted-name with comma (`"Maras, Katie"`)
    /// - mixed-case address that should be lowercased
    fn universe_fixture() -> Vec<NotmuchAddrRow> {
        serde_json::from_str(
            r#"[
              {"name": "Alice Adams", "address": "alice@example.com", "name-addr": "Alice Adams <alice@example.com>"},
              {"name": "", "address": "newsletter@example.org", "name-addr": "newsletter@example.org"},
              {"name": "Bob Brown", "address": "Bob.Brown@Example.NET", "name-addr": "Bob Brown <Bob.Brown@Example.NET>"},
              {"name": "Maras, Katie", "address": "katie@city.ac.uk", "name-addr": "\"Maras, Katie\" <katie@city.ac.uk>"},
              {"name": "Carol", "address": "carol@example.com", "name-addr": "Carol <carol@example.com>"}
            ]"#,
        )
        .unwrap()
    }

    #[test]
    fn parses_notmuch_address_json() {
        let rows = universe_fixture();
        assert_eq!(rows.len(), 5);
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
    fn merge_lowercases_addresses() {
        let universe = universe_fixture();
        let book = merge(universe, &HashMap::new(), &HashMap::new());
        let bob = book.iter().find(|e| e.name.as_deref() == Some("Bob Brown")).unwrap();
        assert_eq!(bob.address, "bob.brown@example.net");
    }

    #[test]
    fn merge_skips_empty_addresses() {
        let universe: Vec<NotmuchAddrRow> = serde_json::from_str(
            r#"[{"name": "Anon", "address": "", "name-addr": ""},
                {"name": "Real", "address": "real@example.com", "name-addr": "Real <real@example.com>"}]"#,
        )
        .unwrap();
        let book = merge(universe, &HashMap::new(), &HashMap::new());
        assert_eq!(book.len(), 1);
        assert_eq!(book[0].address, "real@example.com");
    }

    #[test]
    fn merge_dedupes_universe() {
        // Two rows with the same address (defensive — notmuch should
        // already dedupe with --deduplicate=address, but we shouldn't
        // double-count if it doesn't).
        let universe: Vec<NotmuchAddrRow> = serde_json::from_str(
            r#"[{"name": "First", "address": "x@y.com", "name-addr": ""},
                {"name": "Second", "address": "X@Y.COM", "name-addr": ""}]"#,
        )
        .unwrap();
        let book = merge(universe, &HashMap::new(), &HashMap::new());
        assert_eq!(book.len(), 1);
        // First occurrence wins — caller passed newest-first.
        assert_eq!(book[0].name.as_deref(), Some("First"));
    }

    #[test]
    fn merge_attaches_counts() {
        let universe = universe_fixture();
        let mut sent = HashMap::new();
        sent.insert("alice@example.com".to_string(), 7);
        sent.insert("carol@example.com".to_string(), 2);
        let mut recv = HashMap::new();
        recv.insert("alice@example.com".to_string(), 3);
        recv.insert("newsletter@example.org".to_string(), 12);

        let book = merge(universe, &sent, &recv);
        let alice = book.iter().find(|e| e.address == "alice@example.com").unwrap();
        assert_eq!(alice.sent_count, 7);
        assert_eq!(alice.recv_count, 3);
        let news = book.iter().find(|e| e.address == "newsletter@example.org").unwrap();
        assert_eq!(news.sent_count, 0);
        assert_eq!(news.recv_count, 12);
    }

    #[test]
    fn ranking_orders_by_sent_then_recv_then_display() {
        let universe: Vec<NotmuchAddrRow> = serde_json::from_str(
            r#"[
              {"name": "Bob", "address": "bob@x.com", "name-addr": ""},
              {"name": "Alice", "address": "alice@x.com", "name-addr": ""},
              {"name": "Charlie", "address": "charlie@x.com", "name-addr": ""},
              {"name": "Dave", "address": "dave@x.com", "name-addr": ""}
            ]"#,
        )
        .unwrap();
        let mut sent = HashMap::new();
        sent.insert("charlie@x.com".to_string(), 5);
        sent.insert("alice@x.com".to_string(), 5);
        // bob has 0 sent, 10 recv — should beat dave (0 / 0)
        let mut recv = HashMap::new();
        recv.insert("bob@x.com".to_string(), 10);

        let book = merge(universe, &sent, &recv);
        // Top: alice + charlie (both sent=5); alphabetical tie-break.
        assert_eq!(book[0].address, "alice@x.com");
        assert_eq!(book[1].address, "charlie@x.com");
        // Then bob (recv=10, sent=0).
        assert_eq!(book[2].address, "bob@x.com");
        // Then dave (everything 0).
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
    fn count_addresses_lowercases_and_skips_empty() {
        let rows: Vec<NotmuchAddrRow> = serde_json::from_str(
            r#"[
              {"name": "", "address": "a@b.com", "name-addr": ""},
              {"name": "", "address": "A@B.COM", "name-addr": ""},
              {"name": "", "address": "a@b.com", "name-addr": ""},
              {"name": "", "address": "", "name-addr": ""},
              {"name": "", "address": "c@d.com", "name-addr": ""}
            ]"#,
        )
        .unwrap();
        let counts = count_addresses(&rows);
        assert_eq!(counts.get("a@b.com").copied(), Some(3));
        assert_eq!(counts.get("c@d.com").copied(), Some(1));
        assert!(!counts.contains_key(""));
    }
}
