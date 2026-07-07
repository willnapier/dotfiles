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
            audit_log,
        })
    }
}
