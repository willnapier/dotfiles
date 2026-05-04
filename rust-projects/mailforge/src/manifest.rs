use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Manifest {
    Html(HtmlManifest),
    Pdf(PdfManifest),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HtmlManifest {
    pub subject: Option<String>,
    pub from: Option<String>,
    pub date: Option<String>,
    pub html_file: String,
    /// Map: original cid (without the "cid:" prefix) -> on-disk filename under cid/
    pub assets: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PdfManifest {
    pub subject: Option<String>,
    pub from: Option<String>,
    pub date: Option<String>,
    pub pdf_file: String,
    pub pdf_filename: Option<String>,
}

pub fn cache_root() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .context("HOME unset, cannot determine cache directory")?;
    Ok(base.join("mailforge"))
}

pub fn write(dir: &Path, m: &Manifest) -> Result<()> {
    let path = dir.join("manifest.json");
    let f = std::fs::File::create(&path)
        .with_context(|| format!("creating {}", path.display()))?;
    serde_json::to_writer_pretty(f, m).context("writing manifest.json")
}

// Reader for the manifest. Kept around as part of the public API even
// though no in-tree caller currently invokes it: the wrapper render
// handler that used to be its only consumer was deleted in the 2026-05-02
// single-iframe refactor (see daemon.rs). Out-of-process tooling and
// future debugging helpers can still call this without resurrecting the
// wrapper.
#[allow(dead_code)]
pub fn read(dir: &Path) -> Result<Manifest> {
    let path = dir.join("manifest.json");
    let f = std::fs::File::open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    serde_json::from_reader(f).context("parsing manifest.json")
}
