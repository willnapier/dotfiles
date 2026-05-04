//! pageprobe — Rust CLI for inspecting Chrome via the DevTools Protocol.
//!
//! See `~/Assistants/shared/pageprobe.md` for design notes and the v0.2
//! roadmap. v0.1 ships start, stop, status, tabs, attach, network, console.
mod cdp;
mod chrome;
mod commands;
mod state;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "pageprobe",
    version,
    about = "Inspect Chrome via the DevTools Protocol",
    long_about = "pageprobe drives a debug-flagged Chrome over the DevTools \
                  Protocol so you can read network timings, console errors, \
                  and DOM state without opening DevTools manually."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Launch debug-Chrome on a dedicated profile.
    Start {
        /// Remote-debugging port to bind.
        #[arg(long, default_value_t = 9222)]
        port: u16,
        /// User-data-dir to use (default: ~/.config/pageprobe/chrome-profile).
        #[arg(long)]
        user_data_dir: Option<PathBuf>,
    },
    /// Stop the running debug-Chrome (gracefully, then SIGKILL after 3s).
    Stop,
    /// Show whether debug-Chrome is running and which tab is attached.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// List all open tabs.
    Tabs {
        #[arg(long)]
        json: bool,
    },
    /// Pick a tab to target with subsequent commands.
    ///
    /// PATTERN matches against URL and title (case-insensitive substring),
    /// or you can pass a target id (full or 8-char short prefix).
    Attach {
        pattern: String,
    },
    /// Show recent network requests for the attached tab.
    Network {
        /// How many requests to keep (oldest-first).
        #[arg(long, default_value_t = 20)]
        last: usize,
        #[arg(long)]
        json: bool,
    },
    /// Show recent console messages for the attached tab.
    Console {
        /// How many messages to keep (oldest-first).
        #[arg(long, default_value_t = 20)]
        last: usize,
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Start { port, user_data_dir } => commands::start::run(port, user_data_dir).await,
        Cmd::Stop => commands::stop::run().await,
        Cmd::Status { json } => commands::status::run(json).await,
        Cmd::Tabs { json } => commands::tabs::run(json).await,
        Cmd::Attach { pattern } => commands::attach::run(pattern).await,
        Cmd::Network { last, json } => commands::network::run(last, json).await,
        Cmd::Console { last, json } => commands::console::run(last, json).await,
    }
}
