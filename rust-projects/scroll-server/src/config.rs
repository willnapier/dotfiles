//! Startup config loaded from env vars (see systemd ExecStart in §12 Phase D).
//!
//! Fail-fast: any required path missing or unreadable → server refuses to start.

use anyhow::{anyhow, Context, Result};
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub scroll_dir: PathBuf,
    pub seed: Vec<u8>,
    pub word_list: Vec<String>,
    pub audit_log: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let bind: SocketAddr = std::env::var("SCROLL_SERVER_BIND")
            .context("SCROLL_SERVER_BIND not set")?
            .parse()
            .context("SCROLL_SERVER_BIND must be a socket address e.g. 127.0.0.1:8770")?;

        let scroll_dir = PathBuf::from(
            std::env::var("SCROLL_SERVER_SCROLL_DIR")
                .context("SCROLL_SERVER_SCROLL_DIR not set")?,
        );
        if !scroll_dir.is_dir() {
            return Err(anyhow!(
                "SCROLL_SERVER_SCROLL_DIR not a readable directory: {}",
                scroll_dir.display()
            ));
        }

        let seed_path = PathBuf::from(
            std::env::var("SCROLL_SERVER_SEED_FILE")
                .context("SCROLL_SERVER_SEED_FILE not set")?,
        );
        let seed = load_seed(&seed_path)
            .with_context(|| format!("loading seed from {}", seed_path.display()))?;

        let word_list_path = PathBuf::from(
            std::env::var("SCROLL_SERVER_WORD_LIST")
                .context("SCROLL_SERVER_WORD_LIST not set")?,
        );
        let word_list = load_word_list(&word_list_path)
            .with_context(|| format!("loading word list from {}", word_list_path.display()))?;

        let audit_log = PathBuf::from(
            std::env::var("SCROLL_SERVER_AUDIT_LOG")
                .context("SCROLL_SERVER_AUDIT_LOG not set")?,
        );
        // Audit log doesn't have to exist yet; verify the parent directory does.
        if let Some(parent) = audit_log.parent() {
            if !parent.as_os_str().is_empty() && !parent.is_dir() {
                return Err(anyhow!(
                    "SCROLL_SERVER_AUDIT_LOG parent directory does not exist: {}",
                    parent.display()
                ));
            }
        }

        Ok(Config {
            bind,
            scroll_dir,
            seed,
            word_list,
            audit_log,
        })
    }
}

/// Read the seed file. Per §14 the on-disk format is `head -c 32 /dev/urandom | base64`,
/// which produces a base64-encoded line. We accept either:
/// - the raw bytes as-is (HMAC keys can be any length), or
/// - a single trailing newline trimmed.
///
/// HMAC is happy with any key length; we just want the bytes William put in the file.
fn load_seed(path: &std::path::Path) -> Result<Vec<u8>> {
    let mut bytes = std::fs::read(path)?;
    // Trim a single trailing newline if present (common when the file was created
    // with shell redirection); preserves any other content as-is.
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.is_empty() {
        return Err(anyhow!("seed file is empty"));
    }
    Ok(bytes)
}

fn load_word_list(path: &std::path::Path) -> Result<Vec<String>> {
    let raw = std::fs::read_to_string(path)?;
    let list: Vec<String> = raw
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if list.is_empty() {
        return Err(anyhow!("word list is empty"));
    }
    Ok(list)
}
