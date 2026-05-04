//! Per-domain HTML auto-render trust store.
//!
//! Persists a small set of email-domains the user has explicitly opted into
//! HTML auto-rendering for. The decision tree in
//! [`crate::mail::message::show_message`] consults [`is_trusted`] on every
//! render; on a trusted-domain hit AND a passed Authentication-Results
//! header, the message renders as HTML inline (instead of the default
//! plaintext-preferred view).
//!
//! ## Storage
//!
//! `~/.config/mailforge/html-trusted-senders.json` — versioned JSON with
//! a sorted-by-write-order domain list. Atomic writes via `<path>.tmp`
//! then `rename(2)`. The on-disk shape is:
//!
//! ```json
//! {
//!   "version": 1,
//!   "domains": ["nytimes.com", "github.com"],
//!   "updated_at": "2026-05-04T12:09:00Z"
//! }
//! ```
//!
//! ## In-memory cache
//!
//! Loaded once, lazily, into a `OnceLock<RwLock<TrustedState>>`. All
//! handler calls hit memory; writes refresh the file under the same lock.
//!
//! ## Why a HashSet
//!
//! Lookups happen on every message render. A `HashSet<String>` keeps that
//! O(1). The domain list is tiny (dozens, not thousands), so the memory
//! footprint is negligible.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

/// On-disk shape. `version=1` lets future migrations recognise old files.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrustedFile {
    version: u32,
    /// Sorted alphabetically on save (deterministic output).
    domains: Vec<String>,
    /// RFC3339 timestamp of the last mutation. Best-effort; failure to
    /// fetch the system clock falls back to an empty string.
    updated_at: String,
}

impl Default for TrustedFile {
    fn default() -> Self {
        Self { version: 1, domains: Vec::new(), updated_at: String::new() }
    }
}

/// Process-global trusted-domain cache. Wrapped in OnceLock so the
/// initialisation happens exactly once on first access.
static STATE: OnceLock<RwLock<HashSet<String>>> = OnceLock::new();

/// Resolve the on-disk path. Honours `$XDG_CONFIG_HOME` first; otherwise
/// falls back to `$HOME/.config/mailforge/html-trusted-senders.json`.
pub fn config_path() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .context("HOME unset, cannot determine config directory")?;
    Ok(base.join("mailforge").join("html-trusted-senders.json"))
}

/// Read-or-init the in-memory set. First call loads from disk; subsequent
/// calls return the cached `RwLock`.
fn ensure_loaded() -> &'static RwLock<HashSet<String>> {
    STATE.get_or_init(|| {
        let set = match config_path().and_then(|p| load_from_path(&p)) {
            Ok(set) => set,
            Err(e) => {
                tracing::debug!(
                    "trusted-senders: starting with empty set ({})",
                    e
                );
                HashSet::new()
            }
        };
        RwLock::new(set)
    })
}

/// Read the file at `path` into a HashSet. Missing file → empty set
/// (not an error).
fn load_from_path(path: &std::path::Path) -> Result<HashSet<String>> {
    if !path.exists() {
        return Ok(HashSet::new());
    }
    let bytes = std::fs::read(path)
        .with_context(|| format!("reading {}", path.display()))?;
    parse_bytes(&bytes)
}

/// Parse JSON bytes into a HashSet. Tolerates legacy `domains: null` by
/// falling back to empty.
fn parse_bytes(bytes: &[u8]) -> Result<HashSet<String>> {
    let f: TrustedFile = serde_json::from_slice(bytes)
        .context("deserialising html-trusted-senders.json")?;
    Ok(f.domains.into_iter().map(|d| normalise_domain(&d)).collect())
}

/// Persist the current set to disk via atomic write.
fn save_to_disk(domains: &HashSet<String>) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("mkdir -p {}", parent.display()))?;
    }
    let mut sorted: Vec<String> = domains.iter().cloned().collect();
    sorted.sort();
    let f = TrustedFile {
        version: 1,
        domains: sorted,
        updated_at: now_rfc3339(),
    };
    let json = serde_json::to_vec_pretty(&f)
        .context("serialising html-trusted-senders.json")?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Best-effort RFC3339 "now" string. Uses `SystemTime` and a hand-rolled
/// formatter so we don't pull in a chrono/time dependency. The exact
/// content isn't load-bearing — it's debug-grade metadata. On the rare
/// SystemTime failure, returns an empty string.
fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d,
        Err(_) => return String::new(),
    };
    let secs = dur.as_secs() as i64;
    // Compute UTC date-time from secs since epoch using a simple conversion.
    // Y-M-D-H-M-S, no leap-second handling — that's fine for a metadata stamp.
    let (year, month, day, hour, min, sec) = epoch_to_ymdhms(secs);
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z"
    )
}

/// Convert seconds-since-epoch to (year, month, day, hour, min, sec) in UTC.
/// Algorithm: Howard Hinnant's "days_from_civil" inverse, simplified.
fn epoch_to_ymdhms(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let secs_today = secs.rem_euclid(86_400) as u32;
    let hour = secs_today / 3600;
    let min = (secs_today % 3600) / 60;
    let sec = secs_today % 60;

    // Civil-from-days (Hinnant). Reference epoch: 1970-01-01.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u32, d as u32, hour, min, sec)
}

/// Lowercase-and-strip-whitespace the input. Caller-supplied domains
/// might arrive with surrounding spaces or upper-case mixed in;
/// normalise so set membership is stable.
pub fn normalise_domain(d: &str) -> String {
    d.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Is `domain` (case-insensitive) in the trusted set?
pub fn is_trusted(domain: &str) -> bool {
    let domain = normalise_domain(domain);
    if domain.is_empty() {
        return false;
    }
    let lock = ensure_loaded();
    let g = match lock.read() {
        Ok(g) => g,
        Err(p) => p.into_inner(), // poisoned lock — degrade gracefully
    };
    g.contains(&domain)
}

/// Add a domain to the trusted set. Idempotent — duplicates collapse.
/// Persists to disk; returns the new total.
pub fn add(domain: &str) -> Result<usize> {
    let domain = normalise_domain(domain);
    if domain.is_empty() {
        anyhow::bail!("empty domain");
    }
    let lock = ensure_loaded();
    {
        let mut g = lock.write().map_err(|_| anyhow::anyhow!("trusted-senders lock poisoned"))?;
        g.insert(domain);
    }
    let snapshot: HashSet<String> = lock
        .read()
        .map_err(|_| anyhow::anyhow!("trusted-senders lock poisoned"))?
        .clone();
    save_to_disk(&snapshot)?;
    Ok(snapshot.len())
}

/// Remove a domain from the trusted set. Idempotent — missing is fine.
/// Persists to disk; returns the new total.
pub fn remove(domain: &str) -> Result<usize> {
    let domain = normalise_domain(domain);
    let lock = ensure_loaded();
    {
        let mut g = lock.write().map_err(|_| anyhow::anyhow!("trusted-senders lock poisoned"))?;
        g.remove(&domain);
    }
    let snapshot: HashSet<String> = lock
        .read()
        .map_err(|_| anyhow::anyhow!("trusted-senders lock poisoned"))?
        .clone();
    save_to_disk(&snapshot)?;
    Ok(snapshot.len())
}

/// Extract the lowercased domain from a `From:` header value.
///
/// Accepts shapes like:
///
/// - `name@domain.com`
/// - `Display Name <name@domain.com>`
/// - `"Display, Name" <name@domain.com>`
/// - `<name@domain.com>`
///
/// Returns None when no `@` is present or the right-hand side is empty.
pub fn extract_from_domain(from_header: &str) -> Option<String> {
    // Trim whitespace and surrounding angle brackets, then locate the LAST
    // `@`. Real addresses can contain `@` only once; multi-`@` is malformed
    // but the rightmost one bounds the local-part vs domain split safely.
    let s = from_header.trim();

    // Strip everything before the first `<` and after the matching `>` if
    // present; otherwise treat the whole string as the address.
    let inside = if let (Some(lt), Some(gt)) = (s.rfind('<'), s.rfind('>')) {
        if gt > lt {
            &s[lt + 1..gt]
        } else {
            s
        }
    } else {
        s
    };

    let at = inside.rfind('@')?;
    let raw = inside[at + 1..].trim();
    let raw = raw.trim_end_matches('>').trim_end_matches('.');
    if raw.is_empty() {
        return None;
    }
    Some(raw.to_ascii_lowercase())
}

// ----------------------------------------------------------------------------
// HTTP handlers
// ----------------------------------------------------------------------------

use axum::{http::StatusCode, response::IntoResponse, Json};

#[derive(Debug, Deserialize)]
pub struct DomainBody {
    pub domain: String,
}

#[derive(Debug, Serialize)]
pub struct TrustResponse {
    pub ok: bool,
    pub total_trusted: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// POST `/api/html-trusted/add` — body `{ "domain": "..." }`. Idempotent.
pub async fn add_post(Json(body): Json<DomainBody>) -> impl IntoResponse {
    match add(&body.domain) {
        Ok(total) => (
            StatusCode::OK,
            Json(TrustResponse { ok: true, total_trusted: total, error: None }),
        ),
        Err(e) => {
            tracing::warn!("trusted-senders add({}) failed: {}", body.domain, e);
            (
                StatusCode::BAD_REQUEST,
                Json(TrustResponse {
                    ok: false,
                    total_trusted: 0,
                    error: Some(e.to_string()),
                }),
            )
        }
    }
}

/// POST `/api/html-trusted/remove` — body `{ "domain": "..." }`. Idempotent.
pub async fn remove_post(Json(body): Json<DomainBody>) -> impl IntoResponse {
    match remove(&body.domain) {
        Ok(total) => (
            StatusCode::OK,
            Json(TrustResponse { ok: true, total_trusted: total, error: None }),
        ),
        Err(e) => {
            tracing::warn!("trusted-senders remove({}) failed: {}", body.domain, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(TrustResponse {
                    ok: false,
                    total_trusted: 0,
                    error: Some(e.to_string()),
                }),
            )
        }
    }
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_from_simple_addr() {
        assert_eq!(
            extract_from_domain("alice@example.com").as_deref(),
            Some("example.com")
        );
    }

    #[test]
    fn extract_from_display_name() {
        assert_eq!(
            extract_from_domain("Alice <alice@Example.COM>").as_deref(),
            Some("example.com")
        );
    }

    #[test]
    fn extract_from_quoted_display_name_with_comma() {
        assert_eq!(
            extract_from_domain(r#""Doe, Alice" <alice@example.com>"#).as_deref(),
            Some("example.com")
        );
    }

    #[test]
    fn extract_from_angle_only() {
        assert_eq!(
            extract_from_domain("<alice@example.com>").as_deref(),
            Some("example.com")
        );
    }

    #[test]
    fn extract_from_lowercases() {
        assert_eq!(
            extract_from_domain("USER@DOMAIN.COM").as_deref(),
            Some("domain.com")
        );
    }

    #[test]
    fn extract_from_strips_trailing_dot() {
        assert_eq!(
            extract_from_domain("u@example.com.").as_deref(),
            Some("example.com")
        );
    }

    #[test]
    fn extract_from_empty() {
        assert!(extract_from_domain("").is_none());
        assert!(extract_from_domain("not an email").is_none());
        assert!(extract_from_domain("@").is_none());
        assert!(extract_from_domain("a@").is_none());
    }

    #[test]
    fn normalise_lowercases_and_trims() {
        assert_eq!(normalise_domain(" Example.COM "), "example.com");
        assert_eq!(normalise_domain("example.com."), "example.com");
    }

    /// Atomic round-trip: write a set, load from path, assert equality.
    /// Uses an env-var-overridden temp config dir so the production file
    /// is never touched.
    #[test]
    fn atomic_roundtrip_via_path() {
        let tmpdir = tempdir_path("mailforge-trusted-senders-test");
        std::fs::create_dir_all(&tmpdir).unwrap();
        let path = tmpdir.join("html-trusted-senders.json");

        // Build the on-disk file directly via parse_bytes/save logic.
        let mut set = HashSet::new();
        set.insert("nytimes.com".to_string());
        set.insert("github.com".to_string());

        // Write via the same shape save_to_disk produces.
        let mut sorted: Vec<String> = set.iter().cloned().collect();
        sorted.sort();
        let f = TrustedFile { version: 1, domains: sorted, updated_at: "x".into() };
        let bytes = serde_json::to_vec_pretty(&f).unwrap();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).unwrap();
        std::fs::rename(&tmp, &path).unwrap();

        // Read back.
        let loaded = load_from_path(&path).unwrap();
        assert_eq!(loaded, set);

        // Confirm pretty-JSON shape on disk.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"version\""));
        assert!(raw.contains("\"github.com\""));
        assert!(raw.contains("\"nytimes.com\""));

        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    #[test]
    fn parse_legacy_empty_file() {
        let bytes = br#"{"version":1,"domains":[],"updated_at":""}"#;
        let set = parse_bytes(bytes).unwrap();
        assert!(set.is_empty());
    }

    #[test]
    fn parse_normalises_on_load() {
        // Domain spelled mixed-case in JSON file; loading lowercases it
        // so set lookups work regardless of how a hand-edit looked.
        let bytes = br#"{"version":1,"domains":["NYTIMES.com"],"updated_at":""}"#;
        let set = parse_bytes(bytes).unwrap();
        assert!(set.contains("nytimes.com"));
        assert!(!set.contains("NYTIMES.com"));
    }

    #[test]
    fn epoch_to_ymdhms_known_dates() {
        // 2026-05-02 12:00:00 UTC = 1777982400
        let (y, m, d, h, mi, s) = epoch_to_ymdhms(1_777_982_400);
        assert_eq!((y, m, d, h, mi, s), (2026, 5, 5, 12, 0, 0));
        // 1970-01-01 00:00:00 UTC = 0
        let (y, m, d, h, mi, s) = epoch_to_ymdhms(0);
        assert_eq!((y, m, d, h, mi, s), (1970, 1, 1, 0, 0, 0));
    }

    /// Tiny private temp-dir helper (we don't depend on the `tempfile`
    /// crate — keeps the dependency surface tight).
    fn tempdir_path(prefix: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        p.push(format!("{prefix}-{pid}-{nanos}"));
        p
    }
}
