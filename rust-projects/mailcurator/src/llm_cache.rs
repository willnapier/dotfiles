//! Append-only cache for LLM extraction results.
//!
//! Keyed by `(message_id, vendor_module_name)` because the same message
//! is never re-extracted by the same module — so on a cache hit, we can
//! skip the LLM call entirely. Across module changes (or vendor changes
//! that we deliberately want to re-extract for) the human runs
//! `mailcurator llm-cache clear` to flush.
//!
//! Storage: `~/.local/share/mailcurator/llm-cache.<hostname>.jsonl`. One
//! line per cached extraction. Each machine writes only its own per-host
//! file (avoids Syncthing conflicts when the cache directory is shared).
//! On load, all per-host files are unioned into a single in-memory map so
//! Mac benefits from cache entries written on nimbini and vice versa.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use crate::store;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CachedExtraction {
    pub message_id: String,
    pub module: String,
    pub ts: String,
    pub fields: Map<String, Value>,
}

/// In-memory view of the cache. Built once per `mailcurator run` from the
/// jsonl file; subsequent `get` calls are O(1).
pub struct Cache {
    by_key: HashMap<(String, String), Map<String, Value>>,
    path: PathBuf,
}

impl Cache {
    pub fn load() -> Result<Self> {
        // Reads are union across all per-host files (and the legacy
        // single-file if still present); writes go to THIS machine's
        // per-host file. So both machines' cache hits compound.
        let lines = store::read_category_lines("llm-cache")?;
        let mut by_key = HashMap::new();
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(rec) = serde_json::from_str::<CachedExtraction>(&line) else {
                continue; // tolerate corrupt lines
            };
            by_key.insert((rec.message_id, rec.module), rec.fields);
        }
        let path = store::category_path("llm-cache")?;
        Ok(Self { by_key, path })
    }

    pub fn get(&self, message_id: &str, module: &str) -> Option<&Map<String, Value>> {
        self.by_key.get(&(message_id.to_string(), module.to_string()))
    }

    /// Write a new entry. Appends to the jsonl AND updates the in-memory
    /// map so subsequent lookups in the same run see the freshly-cached
    /// result (relevant if the same message somehow gets extracted twice
    /// in one run — it shouldn't, but defensive).
    pub fn put(
        &mut self,
        message_id: String,
        module: String,
        fields: Map<String, Value>,
    ) -> Result<()> {
        let rec = CachedExtraction {
            message_id: message_id.clone(),
            module: module.clone(),
            ts: chrono::Utc::now().to_rfc3339(),
            fields: fields.clone(),
        };
        let line = serde_json::to_string(&rec)?;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("opening cache {}", self.path.display()))?;
        writeln!(f, "{line}")?;
        self.by_key.insert((message_id, module), fields);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn fresh_cache_in(tmp: &TempDir) -> Cache {
        // Override store_dir via env? Simpler: build manually.
        let path = tmp.path().join("llm-cache.jsonl");
        Cache { by_key: HashMap::new(), path }
    }

    #[test]
    fn put_then_get_round_trip() {
        let tmp = TempDir::new().unwrap();
        let mut c = fresh_cache_in(&tmp);
        let mut fields = Map::new();
        fields.insert("total".into(), json!("3.49"));
        c.put("msgA".into(), "amazon_orders".into(), fields.clone()).unwrap();
        let got = c.get("msgA", "amazon_orders").unwrap();
        assert_eq!(got.get("total").and_then(|v| v.as_str()), Some("3.49"));
    }

    #[test]
    fn miss_returns_none() {
        let tmp = TempDir::new().unwrap();
        let c = fresh_cache_in(&tmp);
        assert!(c.get("nonexistent", "amazon_orders").is_none());
    }

    #[test]
    fn persists_to_disk() {
        use std::fs::File;
        use std::io::{BufRead, BufReader};
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("cache.jsonl");
        {
            let mut c = Cache { by_key: HashMap::new(), path: path.clone() };
            let mut fields = Map::new();
            fields.insert("x".into(), json!("y"));
            c.put("m1".into(), "mod".into(), fields).unwrap();
        }
        // Reload from same file — bypass the helper since we're not
        // using the real store_dir.
        let f = File::open(&path).unwrap();
        let mut by_key = HashMap::new();
        for line in BufReader::new(f).lines() {
            let line = line.unwrap();
            let rec: CachedExtraction = serde_json::from_str(&line).unwrap();
            by_key.insert((rec.message_id, rec.module), rec.fields);
        }
        let got = by_key.get(&("m1".to_string(), "mod".to_string())).unwrap();
        assert_eq!(got.get("x").and_then(|v| v.as_str()), Some("y"));
    }
}
