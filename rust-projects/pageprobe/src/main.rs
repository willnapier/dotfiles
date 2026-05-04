//! pageprobe — Rust CLI for inspecting Chrome via the DevTools Protocol.
//!
//! See `~/Assistants/shared/pageprobe.md` for design notes. v0.1 shipped
//! start, stop, status, tabs, attach, network, console. v0.2 adds eval,
//! screenshot, perf, dom.
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
    /// Evaluate a JS expression in the attached tab.
    ///
    /// EXPRESSION may be `-` to read JS from stdin (avoids shell-quoting
    /// pain for complex expressions).
    Eval {
        expression: String,
        /// Emit the raw `Runtime.RemoteObject` as JSON.
        #[arg(long)]
        json: bool,
        /// `awaitPromise: true` — wait for a returned promise to resolve.
        #[arg(long = "await")]
        await_promise: bool,
    },
    /// Capture a PNG (or JPEG with --quality) of the attached tab.
    Screenshot {
        /// Output path. Default: /tmp/pageprobe-<unix-ts>.png.
        path: Option<PathBuf>,
        /// Capture the full scrollable page, not just the viewport.
        #[arg(long)]
        full: bool,
        /// JPEG quality 0-100 (switches output extension to .jpg).
        #[arg(long)]
        quality: Option<i64>,
        /// Crop to "x,y,w,h" (CSS pixels).
        #[arg(long)]
        clip: Option<String>,
    },
    /// Report performance metrics for the attached tab.
    Perf {
        #[arg(long)]
        json: bool,
    },
    /// Query DOM via CSS selector.
    Dom {
        selector: String,
        /// Return outer HTML (default).
        #[arg(long)]
        html: bool,
        /// Return textContent only.
        #[arg(long)]
        text: bool,
        /// Return attributes as a JSON object.
        #[arg(long)]
        attrs: bool,
        /// Return all matches as an array.
        #[arg(long)]
        all: bool,
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
        Cmd::Eval {
            expression,
            json,
            await_promise,
        } => commands::eval::run(expression, json, await_promise).await,
        Cmd::Screenshot {
            path,
            full,
            quality,
            clip,
        } => commands::screenshot::run(path, full, quality, clip).await,
        Cmd::Perf { json } => commands::perf::run(json).await,
        Cmd::Dom {
            selector,
            html,
            text,
            attrs,
            all,
        } => commands::dom::run(selector, html, text, attrs, all).await,
    }
}
