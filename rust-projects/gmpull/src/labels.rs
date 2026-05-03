//! Label cache — `users.labels.list` once at startup.
//!
//! The maildir layer needs label *names* (e.g. "Newsletters",
//! "Cat/Sub") rather than the opaque `Label_3791` IDs Gmail returns
//! per-message. This module fetches the full label list once and
//! exposes a `HashMap<String, String>` (id → name) for cheap lookup.
//!
//! For v1 we don't actually mirror per-label folders (that's the
//! lieer "Option B" we explicitly ruled out); the cache is here so
//! flag derivation and future per-folder mirroring can both consult
//! the same authoritative map.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;

const API_BASE: &str = "https://gmail.googleapis.com/gmail/v1";

#[derive(Debug, Deserialize)]
struct LabelListResponse {
    #[serde(default)]
    labels: Vec<Label>,
}

#[derive(Debug, Deserialize)]
struct Label {
    id: String,
    name: String,
}

/// Fetch every label and return id → name. System labels (INBOX,
/// UNREAD, STARRED, …) are included with their canonical uppercase
/// IDs so callers don't have to special-case them.
pub async fn list_labels(
    http: &reqwest::Client,
    token: &str,
) -> Result<HashMap<String, String>> {
    let url = format!("{API_BASE}/users/me/labels");
    let resp = http
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .context("GET labels.list")?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("labels.list returned HTTP {status}: {body}");
    }
    let parsed: LabelListResponse =
        serde_json::from_str(&body).context("parsing labels.list JSON")?;
    let mut out = HashMap::with_capacity(parsed.labels.len());
    for l in parsed.labels {
        out.insert(l.id, l.name);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_list_parses() {
        let body = r#"{
            "labels": [
                {"id": "INBOX", "name": "INBOX"},
                {"id": "Label_1", "name": "Newsletters"}
            ]
        }"#;
        let r: LabelListResponse = serde_json::from_str(body).unwrap();
        assert_eq!(r.labels.len(), 2);
        assert_eq!(r.labels[1].name, "Newsletters");
    }
}
