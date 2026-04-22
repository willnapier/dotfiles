//! PracticeForge Dashboard — the unified web UI for all users.
//!
//! Primary views: Clinic (session workflow), Clients, Calendar, Search, Billing.
//! Keyboard-first navigation, solarized dark theme.
//!
//! Static assets embedded via `include_str!`, with PF_DEV=1 live-reload mode.

mod handlers;
mod routes;
#[cfg(test)]
mod tests;

use anyhow::Result;

/// Start the admin dashboard on `{bind}:{port}`.
///
/// If `open_browser` is true, attempt to open the URL in the default browser.
pub async fn serve(port: u16, open_browser: bool) -> Result<()> {
    let app = routes::build_router()
        .merge(crate::portal::build_router());

    // Dev mode uses port+1 so it can run alongside the production service
    let actual_port = if std::env::var("PF_DEV").is_ok() { port + 1 } else { port };
    // Bind to localhost only — PHI is served on this port.
    // Use 0.0.0.0 only behind a reverse proxy with auth.
    let addr = format!("127.0.0.1:{actual_port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    if std::env::var("PF_DEV").is_ok() {
        eprintln!("DEV dashboard at http://127.0.0.1:{actual_port} (live-reload from disk)");
    } else {
        eprintln!("Dashboard running at http://127.0.0.1:{actual_port}");
    }

    if open_browser {
        let url = format!("http://127.0.0.1:{actual_port}");
        // Per-OS browser launcher. `open` (macOS), `cmd /c start` (Windows),
        // `xdg-open` (Linux/BSD). The empty quoted arg after `start` is
        // required so the URL isn't parsed as a window title.
        let _ = if cfg!(target_os = "macos") {
            std::process::Command::new("open").arg(&url).spawn()
        } else if cfg!(target_os = "windows") {
            std::process::Command::new("cmd")
                .args(["/c", "start", "", &url])
                .spawn()
        } else {
            std::process::Command::new("xdg-open").arg(&url).spawn()
        };
    }

    axum::serve(listener, app).await?;
    Ok(())
}
