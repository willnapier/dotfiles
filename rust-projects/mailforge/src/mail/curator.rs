//! Bridge to the standalone `mailcurator` CLI.
//!
//! Exposes endpoints that spawn `mailcurator run --now` (with optional
//! `--dry-run` and `--only` flags) and return the parsed result as
//! JSON. The dashboard's "Sweep" button uses [`sweep_post`] for the
//! "yeah, I've seen them and they can go now" workflow.
//!
//! [`blacklist_post`] — kill-sender keystroke (`K` in message view):
//! append a per-sender mailcurator policy to `policies.toml` AND
//! immediately `mailcurator run --only <new-policy>` so the messages
//! disappear from the listing.
//!
//! Why bridge through MailForge instead of having the user shell out:
//! - One-click ergonomics from the listing toolbar.
//! - Two-step confirm: button does a dry-run first, shows the count,
//!   user confirms, then a live run fires.
//! - Result reported via toast in the same UI surface they're using.

use axum::extract::Query;
use axum::response::Json;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Deserialize)]
pub struct SweepParams {
    /// Run a single named policy (`mailcurator run --only <name>`).
    /// When absent, runs all policies.
    pub only: Option<String>,
    /// "1" → preview only (`--dry-run`). Absent → live run.
    pub dry_run: Option<String>,
}

#[derive(Serialize)]
pub struct SweepResult {
    pub ok: bool,
    pub dry_run: bool,
    pub tagged_on_arrival: u64,
    pub archived: u64,
    pub trashed: u64,
    pub stdout: String,
    pub error: Option<String>,
}

pub async fn sweep_post(Query(params): Query<SweepParams>) -> Json<SweepResult> {
    let dry = params.dry_run.as_deref() == Some("1");

    let mut cmd = Command::new("mailcurator");
    cmd.arg("run").arg("--now");
    if dry {
        cmd.arg("--dry-run");
    }
    if let Some(only) = &params.only {
        cmd.arg("--only").arg(only);
    }

    match cmd.output() {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            Json(SweepResult {
                ok: true,
                dry_run: dry,
                tagged_on_arrival: extract_total(&stdout, "tagged-on-arrival=").unwrap_or(0),
                archived: extract_total(&stdout, "archived=").unwrap_or(0),
                trashed: extract_total(&stdout, "trashed=").unwrap_or(0),
                stdout,
                error: None,
            })
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            Json(SweepResult {
                ok: false,
                dry_run: dry,
                tagged_on_arrival: 0,
                archived: 0,
                trashed: 0,
                stdout,
                error: Some(stderr),
            })
        }
        Err(e) => Json(SweepResult {
            ok: false,
            dry_run: dry,
            tagged_on_arrival: 0,
            archived: 0,
            trashed: 0,
            stdout: String::new(),
            error: Some(format!("spawn failed: {e}")),
        }),
    }
}

/// Pull a totals number out of mailcurator's "TOTAL  tagged-on-arrival=N  archived=N  trashed=N"
/// summary line. Token-based so it survives whitespace and column-width changes.
fn extract_total(text: &str, prefix: &str) -> Option<u64> {
    text.lines()
        .filter(|l| l.contains("TOTAL"))
        .find_map(|line| {
            line.split_whitespace()
                .find_map(|tok| tok.strip_prefix(prefix).and_then(|n| n.parse().ok()))
        })
}

// ----------------------------------------------------------------------------
// Kill-sender (blacklist) endpoint
// ----------------------------------------------------------------------------

/// Request body for `POST /api/mailcurator/blacklist`.
///
/// Accepts ONE of three input forms (in priority order):
/// 1. `from`: a domain prefixed with `@`, e.g. `"@bettermode.io"`.
///    Direct path used by the message-view kill-sender (`K`) — the JS
///    has already extracted the domain from the visible header.
/// 2. `msg_id`: a notmuch message id; server fetches the message and
///    extracts the From-domain. Used by the listing-view kill-sender
///    (`K`) when the row represents a single message.
/// 3. `thread_id`: a notmuch thread id; server fetches the thread,
///    uses the first message's From-domain. Used by the listing-view
///    kill-sender when the row is a multi-message thread (no single
///    msg-id, see [`super::tag::IdsRequest`] for the parallel pattern).
///
/// At least one must be present. `from` wins if multiple are supplied.
#[derive(Deserialize)]
pub struct BlacklistBody {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub msg_id: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
}

/// Response for `POST /api/mailcurator/blacklist`.
#[derive(Serialize)]
pub struct BlacklistResult {
    pub ok: bool,
    pub policy_name: String,
    pub already_existed: bool,
    pub trashed_immediately: u64,
    pub stdout: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Resolve `~/.config/mailcurator/policies.toml`. Honours `XDG_CONFIG_HOME`,
/// falls back to `$HOME/.config`.
pub fn policies_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("mailcurator").join("policies.toml"))
}

/// Resolve [`BlacklistBody`] into a canonical `@domain` string. Tries the
/// explicit `from` field first; if absent, fetches the message (or first
/// message of the thread) via notmuch and extracts the From-domain.
///
/// Returns the `@<domain>` form expected by [`derive_policy_name_from_domain`]
/// and the rest of the kill-sender pipeline. Errors are user-facing strings.
fn resolve_from(body: &BlacklistBody) -> Result<String, String> {
    if let Some(f) = body.from.as_ref() {
        let trimmed = f.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    if let Some(msg_id) = body.msg_id.as_ref() {
        let trimmed = msg_id.trim();
        if !trimmed.is_empty() {
            let msg = super::notmuch_db::show(trimmed)
                .map_err(|e| format!("notmuch show {}: {}", trimmed, e))?;
            return extract_at_domain(msg.from.as_deref())
                .ok_or_else(|| format!("no From-domain on message {}", trimmed));
        }
    }
    if let Some(thread_id) = body.thread_id.as_ref() {
        let trimmed = thread_id.trim();
        if !trimmed.is_empty() {
            let msgs = super::notmuch_db::show_thread(trimmed)
                .map_err(|e| format!("notmuch show_thread {}: {}", trimmed, e))?;
            let first = msgs.into_iter().next().ok_or_else(|| {
                format!("thread {} has no messages", trimmed)
            })?;
            return extract_at_domain(first.from.as_deref())
                .ok_or_else(|| format!("no From-domain on first message of thread {}", trimmed));
        }
    }
    Err("must supply one of: from, msg_id, thread_id".to_string())
}

/// Extract the `@<domain>` form from a raw From header.
/// Handles `Name <local@domain>`, `local@domain`, and `<local@domain>`
/// shapes. Returns None when no `@<non-empty>` is present.
fn extract_at_domain(from_header: Option<&str>) -> Option<String> {
    let raw = from_header?.trim();
    if raw.is_empty() {
        return None;
    }
    // Strip a trailing `>` (and any whitespace before it).
    let cleaned = raw.trim_end_matches(|c: char| c == '>' || c.is_whitespace());
    let at = cleaned.rfind('@')?;
    let dom = cleaned[at + 1..].trim();
    if dom.is_empty() {
        return None;
    }
    Some(format!("@{}", dom.to_ascii_lowercase()))
}

/// Derive a stable mailcurator policy name from the `from` value.
///
/// Strips the leading `@`, lowercases, and replaces `.` with `-`.
///
/// - `"@Bettermode.io"` → `"bettermode-io"`
/// - `"@example.co.uk"` → `"example-co-uk"`
/// - `"foo@bar.com"`    → `"bar-com"` (rightmost `@` is the splitter)
///
/// The result is suitable as both a TOML `name = "..."` value and a
/// `mailcurator run --only <name>` argument.
pub fn derive_policy_name_from_domain(from: &str) -> String {
    // Take everything after the rightmost `@` so callers can pass either
    // a bare `@domain` or a full `local@domain` shape.
    let after_at = match from.rfind('@') {
        Some(idx) => &from[idx + 1..],
        None => from,
    };
    after_at
        .trim()
        .to_ascii_lowercase()
        .replace('.', "-")
}

/// Does `policies_toml` already define a `[[policy]]` named `name`?
///
/// Whole-word match in the TOML value position: looks for a line of the
/// shape `name = "<name>"` (allowing arbitrary whitespace either side
/// of the `=`). String-only — we do not parse the TOML, because the
/// hand-edited `policies.toml` carries comments that would round-trip
/// poorly through serde + the `toml` crate's serialiser.
pub fn policy_name_exists(policies_toml: &str, name: &str) -> bool {
    let needle = format!("\"{}\"", name);
    for line in policies_toml.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("name") {
            continue;
        }
        // Looking for the pattern `name = "<name>"`.
        let after_name = match trimmed.strip_prefix("name") {
            Some(rest) => rest.trim_start(),
            None => continue,
        };
        let after_eq = match after_name.strip_prefix('=') {
            Some(rest) => rest.trim_start(),
            None => continue,
        };
        // Trim trailing comment after the closing quote.
        let value = after_eq.split('#').next().unwrap_or("").trim();
        if value == needle {
            return true;
        }
    }
    false
}

/// Append a new `[[policy]]` block to a copy of `existing` and return the
/// new content. Idempotent: returns `existing` unchanged when a policy
/// of `name` already exists.
///
/// Block shape:
/// ```toml
///
/// # Added <YYYY-MM-DD HH:MM:SS> via MailForge K (kill-sender) keystroke.
/// [[policy]]
/// name = "<name>"
/// from = "<from>"
/// intended_categories = ["bulk-marketing"]
/// delete_after_days = 1
/// ```
pub fn append_policy_block(existing: &str, name: &str, from: &str, timestamp: &str) -> String {
    if policy_name_exists(existing, name) {
        return existing.to_string();
    }
    let mut out = String::with_capacity(existing.len() + 256);
    out.push_str(existing);
    // Ensure exactly one blank line of separation between previous content
    // and the new block. Empty file → no leading whitespace at all (so the
    // comment is the very first byte). Non-empty file → terminate the
    // last line if needed, then emit a blank line.
    if !existing.is_empty() {
        if !existing.ends_with('\n') {
            out.push('\n');
        }
        if !out.ends_with("\n\n") {
            out.push('\n');
        }
    }
    out.push_str(&format!(
        "# Added {timestamp} via MailForge K (kill-sender) keystroke.\n"
    ));
    out.push_str("[[policy]]\n");
    out.push_str(&format!("name = \"{name}\"\n"));
    out.push_str(&format!("from = \"{from}\"\n"));
    out.push_str("intended_categories = [\"bulk-marketing\"]\n");
    out.push_str("delete_after_days = 1\n");
    out
}

/// Atomic write: `<path>.tmp` then `rename(2)`. Creates parent dirs if
/// missing. Never leaves the target half-written: a crash either
/// preserves the previous content or hands over the new content
/// whole.
fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Sibling-temp so the rename is on the same filesystem.
    let tmp = path.with_extension(match path.extension() {
        Some(ext) => format!("{}.tmp", ext.to_string_lossy()),
        None => "tmp".to_string(),
    });
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

/// Best-effort "now" for the policy comment. Format `YYYY-MM-DD HH:MM:SS`
/// (UTC). Hand-rolled to avoid pulling in `chrono` or `time`. On the
/// rare SystemTime failure, returns an empty string.
fn now_local_ish() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d,
        Err(_) => return String::new(),
    };
    let secs = dur.as_secs() as i64;
    let (y, m, d, h, mi, s) = epoch_to_ymdhms(secs);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// Civil-from-days (Hinnant). Inlined here so curator.rs has no
/// dependency on trusted_senders.rs's identical helper.
fn epoch_to_ymdhms(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let secs_today = secs.rem_euclid(86_400) as u32;
    let hour = secs_today / 3600;
    let min = (secs_today % 3600) / 60;
    let sec = secs_today % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u32, d as u32, hour, min, sec)
}

/// `POST /api/mailcurator/blacklist`
///
/// Body: `{ "from": "@<domain>" }`. Validates the input, derives a policy
/// name, appends a `[[policy]]` block to `policies.toml` if novel, then
/// runs `mailcurator run --now --only <name>` so existing messages from
/// the sender disappear immediately.
///
/// The policy IS in place before the sweep runs — so even if the sweep
/// fails, the next scheduled run will still catch it. We surface the
/// stderr in `error` for visibility, but `ok: true` reflects "policy
/// added"; the trash count is best-effort.
pub async fn blacklist_post(Json(body): Json<BlacklistBody>) -> Json<BlacklistResult> {
    // ---- Resolve `from` domain ----------------------------------------
    // Priority: explicit `from` > resolved-from-msg_id > resolved-from-thread_id.
    let from = match resolve_from(&body) {
        Ok(f) => f,
        Err(e) => {
            return Json(BlacklistResult {
                ok: false,
                policy_name: String::new(),
                already_existed: false,
                trashed_immediately: 0,
                stdout: String::new(),
                error: Some(e),
            });
        }
    };

    if !from.contains('@')
        || from.len() < 2
        || from.len() > 100
    {
        return Json(BlacklistResult {
            ok: false,
            policy_name: String::new(),
            already_existed: false,
            trashed_immediately: 0,
            stdout: String::new(),
            error: Some(format!(
                "invalid resolved `from`: must contain @, length 2-100 (got {} chars: {:?})",
                from.len(),
                from
            )),
        });
    }

    // ---- Derive policy name -------------------------------------------
    let policy_name = derive_policy_name_from_domain(&from);
    if policy_name.is_empty() {
        return Json(BlacklistResult {
            ok: false,
            policy_name: String::new(),
            already_existed: false,
            trashed_immediately: 0,
            stdout: String::new(),
            error: Some("could not derive a non-empty policy name from `from`".to_string()),
        });
    }

    // ---- Resolve config path ------------------------------------------
    let path = match policies_path() {
        Some(p) => p,
        None => {
            return Json(BlacklistResult {
                ok: false,
                policy_name,
                already_existed: false,
                trashed_immediately: 0,
                stdout: String::new(),
                error: Some("could not resolve mailcurator config dir".to_string()),
            });
        }
    };

    // ---- Read existing content (missing file → empty) -----------------
    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Json(BlacklistResult {
                ok: false,
                policy_name,
                already_existed: false,
                trashed_immediately: 0,
                stdout: String::new(),
                error: Some(format!("reading {}: {e}", path.display())),
            });
        }
    };

    // ---- Idempotent append --------------------------------------------
    let already = policy_name_exists(&existing, &policy_name);
    if !already {
        let new_content =
            append_policy_block(&existing, &policy_name, &from, &now_local_ish());
        if let Err(e) = atomic_write(&path, &new_content) {
            return Json(BlacklistResult {
                ok: false,
                policy_name,
                already_existed: false,
                trashed_immediately: 0,
                stdout: String::new(),
                error: Some(format!("writing {}: {e}", path.display())),
            });
        }
    }

    // ---- Run mailcurator on the new policy only -----------------------
    let mut cmd = Command::new("mailcurator");
    cmd.arg("run").arg("--now").arg("--only").arg(&policy_name);

    match cmd.output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let trashed = extract_total(&stdout, "trashed=").unwrap_or(0);
            // Don't fail the request if mailcurator returned non-zero —
            // the policy IS in place, future runs will catch it.
            let error = if out.status.success() {
                None
            } else {
                Some(format!(
                    "mailcurator exited non-zero ({}); policy still installed. stderr: {}",
                    out.status, stderr.trim()
                ))
            };
            Json(BlacklistResult {
                ok: true,
                policy_name,
                already_existed: already,
                trashed_immediately: trashed,
                stdout,
                error,
            })
        }
        Err(e) => Json(BlacklistResult {
            // Spawn failed — but the policy file was already written. Mark
            // ok=true so the user sees their action took effect; surface
            // the spawn error in `error`.
            ok: true,
            policy_name,
            already_existed: already,
            trashed_immediately: 0,
            stdout: String::new(),
            error: Some(format!("spawn failed: {e}")),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_total_parses_summary_line() {
        let stdout = "verification-codes              +arrival=25    →trash=25  \n\
                      TOTAL  tagged-on-arrival=25  archived=0  trashed=25  [DRY RUN — no changes made]";
        assert_eq!(extract_total(stdout, "trashed="), Some(25));
        assert_eq!(extract_total(stdout, "tagged-on-arrival="), Some(25));
        assert_eq!(extract_total(stdout, "archived="), Some(0));
    }

    #[test]
    fn extract_total_returns_none_when_absent() {
        let stdout = "no policies matched\n";
        assert_eq!(extract_total(stdout, "trashed="), None);
    }

    // ---- Kill-sender helpers --------------------------------------

    #[test]
    fn derive_policy_name_from_domain_basic() {
        assert_eq!(derive_policy_name_from_domain("@bettermode.io"), "bettermode-io");
        assert_eq!(derive_policy_name_from_domain("@example.co.uk"), "example-co-uk");
    }

    #[test]
    fn derive_policy_name_from_domain_lowercases() {
        assert_eq!(derive_policy_name_from_domain("@Bettermode.IO"), "bettermode-io");
        assert_eq!(derive_policy_name_from_domain("@FOO.BAR"), "foo-bar");
    }

    #[test]
    fn derive_policy_name_from_domain_full_addr() {
        // Caller might pass a whole `local@domain` instead of `@domain`;
        // rightmost @ wins.
        assert_eq!(
            derive_policy_name_from_domain("noreply@bettermode.io"),
            "bettermode-io"
        );
    }

    #[test]
    fn derive_policy_name_from_domain_no_at() {
        // No `@` → input is taken verbatim (after lowercasing + dot→dash).
        assert_eq!(derive_policy_name_from_domain("bettermode.io"), "bettermode-io");
    }

    #[test]
    fn policy_name_exists_finds_match() {
        let toml = "[[policy]]\nname = \"existing\"\nfrom = \"@x\"\n";
        assert!(policy_name_exists(toml, "existing"));
        assert!(!policy_name_exists(toml, "other"));
    }

    #[test]
    fn policy_name_exists_ignores_substring() {
        // Whole-word match: "existing" ≠ "exist" even though one is a prefix of the other.
        let toml = "[[policy]]\nname = \"existing\"\n";
        assert!(!policy_name_exists(toml, "exist"));
        assert!(!policy_name_exists(toml, "existi"));
    }

    #[test]
    fn policy_name_exists_ignores_value_position_only_for_name() {
        // A `from` value coincidentally matching the lookup string must
        // not produce a false positive — only `name = ...` lines count.
        let toml = "[[policy]]\nname = \"foo\"\nfrom = \"@bar\"\n";
        assert!(policy_name_exists(toml, "foo"));
        assert!(!policy_name_exists(toml, "bar"));
        assert!(!policy_name_exists(toml, "@bar"));
    }

    #[test]
    fn policies_toml_append_idempotent() {
        // Appending a policy that already exists must leave the file
        // byte-for-byte identical.
        let original = "# top comment\n\n[[policy]]\nname = \"foo\"\nfrom = \"@foo.com\"\n";
        let after = append_policy_block(original, "foo", "@foo.com", "2026-05-02 10:00:00");
        assert_eq!(after, original);
    }

    #[test]
    fn policies_toml_append_new_block_appends_at_end() {
        let original = "[[policy]]\nname = \"existing\"\nfrom = \"@old.com\"\n";
        let after = append_policy_block(
            original,
            "newdomain-com",
            "@newdomain.com",
            "2026-05-02 10:00:00",
        );
        assert!(after.starts_with(original));
        assert!(after.contains("name = \"newdomain-com\""));
        assert!(after.contains("from = \"@newdomain.com\""));
        assert!(after.contains("intended_categories = [\"bulk-marketing\"]"));
        assert!(after.contains("delete_after_days = 1"));
        assert!(after.contains(
            "# Added 2026-05-02 10:00:00 via MailForge K (kill-sender) keystroke."
        ));
    }

    #[test]
    fn policies_toml_append_handles_empty_file() {
        // No existing policies file → still produces a valid block.
        let after = append_policy_block("", "foo-com", "@foo.com", "2026-05-02 10:00:00");
        assert!(after.contains("[[policy]]"));
        assert!(after.contains("name = \"foo-com\""));
        // Ensure the appended block starts with a comment header followed
        // by `[[policy]]`. No leading blank line at file start.
        assert!(after.starts_with("# Added "));
    }

    #[test]
    fn policies_toml_append_handles_no_trailing_newline() {
        // Some editors strip trailing newlines; the append path must
        // still produce a well-formed concatenation.
        let original = "[[policy]]\nname = \"existing\"\nfrom = \"@old.com\""; // no trailing \n
        let after = append_policy_block(
            original,
            "newdomain-com",
            "@newdomain.com",
            "2026-05-02 10:00:00",
        );
        assert!(after.contains("\"existing\""));
        assert!(after.contains("\"newdomain-com\""));
        // The new block must not be glued onto the previous block's last line.
        assert!(after.contains("\n\n# Added "));
    }

    #[test]
    fn epoch_to_ymdhms_known_dates() {
        // 1970-01-01 00:00:00 UTC = 0
        let (y, m, d, h, mi, s) = epoch_to_ymdhms(0);
        assert_eq!((y, m, d, h, mi, s), (1970, 1, 1, 0, 0, 0));
    }
}
