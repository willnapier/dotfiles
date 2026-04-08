//! `clinical-portal` — secure letter delivery HTTP server.
//!
//! This binary contains only the `serve` subcommand. The previous
//! laptop-side client subcommands (`share`, `status`, `revoke`,
//! `changes`) have moved into the main `clinical` binary so the
//! laptop workflow is unified under one entry point. This binary's
//! sole job is now to run on Fly.io and serve HTTP.

use clap::{Parser, Subcommand};

mod db;
mod email;
mod routes;

#[derive(Parser)]
#[command(name = "clinical-portal", about = "Clinical letter portal server")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the secure letter portal HTTP server
    Serve {
        /// Port to listen on
        #[arg(short, long, default_value = "3849")]
        port: u16,

        /// Database path
        #[arg(long, default_value = "clinical-portal.db")]
        db: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Serve { port, db } => {
            let pool = db::init(&db)?;
            let app = routes::router(pool);

            let addr = format!("0.0.0.0:{port}");
            println!("Clinical portal listening on {addr}");

            let listener = tokio::net::TcpListener::bind(&addr).await?;
            axum::serve(listener, app).await?;
        }
    }

    Ok(())
}
