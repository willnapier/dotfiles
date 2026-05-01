//! Bridge to the standalone `mailcurator` CLI.
//!
//! Exposes a single endpoint that spawns `mailcurator run --now` (with
//! optional `--dry-run` and `--only` flags) and returns the parsed
//! result as JSON. The dashboard's "Sweep" button uses this for the
//! "yeah, I've seen them and they can go now" workflow.
//!
//! Why bridge through MailForge instead of having the user shell out:
//! - One-click ergonomics from the listing toolbar.
//! - Two-step confirm: button does a dry-run first, shows the count,
//!   user confirms, then a live run fires.
//! - Result reported via toast in the same UI surface they're using.

use axum::extract::Query;
use axum::response::Json;
use serde::{Deserialize, Serialize};
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
}
