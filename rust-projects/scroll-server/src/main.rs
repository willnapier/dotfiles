//! scroll-server entry point. All logic lives in `lib.rs` so integration
//! tests can spin up the same router on an ephemeral port.

use anyhow::Result;
use scroll_server::config::Config;

#[tokio::main]
async fn main() -> Result<()> {
    let _cfg = Config::from_env()?;
    eprintln!("scroll-server: skeleton — handlers wired in subsequent commits");
    Ok(())
}
