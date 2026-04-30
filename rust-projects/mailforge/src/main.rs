use anyhow::Result;
use clap::{Parser, Subcommand};

mod daemon;
mod mail;
mod manifest;
mod pipe;

#[derive(Parser)]
#[command(version, about = "Browser-handoff viewer for HTML and PDF mail parts")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Read RFC822 from stdin, cache + open in browser.
    Pipe {
        /// Daemon port (default 8765, override via $MELIVIEW_PORT).
        #[arg(long)]
        port: Option<u16>,
        /// Don't actually open the browser (for tests).
        #[arg(long)]
        no_open: bool,
    },
    /// Run the Axum daemon.
    Daemon {
        /// Listen port (default 8765, override via $MELIVIEW_PORT).
        #[arg(long)]
        port: Option<u16>,
    },
}

fn port_from(arg: Option<u16>) -> u16 {
    arg.or_else(|| std::env::var("MELIVIEW_PORT").ok().and_then(|s| s.parse().ok()))
        .unwrap_or(8765)
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Pipe { port, no_open } => pipe::run(port_from(port), no_open),
        Cmd::Daemon { port } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(daemon::run(port_from(port)))
        }
    }
}
