use anyhow::{Context, Result};
use chrono::NaiveDate;
use std::process::Command;

use crate::types::DayActivity;

/// Run `continuum-activity --json --verbose DATE` and parse output.
pub fn fetch_activity(date: NaiveDate) -> Result<DayActivity> {
    let date_str = date.format("%Y-%m-%d").to_string();
    let output = Command::new("continuum-activity")
        .args(["--json", "--verbose", &date_str])
        .output()
        .context("Failed to run continuum-activity")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "continuum-activity failed for {}: {}",
            date_str,
            stderr.trim()
        );
    }

    let stdout = String::from_utf8(output.stdout)
        .context("continuum-activity output is not valid UTF-8")?;

    serde_json::from_str(&stdout)
        .with_context(|| format!("Failed to parse continuum-activity JSON for {}", date_str))
}
