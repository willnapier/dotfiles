//! Admin Dashboard — practice-wide web UI for administration.
//!
//! A separate Axum server from the practitioner dashboard, intended for
//! practice admin (Olly). Provides multi-practitioner calendar views,
//! client management, search, and billing overview.
//!
//! All static assets are embedded in the binary via `include_str!`.

mod handlers;
mod routes;
#[cfg(test)]
mod tests;

use anyhow::Result;

/// Start the admin dashboard on `{bind}:{port}`.
///
/// If `open_browser` is true, attempt to open the URL in the default browser.
pub async fn serve(port: u16, open_browser: bool) -> Result<()> {
    let app = routes::build_router();

    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("Admin dashboard running at http://{addr}");

    if open_browser {
        let _ = std::process::Command::new("xdg-open")
            .arg(format!("http://127.0.0.1:{port}"))
            .spawn();
    }

    axum::serve(listener, app).await?;
    Ok(())
}
