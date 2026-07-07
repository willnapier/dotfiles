//! scroll-server entry point. All logic lives in `lib.rs` so integration
//! tests can spin up the same router on an ephemeral port.

use anyhow::Result;
use scroll_server::{config::Config, serve};

#[tokio::main]
async fn main() -> Result<()> {
    let cfg = Config::from_env()?;
    serve(cfg).await
}
